//! What the search tests of several modules share: a mock Gateway on a
//! tokio runtime, a route for it that records each search it receives, the
//! text of Host resources whose entry names it, and the search tool that a
//! test calls against it.

use std::net::SocketAddr;
use std::sync::{Arc, Mutex};

use axum::Json;
use axum::Router;
use axum::http::HeaderMap;
use axum::routing::post;
use runvil_plugin_sdk::{ToolError, ToolOutput};
use serde_json::{Value, json};
use tokio::runtime::Handle;

use crate::gateway::SearchClient;
use crate::search::WebSearch;

/// The path of the Gateway's web search route.
pub(crate) const ROUTE: &str = "/v1/tools/web_search";

/// The key [`search`] reaches a mock Gateway with.
pub(crate) const KEY: &str = "tok";

/// A mock gateway whose task is owned by the test: dropping it aborts the
/// server task deterministically instead of leaking a detached task.
pub(crate) struct MockServer {
    addr: SocketAddr,
    handle: tokio::task::JoinHandle<()>,
}

impl MockServer {
    /// Binds an ephemeral port, serves `router`, and returns the address.
    pub(crate) async fn spawn(router: Router) -> MockServer {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        let handle = tokio::spawn(async move {
            let _ = axum::serve(listener, router).await;
        });
        MockServer { addr, handle }
    }

    /// The address of the mock's web search route, as an entry of the
    /// Host's resources names it.
    pub(crate) fn url(&self) -> String {
        format!("http://{}{ROUTE}", self.addr)
    }
}

impl Drop for MockServer {
    fn drop(&mut self) {
        self.handle.abort();
    }
}

/// The `Authorization` header and the body, as text, of each search a mock
/// received. The body is text and not a parsed value, so that it shows the
/// order of its fields.
pub(crate) type Seen = Arc<Mutex<Vec<(Option<String>, String)>>>;

/// A mock Gateway that records each search and answers `reply`.
pub(crate) fn recording_gateway(seen: Seen, reply: Value) -> Router {
    Router::new().route(
        ROUTE,
        post(move |headers: HeaderMap, body: String| {
            let seen = Arc::clone(&seen);
            let reply = reply.clone();
            async move {
                let auth = headers
                    .get("authorization")
                    .and_then(|value| value.to_str().ok())
                    .map(str::to_owned);
                seen.lock()
                    .expect("the capture lock is healthy")
                    .push((auth, body));
                Json(reply)
            }
        }),
    )
}

/// The text of Host resources whose `runvil/web-search` entry is version 1
/// and names `url`, with `api_key` as its key when there is one.
pub(crate) fn resources(url: &str, api_key: Option<&str>) -> String {
    let mut entry = json!({ "version": 1, "url": url });
    if let Some(api_key) = api_key {
        entry["api_key"] = Value::from(api_key);
    }
    json!({ "runvil/web-search": entry }).to_string()
}

/// The search tool over `client`, on the test's runtime.
pub(crate) fn tool_over(client: SearchClient) -> WebSearch {
    WebSearch::new(client, Handle::current())
}

/// The search tool over the client a Plugin loads with, on the test's
/// runtime.
pub(crate) fn tool() -> WebSearch {
    tool_over(SearchClient::try_new().expect("the search client builds"))
}

/// Runs one search with `args` against the Gateway whose web search route
/// is at `url`, reached through an entry that holds [`KEY`].
pub(crate) async fn search(url: &str, args: Value) -> Result<ToolOutput, ToolError> {
    tool().call(args, &resources(url, Some(KEY))).await
}
