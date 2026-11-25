//! Golden tests for [`WebFetch`]: the whole text a fetch returns for a saved
//! page, header included, on the readability path, with `raw`, and on the
//! fallback for a page too short to hold an article, and the `url:` line
//! after a redirect.
//!
//! The pages and the texts they render to sit in `tests/golden/`. A change to
//! the rendering, such as an upgrade of `readabilityrs` or `htmd`, shows as a
//! failure here; `GOLDEN_BLESS=1 cargo nextest run` rewrites each text from
//! the output, and the diff of `tests/golden/` is the review of the change.

use std::path::PathBuf;

use super::*;

const ARTICLE_PATH: &str = "/guide/install/article";
const SHORT_PATH: &str = "/status/short";

async fn article() -> Html<&'static str> {
    Html(include_str!("../../tests/golden/article.html"))
}

async fn short() -> Html<&'static str> {
    Html(include_str!("../../tests/golden/short.html"))
}

async fn moved() -> Redirect {
    Redirect::permanent(ARTICLE_PATH)
}

/// Serves the golden pages on a loopback port, and a redirect to the article
/// from `/moved`.
async fn spawn_golden_server() -> u16 {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("binding a loopback listener must succeed");
    let port = listener
        .local_addr()
        .expect("the listener must have a local address")
        .port();
    let app = Router::new()
        .route(ARTICLE_PATH, get(article))
        .route(SHORT_PATH, get(short))
        .route("/moved", get(moved));
    tokio::spawn(async move {
        axum::serve(listener, app)
            .await
            .expect("the loopback server must serve");
    });
    port
}

/// Fetches `path` from the golden server with `raw`, and returns the text
/// with the server's port written as `PORT`, so it does not change between
/// runs.
async fn fetch(path: &str, raw: bool) -> String {
    let port = spawn_golden_server().await;
    let out = loopback_tool(port)
        .call(serde_json::json!({ "url": format!("http://localhost:{port}{path}"), "raw": raw }))
        .await
        .expect("a loopback fetch of a golden page must succeed")
        .text()
        .to_owned();
    out.replace(&format!("localhost:{port}"), "localhost:PORT")
}

/// Compares `actual` with the golden text in `tests/golden/{name}`, or writes
/// it there when `GOLDEN_BLESS` is set.
fn assert_golden(name: &str, actual: &str) {
    let path: PathBuf = [env!("CARGO_MANIFEST_DIR"), "tests", "golden", name]
        .iter()
        .collect();
    if std::env::var_os("GOLDEN_BLESS").is_some() {
        std::fs::write(&path, actual).expect("the golden text must be writable");
        return;
    }
    let expected = std::fs::read_to_string(&path).unwrap_or_else(|err| {
        panic!(
            "{}: {err}; run with GOLDEN_BLESS=1 to write it",
            path.display()
        )
    });
    assert!(
        actual == expected,
        "the output differs from {}; run with GOLDEN_BLESS=1 and review the diff\n\
         --- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_article_renders_as_its_golden_text() {
    assert_golden("article.md", &fetch(ARTICLE_PATH, false).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn an_article_with_raw_renders_as_its_golden_text() {
    assert_golden("article-raw.md", &fetch(ARTICLE_PATH, true).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_page_too_short_for_an_article_falls_back_to_its_golden_text() {
    assert_golden("short.md", &fetch(SHORT_PATH, false).await);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn after_a_redirect_the_url_line_names_the_page_that_answered() {
    let out = fetch("/moved", false).await;

    let (header, _body) = split_header(&out);
    assert_eq!(
        header.lines().next(),
        Some(format!("url: http://localhost:PORT{ARTICLE_PATH}").as_str()),
        "got: {header}"
    );
}
