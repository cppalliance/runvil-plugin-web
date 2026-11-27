//! Response handling: content-type routing, bounded body reads, decoding, and
//! HTML-to-markdown extraction.
//!
//! The route is decided from the response `Content-Type` before the body is
//! read, so a binary or absent type is refused without downloading it. HTML and
//! structured text are read all-or-nothing under the byte cap; genuinely flat
//! text is truncated at the cap. Decoding honors the declared charset, and HTML
//! is rendered to markdown with readability plus a whole-page fallback, with
//! every relative link in it resolved against the URL it was served from.

use std::rc::Rc;

use html5ever::serialize::{SerializeOpts, serialize};
use markup5ever_rcdom::{Handle, NodeData, SerializableHandle};
use readabilityrs::elements::standardize_all;
use readabilityrs::markdown::html_to_markdown;
use readabilityrs::{MarkdownOptions, Readability, ReadabilityOptions};
use url::Url;

use crate::error::{FetchError, SafeUrl};
use crate::url_policy::MAX_URL_CHARS;

/// The length in characters below which an extracted article is replaced by
/// the whole page, however much of the page it is.
const MIN_ARTICLE_CHARS: usize = 250;

/// The length in characters below which an extracted article is replaced by
/// the whole page when it is also under [`MIN_ARTICLE_SHARE`] of it.
const SHORT_ARTICLE_CHARS: usize = 3_000;

/// The share of the whole page, as `1 / MIN_ARTICLE_SHARE`, under which a short
/// article is replaced by the whole page: 5 for 20%.
const MIN_ARTICLE_SHARE: usize = 5;

/// The elements the whole-page rendering leaves out, because their text is
/// code or style rules rather than content of the page. `noscript` stays,
/// because some sites, such as Discourse forums, serve their content in it.
const SKIPPED_TAGS: [&str; 2] = ["script", "style"];

/// How a response body was turned into the returned text.
///
/// The mode is reported on the provenance header's `extraction:` line so the
/// model knows whether it is holding an extracted article, a whole-page
/// rendering, or decoded plain text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Extraction {
    /// An HTML page's main article, isolated by [`readabilityrs`].
    Readability,
    /// A whole HTML document rendered to markdown, with no article extraction.
    RawHtml,
    /// A non-HTML text body decoded and returned verbatim, with no extraction.
    Plain,
}

impl Extraction {
    /// The label written on the provenance header's `extraction:` line.
    pub(crate) fn label(self) -> &'static str {
        match self {
            Extraction::Readability => "readability",
            Extraction::RawHtml => "raw-html",
            Extraction::Plain => "plain",
        }
    }
}

/// How a response's `Content-Type` routes through the fetch pipeline.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Route {
    /// `text/html` and `application/xhtml+xml`: readability plus htmd.
    Html,
    /// A non-HTML text body returned as decoded plain text, with no extraction.
    ///
    /// `structured` distinguishes JSON and XML (where a truncated prefix is
    /// invalid, so the body is read all-or-nothing on the byte cap) from
    /// genuinely flat text (where a prefix is a legitimate result, so an
    /// oversized body is truncated and flagged rather than refused).
    Plain {
        /// Whether the body is a structured format (JSON or XML) that must be
        /// read all-or-nothing rather than truncated to a prefix.
        structured: bool,
    },
}

/// Classifies a parsed content type into its fetch [`Route`].
///
/// `text/html` and `application/xhtml+xml` route to [`Route::Html`].
/// `application/json`, `application/xml`, `text/xml`, and any `+json`/`+xml`
/// suffix route to a structured [`Route::Plain`] (all-or-nothing on the byte
/// cap). Every other `text/*` routes to a flat [`Route::Plain`] (truncated on
/// oversize). Everything else (PDF, octet-stream, images, audio, video,
/// archives) returns [`None`], meaning refuse.
pub(crate) fn classify(mime: &mime::Mime) -> Option<Route> {
    let type_ = mime.type_();
    let subtype = mime.subtype();
    let suffix = mime.suffix();

    let is_html = (type_ == mime::TEXT && subtype == mime::HTML)
        || (type_ == mime::APPLICATION && subtype == "xhtml" && suffix == Some(mime::XML));
    if is_html {
        return Some(Route::Html);
    }

    let is_json_or_xml = subtype == mime::JSON
        || subtype == mime::XML
        || suffix == Some(mime::JSON)
        || suffix == Some(mime::XML);

    if type_ == mime::APPLICATION && is_json_or_xml {
        return Some(Route::Plain { structured: true });
    }

    if type_ == mime::TEXT {
        return Some(Route::Plain {
            structured: is_json_or_xml,
        });
    }

    None
}

