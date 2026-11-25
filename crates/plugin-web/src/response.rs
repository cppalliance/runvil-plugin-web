//! Response handling: content-type routing, bounded body reads, decoding, and
//! HTML-to-markdown extraction.
//!
//! The route is decided from the response `Content-Type` before the body is
//! read, so a binary or absent type is refused without downloading it. HTML and
//! structured text are read all-or-nothing under the byte cap; genuinely flat
//! text is truncated at the cap. Decoding honors the declared charset, and HTML
//! is rendered to markdown with readability plus a whole-page fallback, after
//! every relative link in it is resolved against the URL it was served from.

use std::rc::Rc;

use html5ever::serialize::{SerializeOpts, serialize};
use markup5ever_rcdom::{Handle, NodeData, SerializableHandle};
use readabilityrs::{Readability, ReadabilityOptions};
use url::Url;

use crate::error::{FetchError, SafeUrl};
use crate::url_policy::MAX_URL_CHARS;

/// The minimum extracted length below which the whole-page fallback fires.
const MIN_CONTENT_LEN: usize = 100;

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
/// When `raw` is true the whole document is converted with [`htmd::convert`] and
/// the mode is [`Extraction::RawHtml`], skipping article extraction entirely.
/// Otherwise [`readabilityrs`] isolates the main article; if that yields fewer
/// than [`MIN_CONTENT_LEN`] characters, the whole document is converted instead
/// and the mode is [`Extraction::RawHtml`]. Rendering never fails: the worst
/// case is an empty string when even the whole-page conversion produces no text.
///
/// Before either renders the page, each relative link and image source in it
/// is made absolute against `base_url`, the URL the page was served from, as
/// [`absolutize`] says, so a model can fetch what the markdown links to.
pub(crate) fn extract_html(html: &str, base_url: Option<&Url>, raw: bool) -> (String, Extraction) {
    let absolute = base_url.and_then(|base| absolutize(html, base));
    let html = absolute.as_deref().unwrap_or(html);

    if raw {
        return (htmd::convert(html).unwrap_or_default(), Extraction::RawHtml);
    }

    let options = ReadabilityOptions {
        output_markdown: true,
        ..ReadabilityOptions::default()
    };

    let article_markdown = Readability::new(html, base_url.map(Url::as_str), Some(options))
        .ok()
        .and_then(Readability::parse)
        .and_then(|article| {
            article.markdown_content.or_else(|| {
                article
                    .content
                    .and_then(|content| htmd::convert(&content).ok())
            })
        })
        .unwrap_or_default();

    if article_markdown.trim().len() >= MIN_CONTENT_LEN {
        return (article_markdown, Extraction::Readability);
    }

    (
        htmd::convert(html).unwrap_or(article_markdown),
        Extraction::RawHtml,
    )
}

/// Returns `html` with every relative `href` of an `a` and `src` of an `img`
/// resolved to an absolute `http` or `https` URL, or `None` when the page does
/// not parse back into text.
///
/// A reference resolves against the page's first `<base href>` when it names an
/// `http` or `https` URL, and against `page_url` otherwise, as a browser
/// resolves it. A reference that is already absolute, such as a `mailto:`
/// link, one that is only a fragment, which points into the page itself, and
/// one that does not resolve to `http` or `https` are left as written, as is
/// one whose absolute form [`check_url`](crate::url_policy::check_url) would
/// refuse as too long.
///
/// Resolving lengthens a link by up to the length of the base, which the page
/// chooses with `<base href>`, so a page of many short links could otherwise
/// grow far past the byte cap of its body. The links resolve in document order
/// until they have added as many bytes as the page holds, and the rest stay as
/// written, so the result is at most about twice the size of `html`.
fn absolutize(html: &str, page_url: &Url) -> Option<String> {
    let document = htmd::HtmlToMarkdown::new().html_to_tree(html).ok()?;
    let base = document_base(&document, page_url);
    let mut budget = html.len();
    for node in descendants(&document) {
        let NodeData::Element { name, attrs, .. } = &node.data else {
            continue;
        };
        let target = match &*name.local {
            "a" => "href",
            "img" => "src",
            _ => continue,
        };
        for attr in attrs.borrow_mut().iter_mut() {
            if &*attr.name.local == target
                && let Some(url) = resolve(&attr.value, &base)
                && let Some(rest) =
                    budget.checked_sub(url.as_str().len().saturating_sub(attr.value.len()))
            {
                budget = rest;
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

/// The URL a page's relative references resolve against: its first
/// `<base href>`, resolved against `page_url`, when that is `http` or `https`,
/// and `page_url` otherwise.
fn document_base(document: &Handle, page_url: &Url) -> Url {
    descendants(document)
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
        .unwrap_or_else(|| page_url.clone())
}

/// Resolves `reference` against `base`, or `None` when it is empty, already
/// absolute, only a fragment, or resolves to a scheme other than `http` or
/// `https` or to a URL over [`MAX_URL_CHARS`], which no fetch would take.
fn resolve(reference: &str, base: &Url) -> Option<Url> {
    let reference = reference.trim();
    if reference.is_empty() || reference.starts_with('#') || Url::parse(reference).is_ok() {
        return None;
    }
    let url = base.join(reference).ok()?;
    // A serialized URL is ASCII, so its byte length is its character count.
    (matches!(url.scheme(), "http" | "https") && url.as_str().len() <= MAX_URL_CHARS).then_some(url)
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
    use url::Url;

    use super::{Extraction, absolutize, extract_html};
    use crate::url_policy::MAX_URL_CHARS;

    /// Parses `s` into a [`Url`], panicking with context on failure.
    fn url(s: &str) -> Url {
        Url::parse(s).expect("test url must parse")
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
    fn a_link_too_long_to_fetch_stays_as_written() {
        let base = format!("https://example.com/{}/", "a".repeat(MAX_URL_CHARS));
        let html =
            format!(r#"<head><base href="{base}"></head><body><a href="page">Page</a></body>"#);

        let (markdown, _mode) = extract_html(&html, Some(&url("https://example.com/")), true);

        assert!(markdown.contains("(page)"), "got: {markdown}");
    }

    #[test]
    fn resolving_at_most_doubles_the_page() {
        let base = format!("https://example.com/{}/", "a".repeat(1_900));
        let links = r#"<a href="b">x</a>"#.repeat(1_000);
        let html = format!(r#"<html><head><base href="{base}"></head><body>{links}</body></html>"#);

        let out = absolutize(&html, &url("https://example.com/")).expect("the page serializes");

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
}