/// Decodes response bytes to text using the declared charset.
///
/// An absent charset, or UTF-8, decodes as UTF-8 with lossy replacement of
/// invalid sequences. A declared non-UTF-8 charset is decoded through
/// [`encoding_rs`]. The header charset is authoritative: no embedded `meta`
/// charset is consulted.
///
/// # Errors
/// Returns [`FetchError::Undecodable`] if `charset` names a label
/// [`encoding_rs`] does not recognize.
pub(crate) fn decode_body(
    bytes: &[u8],
    charset: Option<&str>,
    url: &str,
) -> Result<String, FetchError> {
    match charset {
        None => Ok(String::from_utf8_lossy(bytes).into_owned()),
        Some(label)
            if label.eq_ignore_ascii_case("utf-8") || label.eq_ignore_ascii_case("utf8") =>
        {
            Ok(String::from_utf8_lossy(bytes).into_owned())
        }
        Some(label) => {
            let encoding = encoding_rs::Encoding::for_label(label.as_bytes()).ok_or_else(|| {
                FetchError::Undecodable {
                    url: SafeUrl::new(url),
                    charset: label.to_string(),
                }
            })?;
            Ok(encoding.decode(bytes).0.into_owned())
        }
    }
}

/// Renders an HTML page to markdown, reporting how it was produced.
///
/// When `raw` is true the whole document is rendered by [`whole_page`] and the
/// mode is [`Extraction::RawHtml`], skipping article extraction entirely.
/// Otherwise [`readabilityrs`] isolates the main article, and the whole page
/// is returned in its place, with the mode [`Extraction::RawHtml`], when the
/// article is under [`MIN_ARTICLE_CHARS`] characters, or under
/// [`SHORT_ARTICLE_CHARS`] and under a fifth of the whole page: such an
/// article is a fragment of a page that is not an article, such as a profile
/// or a list of links. Rendering never fails: the worst case is an empty
/// string when even the whole-page rendering produces no text.
///
/// Each relative link and image source in what is rendered is made absolute
/// against `base_url`, the URL the page was served from, as [`absolutize`]
/// says, so a model can fetch what the markdown links to. On the readability
/// path this happens after `readabilityrs` has picked each image's source,
/// which may come from `data-src` or `srcset` rather than `src`.
pub(crate) fn extract_html(html: &str, base_url: Option<&Url>, raw: bool) -> (String, Extraction) {
    let document = base_url.and_then(|_| parse(html));
    let base = document
        .as_ref()
        .zip(base_url)
        .and_then(|(document, page_url)| LinkBase::of(document, page_url));
    let render_whole_page = |document: Option<Handle>| {
        let html = document
            .zip(base.as_ref())
            .and_then(|(document, base)| absolutize(document, base, html.len()))
            .unwrap_or_else(|| html.to_owned());
        whole_page(&html)
    };

    if raw {
        return (render_whole_page(document), Extraction::RawHtml);
    }

    let options = ReadabilityOptions::default();
    let markdown_options = MarkdownOptions {
        sanitize_urls: options.sanitize_content,
        ..MarkdownOptions::default()
    };

    // What `output_markdown` does inside `readabilityrs`, with the links
    // resolved between picking the image sources and rendering.
    let article_markdown = Readability::new(html, base_url.map(Url::as_str), Some(options))
        .ok()
        .and_then(Readability::parse)
        .and_then(|article| {
            let content = standardize_all(&article.content?, article.title.as_deref());
            let content = base
                .as_ref()
                .and_then(|base| absolutize(parse(&content)?, base, content.len()))
                .unwrap_or(content);
            Some(html_to_markdown(&content, &markdown_options))
        })
        .unwrap_or_default();

    let article_chars = article_markdown.trim().chars().count();
    if article_chars >= SHORT_ARTICLE_CHARS {
        return (article_markdown, Extraction::Readability);
    }
    let page_markdown = render_whole_page(document);
    let page_chars = page_markdown.trim().chars().count();
    if article_chars >= MIN_ARTICLE_CHARS && article_chars * MIN_ARTICLE_SHARE >= page_chars {
        return (article_markdown, Extraction::Readability);
    }
    if page_chars == 0 {
        return (article_markdown, Extraction::Readability);
    }
    (page_markdown, Extraction::RawHtml)
}

/// Renders a whole HTML document to markdown, leaving out [`SKIPPED_TAGS`].
fn whole_page(html: &str) -> String {
    htmd::HtmlToMarkdown::builder()
        .skip_tags(SKIPPED_TAGS.to_vec())
        .build()
        .convert(html)
        .unwrap_or_default()
}

/// Parses `html` into the tree `htmd` renders from.
fn parse(html: &str) -> Option<Handle> {
    htmd::HtmlToMarkdown::new().html_to_tree(html).ok()
}

/// What a page's relative references resolve against.
struct LinkBase {
    /// The page's first `<base href>`, resolved against the page URL, when
    /// that is `http` or `https`, and the page URL otherwise.
    url: Url,
    /// Whether `url` names the page itself, so that a link that is only a
    /// fragment points into the page and stays as written.
    is_page: bool,
}

impl LinkBase {
    /// The base of `document`, served from `page_url`, or `None` when it is
    /// over [`MAX_URL_CHARS`]: every reference that resolves against it then
    /// stays as written, and the cost of each resolve stays under the length
    /// of a URL a fetch would take.
    fn of(document: &Handle, page_url: &Url) -> Option<Self> {
        let url = descendants(document)
            .find_map(|node| match &node.data {
                NodeData::Element { name, attrs, .. } if &*name.local == "base" => attrs
                    .borrow()
                    .iter()
                    .find(|attr| &*attr.name.local == "href")
                    .map(|attr| attr.value.trim().to_owned()),
                _ => None,
            })
            .and_then(|href| page_url.join(&href).ok())
            .filter(|base| matches!(base.scheme(), "http" | "https"))
            .unwrap_or_else(|| page_url.clone());
        // A serialized URL is ASCII, so its byte length is its character count.
        if url.as_str().len() > MAX_URL_CHARS {
            return None;
        }
        let is_page = url[..url::Position::AfterQuery] == page_url[..url::Position::AfterQuery];
        Some(Self { url, is_page })
    }
}

/// Serializes `document` with every relative `href` of an `a` and `src` of an
/// `img` resolved against `base` to an absolute `http` or `https` URL, or
/// returns `None` when the tree does not serialize to text.
///
/// A reference that is already absolute, such as a `mailto:` link, is left as
/// written, as is one that does not resolve to `http` or `https`, or that
/// resolves to a URL over [`MAX_URL_CHARS`], which no fetch would take. A
/// reference that is only a fragment is left as written when `base` is the
/// page itself, and resolves like any other when a `<base href>` points it
/// elsewhere, as a browser resolves it.
///
/// `budget` bounds the work, and is given the length of the HTML the tree was
/// parsed from. Each resolve spends from it: one that is kept, the bytes it
/// adds to the page, and one that is thrown away, the length of the URL it
/// built. When the budget runs out, the walk stops and the rest stay as
/// written, so the result is at most about twice the size of the HTML, and
/// resolving costs about as much as one more pass over it.
fn absolutize(document: Handle, base: &LinkBase, mut budget: usize) -> Option<String> {
    'walk: for node in descendants(&document) {
        let NodeData::Element { name, attrs, .. } = &node.data else {
            continue;
        };
        let target = match &*name.local {
            "a" => "href",
            "img" => "src",
            _ => continue,
        };
        for attr in attrs.borrow_mut().iter_mut() {
            if &*attr.name.local != target {
                continue;
            }
            let reference = attr.value.trim();
            if reference.is_empty()
                || (reference.starts_with('#') && base.is_page)
                || Url::parse(reference).is_ok()
            {
                continue;
            }
            let Ok(url) = base.url.join(reference) else {
                continue;
            };
            let len = url.as_str().len();
            let keep = matches!(url.scheme(), "http" | "https") && len <= MAX_URL_CHARS;
            let cost = if keep {
                len.saturating_sub(attr.value.len())
            } else {
                len
            };
            let Some(rest) = budget.checked_sub(cost) else {
                break 'walk;
            };
            budget = rest;
            if keep {
                attr.value = url.as_str().into();
            }
        }
    }
    let mut out = Vec::new();
    serialize(
        &mut out,
        &SerializableHandle::from(document),
        SerializeOpts::default(),
    )
    .ok()?;
    String::from_utf8(out).ok()
}

/// Every node under `root`, in document order, walked with a stack rather than
/// recursion, so a page nested thousands of levels deep cannot overflow the
/// thread's stack.
fn descendants(root: &Handle) -> impl Iterator<Item = Handle> {
    let mut stack = vec![Rc::clone(root)];
    std::iter::from_fn(move || {
        let node = stack.pop()?;
        stack.extend(node.children.borrow().iter().rev().cloned());
        Some(node)
    })
}

/// Reads a response body into memory, truncating at a decompressed-byte cap.
///
/// Unlike [`read_body_capped`], a body over `max_bytes` is not refused: the read
/// stops at `max_bytes` and the returned flag is `true`, because a flat-text
/// prefix is still useful. The count runs over the decompressed stream, so a
/// compressed payload is measured on its expanded size.
///
/// # Errors
/// Returns [`FetchError::BodyRead`] on a transport failure mid-stream.
pub(crate) async fn read_body_truncating(
    response: reqwest::Response,
    max_bytes: usize,
) -> Result<(Vec<u8>, bool), FetchError> {
    let url = response.url().to_string();
    let mut response = response;
    let mut body: Vec<u8> = Vec::new();
    let mut truncated = false;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| FetchError::BodyRead {
            url: SafeUrl::new(&url),
            source,
        })?
    {
        let remaining = max_bytes - body.len();
        if chunk.len() > remaining {
            body.extend_from_slice(&chunk[..remaining]);
            truncated = true;
            break;
        }
        body.extend_from_slice(&chunk);
    }
    Ok((body, truncated))
}

/// Reads a response body into memory under a decompressed-byte cap.
///
/// A declared `Content-Length` greater than `max_bytes` is refused before the
/// body is read. Otherwise the body is streamed and counted as it arrives
/// (reqwest decompresses in the stream, so the count is on decompressed bytes),
/// and the read is aborted the moment the running total exceeds `max_bytes`. A
/// body of exactly `max_bytes` is accepted.
///
/// # Errors
/// Returns [`FetchError::TooLarge`] if the response exceeds `max_bytes`, or
/// [`FetchError::BodyRead`] on a transport failure mid-stream.
pub(crate) async fn read_body_capped(
    mut response: reqwest::Response,
    url: &str,
    max_bytes: usize,
) -> Result<Vec<u8>, FetchError> {
    let too_large = || FetchError::TooLarge {
        url: SafeUrl::new(url),
        limit: max_bytes,
    };

    if let Some(len) = response.content_length()
        && len > max_bytes as u64
    {
        return Err(too_large());
    }

    let mut body: Vec<u8> = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|source| FetchError::BodyRead {
            url: SafeUrl::new(url),
            source,
        })?
    {
        if body.len() + chunk.len() > max_bytes {
            return Err(too_large());
        }
        body.extend_from_slice(&chunk);
    }
    Ok(body)
}

/// Truncates `text` to at most `max_chars` characters on a character boundary.
///
/// Returns the (possibly shortened) text and whether it was truncated. The cut
/// falls on a [`char`] boundary, so a multibyte character is never split. Text
/// of exactly `max_chars` characters is returned untruncated.
pub(crate) fn truncate_to_chars(text: &str, max_chars: usize) -> (&str, bool) {
    match text.char_indices().nth(max_chars) {
        Some((idx, _)) => (&text[..idx], true),
        None => (text, false),
    }
}

#[cfg(test)]
mod tests {
    use std::fmt::Write as _;

    use url::Url;

    use super::{Extraction, LinkBase, absolutize, extract_html, parse};
    use crate::url_policy::MAX_URL_CHARS;

    /// Parses `s` into a [`Url`], panicking with context on failure.
    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url must parse")
    }

    /// `html` with its links resolved as [`extract_html`] resolves them on a
    /// page served from `page_url`.
    fn absolutize_page(html: &str, page_url: &str) -> String {
        let document = parse(html).expect("the page parses");
        let base = LinkBase::of(&document, &url(page_url)).expect("the base is short enough");
        absolutize(document, &base, html.len()).expect("the page serializes")
    }

    /// A page whose article readability keeps, with `links` as its last
    /// paragraph.
    fn article(links: &str) -> String {
        let prose = "<p>This paragraph is long enough for the extractor to treat it as \
                     real article content rather than navigation or boilerplate.</p>"
            .repeat(4);
        format!(
            "<html><head><title>T</title></head><body><article><h1>Title</h1>\
             {prose}<p>{links}</p></article></body></html>"
        )
    }

    #[test]
    fn extracts_article_body_and_drops_boilerplate() {
        let html = r#"
            <html>
              <body>
                <nav><a href="/home">Home</a><a href="/about">About Us Navigation</a></nav>
                <article>
                  <h1>The Title Of The Piece</h1>
                  <p>This is the first substantial paragraph of the article body,
                     long enough to be treated as real content by the extractor.</p>
                  <p>Here is a second paragraph that continues the discussion with
                     even more prose so the reader has plenty of material to read.</p>
                  <p>A third and final paragraph rounds out the article nicely and
                     keeps the character count comfortably above the threshold.</p>
                </article>
                <footer>Copyright boilerplate footer text here.</footer>
              </body>
            </html>
        "#;

        let (markdown, _mode) =
            extract_html(html, Some(&url("https://example.com/article")), false);

        assert!(
            markdown.contains("first substantial paragraph"),
            "expected article body in output, got: {markdown}"
        );
        assert!(
            !markdown.contains("About Us Navigation"),
            "navigation boilerplate should be stripped, got: {markdown}"
        );
    }

    #[test]
    fn falls_back_for_non_article_html() {
        let html = "<div>short</div>";

        let (markdown, _mode) = extract_html(html, None, false);

        assert!(
            !markdown.trim().is_empty(),
            "fallback conversion should return non-empty markdown, got: {markdown:?}"
        );
        assert!(
            markdown.contains("short"),
            "fallback should preserve the page text, got: {markdown}"
        );
    }

    #[test]
    fn an_article_links_absolutely_to_what_it_names() {
        let html = article(
            r#"See <a href="/learn">learn</a>, <a href="guide/intro.html">intro</a>, and <img src="../img/a.png" alt="a">."#,
        );

        let (markdown, mode) = extract_html(
            &html,
            Some(&url("https://www.rust-lang.org/tools/install")),
            false,
        );

        assert_eq!(mode, Extraction::Readability, "got: {markdown}");
        for link in [
            "(https://www.rust-lang.org/learn)",
            "(https://www.rust-lang.org/tools/guide/intro.html)",
            "(https://www.rust-lang.org/img/a.png)",
        ] {
            assert!(markdown.contains(link), "expected {link} in: {markdown}");
        }
    }

    #[test]
    fn a_raw_page_links_absolutely_to_what_it_names() {
        let html = r#"<nav><a href="/docs">Docs</a> <a href="//cdn.example.org/x">CDN</a></nav>"#;

        let (markdown, mode) = extract_html(html, Some(&url("https://example.com/a/b")), true);

        assert_eq!(mode, Extraction::RawHtml);
        assert!(
            markdown.contains("(https://example.com/docs)"),
            "got: {markdown}"
        );
        assert!(
            markdown.contains("(https://cdn.example.org/x)"),
            "got: {markdown}"
        );
    }

    #[test]
    fn a_base_element_moves_where_links_resolve() {
        let html = r#"<head><base href="https://mirror.example.net/v2/"></head><body><a href="page">Page</a></body>"#;

        let (markdown, _mode) = extract_html(html, Some(&url("https://example.com/a/b")), true);

        assert!(
            markdown.contains("(https://mirror.example.net/v2/page)"),
            "got: {markdown}"
        );
    }

    #[test]
    fn a_base_element_that_is_not_http_is_ignored() {
        let html =
            r#"<head><base href="ftp://example.net/"></head><body><a href="page">Page</a></body>"#;

        let (markdown, _mode) = extract_html(html, Some(&url("https://example.com/a/b")), true);

        assert!(
            markdown.contains("(https://example.com/a/page)"),
            "got: {markdown}"
        );
    }

    #[test]
    fn links_that_need_no_resolving_stay_as_written() {
        let html = r##"<p><a href="#top">Top</a> <a href="mailto:a@example.com">Mail</a> <a href="https://other.example/x">Other</a> <a href="javascript:void(0)">Js</a></p>"##;

        let (markdown, _mode) = extract_html(html, Some(&url("https://example.com/a")), true);

        for link in [
            "(#top)",
            "(mailto:a@example.com)",
            "(https://other.example/x)",
            "(javascript:void",
        ] {
            assert!(markdown.contains(link), "expected {link} in: {markdown}");
        }
    }

    #[test]
    fn without_a_base_url_links_stay_as_written() {
        let html = r#"<p><a href="/docs">Docs</a></p>"#;

        let (markdown, _mode) = extract_html(html, None, true);

        assert!(markdown.contains("(/docs)"), "got: {markdown}");
    }

    #[test]
    fn a_base_too_long_to_fetch_leaves_every_link_as_written() {
        let base = format!("https://example.com/{}/", "a".repeat(MAX_URL_CHARS));
        let html = format!(
            r#"<head><base href="{base}"></head><body><a href="page">Page</a> <a href="/root">Root</a></body>"#
        );

        let (markdown, _mode) = extract_html(&html, Some(&url("https://example.com/")), true);

        assert!(markdown.contains("(page)"), "got: {markdown}");
        assert!(markdown.contains("(/root)"), "got: {markdown}");
    }

    #[test]
    fn a_link_too_long_to_fetch_stays_as_written() {
        // The base is two characters short of the limit, so `page` goes over it.
        let base = format!("https://example.com/{}/", "a".repeat(MAX_URL_CHARS - 23));
        let html =
            format!(r#"<head><base href="{base}"></head><body><a href="page">Page</a></body>"#);

        let (markdown, _mode) = extract_html(&html, Some(&url("https://example.com/")), true);

        assert!(markdown.contains("(page)"), "got: {markdown}");
    }

    #[test]
    fn links_thrown_away_as_too_long_still_spend_the_budget() {
        let base = format!("https://example.com/{}/", "a".repeat(MAX_URL_CHARS - 23));
        let long = r#"<a href="page">x</a>"#.repeat(100);
        let html = format!(
            r#"<html><head><base href="{base}"></head><body>{long}<a href="/ok">ok</a></body></html>"#
        );

        let out = absolutize_page(&html, "https://example.com/");

        assert!(
            out.ends_with(r#"<a href="/ok">ok</a></body></html>"#),
            "the walk stops before the last link"
        );
    }

    #[test]
    fn a_fragment_resolves_against_a_base_elsewhere() {
        let html =
            r##"<head><base href="/manual/"></head><body><a href="#section">Section</a></body>"##;

        let (markdown, _mode) = extract_html(html, Some(&url("https://example.com/guide/")), true);

        assert!(
            markdown.contains("(https://example.com/manual/#section)"),
            "got: {markdown}"
        );
    }

    #[test]
    fn a_fragment_stays_bare_when_the_base_is_the_page() {
        let html = r##"<head><base href="/guide/#intro"></head><body><a href="#section">Section</a></body>"##;

        let (markdown, _mode) = extract_html(html, Some(&url("https://example.com/guide/")), true);

        assert!(markdown.contains("(#section)"), "got: {markdown}");
    }

    #[test]
    fn an_article_image_resolves_the_source_readability_picks() {
        let html = article(
            r#"<img src="data:image/gif;base64,R0lGODlhAQABAIAAAAAAAP///yH5BAEAAAAALAAAAAABAAEAAAIBRAA7" data-src="img/lazy.png" alt="lazy">
               <img src="img/small.png" srcset="img/small.png 400w, img/large.png 1200w" alt="wide">"#,
        );

        let (markdown, mode) = extract_html(&html, Some(&url("https://example.com/a/b")), false);

        assert_eq!(mode, Extraction::Readability, "got: {markdown}");
        for link in [
            "(https://example.com/a/img/lazy.png)",
            "(https://example.com/a/img/large.png)",
        ] {
            assert!(markdown.contains(link), "expected {link} in: {markdown}");
        }
    }

    #[test]
    fn resolving_at_most_doubles_the_page() {
        let base = format!("https://example.com/{}/", "a".repeat(1_900));
        let links = r#"<a href="b">x</a>"#.repeat(1_000);
        let html = format!(r#"<html><head><base href="{base}"></head><body>{links}</body></html>"#);

        let out = absolutize_page(&html, "https://example.com/");

        assert!(
            out.len() <= 2 * html.len(),
            "{} bytes resolved from {}",
            out.len(),
            html.len()
        );
        assert!(
            out.contains(&format!(r#"href="{base}b""#)),
            "the first links resolve"
        );
        assert!(
            out.ends_with(r#"<a href="b">x</a></body></html>"#),
            "the last link stays as written"
        );
    }

    #[test]
    fn a_table_with_a_long_cell_stays_under_four_times_its_size_on_both_paths() {
        let cell = "word ".repeat(1_000);
        let rows = format!("<tr><td>{cell}</td><td>short</td></tr>").repeat(4);
        let html = format!(
            "<html><body><table><tr><th>Long</th><th>Short</th></tr>{rows}</table></body></html>"
        );

        for raw in [false, true] {
            let (markdown, _mode) = extract_html(&html, Some(&url("https://example.com/")), raw);

            assert!(
                markdown.len() < 4 * html.len(),
                "raw {raw}: {} bytes from {}",
                markdown.len(),
                html.len()
            );
        }
    }

    const PROFILE: &str = r#"<!doctype html>
<html>
  <head>
    <title>Jane Doe</title>
    <style>.card { color: red; }</style>
    <script type="application/json">{"featureFlags":["one","two"]}</script>
  </head>
  <body>
    <nav><a href="/">Home</a> <a href="/login">Sign in</a></nav>
    <div class="profile">
      <h1>Jane Doe</h1>
      <ul>
        <li>Turku, Finland</li>
        <li><a href="https://example.com/jane">example.com/jane</a></li>
      </ul>
    </div>
    <div class="pinned">
      <h2>Pinned</h2>
      <ol>
        <li>
          <p>Neon: Serverless Postgres. We separated storage and compute to offer autoscaling, code-like database branching, and scale to zero.</p>
        </li>
      </ol>
    </div>
  </body>
</html>"#;

    #[test]
    fn default_result_keeps_the_profile() {
        let (markdown, mode) = extract_html(PROFILE, None, false);

        assert!(
            markdown.contains("Jane Doe") && markdown.contains("Turku, Finland"),
            "mode {mode:?}, got: {markdown}"
        );
    }

    #[test]
    fn raw_result_holds_no_script_or_style_text() {
        let (markdown, _mode) = extract_html(PROFILE, None, true);

        assert!(
            !markdown.contains("color: red") && !markdown.contains("featureFlags"),
            "got: {markdown}"
        );
    }

    /// A page with an article of `prose_repeats` copies of one sentence, about
    /// 90 characters each, and a list of `links` stories outside it.
    fn article_beside_a_list(prose_repeats: usize, links: usize) -> String {
        let prose =
            "This sentence belongs to the article, which a reader came to the page to read. "
                .repeat(prose_repeats);
        let mut list = String::new();
        for i in 0..links {
            write!(
                list,
                r#"<li><a href="/s/{i}">Story number {i}, with a title of some length</a></li>"#
            )
            .expect("writing to a String cannot fail");
        }
        format!(
            "<html><head><title>T</title></head><body><ul>{list}</ul>\
             <article><h1>Title</h1><p>{prose}</p></article></body></html>"
        )
    }

    #[test]
    fn a_short_article_under_a_fifth_of_the_page_gives_way_to_the_whole_page() {
        let html = article_beside_a_list(5, 100);

        let (markdown, mode) = extract_html(&html, None, false);

        assert_eq!(mode, Extraction::RawHtml, "got: {markdown}");
        assert!(markdown.contains("Story number 99"), "got: {markdown}");
        assert!(
            markdown.contains("belongs to the article"),
            "got: {markdown}"
        );
    }

    #[test]
    fn a_short_article_that_is_most_of_the_page_stays_the_result() {
        let html = article_beside_a_list(5, 3);

        let (markdown, mode) = extract_html(&html, None, false);

        assert_eq!(mode, Extraction::Readability, "got: {markdown}");
        assert!(!markdown.contains("Story number"), "got: {markdown}");
    }

    #[test]
    fn the_whole_page_skips_script_and_style_and_keeps_noscript() {
        let html = r#"<html><head><style>.card { color: red; }</style></head><body>
            <script>window.boot = {"flags": 1};</script>
            <noscript><p>The topic list, served for readers without scripts.</p></noscript>
            <p>Status: up.</p></body></html>"#;

        for raw in [false, true] {
            let (markdown, mode) = extract_html(html, None, raw);

            assert_eq!(mode, Extraction::RawHtml, "raw {raw}: {markdown}");
            assert!(
                !markdown.contains("color: red") && !markdown.contains("window.boot"),
                "raw {raw}: {markdown}"
            );
            assert!(
                markdown.contains("The topic list") && markdown.contains("Status: up."),
                "raw {raw}: {markdown}"
            );
        }
    }
}
