//! The Gateway's web search, as the search tool reaches it:
//! [`WebSearchResource`], the entry of the Host's resources that says
//! where a search goes, and [`SearchClient`], which sends one there. Each
//! search is one `POST` to the entry's `url`, with the Gateway's key as
//! the bearer when the entry holds one, under a fixed deadline, with a
//! bounded and sanitized error body and a capped success body parsed into
//! [`SearchResults`], which mirror the Gateway's response.
//!
//! This is the one module that takes the key out of the Host's resources.
//! The entry's fields are private to it, so the code of the search tool
//! can pass an entry on and cannot print what it holds.

use std::fmt;
use std::time::Duration;

use runvil_plugin_sdk::{ToolError, ToolErrorKind};
use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The name of the entry of the Host's resources that says where a search
/// goes. A Host offers the entry under the same name.
const RESOURCE: &str = "runvil/web-search";

/// The version of the entry this client reads. In version 1 a search is
/// one POST of the Gateway's web search request, as JSON, to the entry's
/// `url`, and the reply is the Gateway's web search reply.
const VERSION: u32 = 1;

/// The largest error body kept for diagnostics, in bytes.
const MAX_ERROR_BODY: usize = 2000;

/// The largest successful response body accepted from the gateway, in bytes.
///
/// Search results include third-party web content, so the body is bounded
/// to keep a hostile or misbehaving upstream from returning an unbounded
/// payload. A body past this cap is rejected rather than silently
/// truncated, since a truncated JSON document is not a valid result set.
const MAX_RESPONSE_BODY: usize = 256 * 1024;

/// The deadline applied to every outbound request, body included, so a
/// stalled gateway cannot hang a search (and thus a run) indefinitely.
const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

/// The `runvil/web-search` entry of the Host's resources: where a search
/// goes. Version 1 holds `url`, the address of the Gateway's web search
/// route, and `api_key`, the Gateway's key.
///
/// `api_key` is optional. With it, the request of a search has
/// `Authorization: Bearer <api_key>`. Without it, or with an empty one,
/// the request has no `Authorization` header at all. The Gateway admits a
/// caller on its own machine that sends no credentials, and it answers 401
/// to any `Authorization` header that is not its key, so "no key" has to
/// mean "no header".
///
/// Its `Debug` prints the version alone: the key is a secret, and an
/// address can hold one.
#[derive(Deserialize)]
pub(crate) struct WebSearchResource {
    /// The entry's version, which is [`VERSION`] in every entry that
    /// [`WebSearchResource::read`] returns.
    version: u32,
    /// The address a search is posted to.
    url: String,
    /// The Gateway's key, when the Host has one.
    api_key: Option<String>,
}

impl WebSearchResource {
    /// Reads the entry out of `resources`, the JSON text of the Host's
    /// resources: an object keyed by resource name.
    ///
    /// It checks the entry's `version` first and by itself, so an entry of
    /// a later version fails as that, whatever else it holds.
    ///
    /// # Errors
    /// Returns a transport failure when the resources hold no entry, as
    /// they do under a Host with no web search and under one whose Gateway
    /// is down. Returns an other failure when the entry is not version 1,
    /// or does not have the shape of version 1. Neither message quotes the
    /// resources, which can hold the key.
    pub(crate) fn read(resources: &str) -> Result<WebSearchResource, ToolError> {
        // Text that is not the JSON text of an object holds no entry, as an
        // object without the entry's name holds none.
        let Ok(resources) = serde_json::from_str::<Value>(resources) else {
            return Err(no_entry());
        };
        let Some(entry) = resources.get(RESOURCE) else {
            return Err(no_entry());
        };
        if entry.get("version").and_then(Value::as_u64) != Some(u64::from(VERSION)) {
            return Err(not_version_1());
        }
        WebSearchResource::deserialize(entry).map_err(|_| not_version_1())
    }
}

impl fmt::Debug for WebSearchResource {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("WebSearchResource")
            .field("version", &self.version)
            .finish_non_exhaustive()
    }
}

/// The failure of a search under a Host whose resources hold no entry.
fn no_entry() -> ToolError {
    ToolError::new(
        ToolErrorKind::Transport,
        "web_search: the host offers no web search right now",
    )
}

/// The failure of a search whose entry is not one this client reads.
fn not_version_1() -> ToolError {
    ToolError::new(
        ToolErrorKind::Other,
        "web_search: the host's runvil/web-search entry is not version 1",
    )
}

/// The reply to a Gateway web search, mirroring the Gateway's
/// `POST /v1/tools/web_search` response. It is read from the Gateway's
/// reply, and it is written as the output of the search tool.
///
/// `query`, `results` and each result's `url` are required, so a reply
/// without them is a malformed response. Every other field defaults when
/// absent, and unknown fields are ignored so the Gateway can grow its reply.
///
/// Written, it is the Gateway's own serialization: the fields in the
/// Gateway's order, with a result's `age` and `site_name` only when
/// present, and its `extra_snippets` only when it holds at least one
/// snippet.
#[derive(Deserialize, Serialize)]
pub(crate) struct SearchResults {
    /// The query the Gateway ran, after its trimming.
    query: String,
    /// The result rows, in the Gateway's order.
    results: Vec<SearchResult>,
}

/// One row of [`SearchResults`].
///
/// The `url` is required, and [`SearchClient::search`] rejects the whole
/// reply when one is blank.
#[derive(Deserialize, Serialize)]
struct SearchResult {
    /// The result's title.
    #[serde(default)]
    title: String,
    /// The result's URL.
    url: String,
    /// A short description or snippet.
    #[serde(default)]
    description: String,
    /// The result's age, when the Gateway reports one.
    #[serde(skip_serializing_if = "Option::is_none")]
    age: Option<String>,
    /// The hostname of `url`, when the Gateway could derive one.
    #[serde(skip_serializing_if = "Option::is_none")]
    site_name: Option<String>,
    /// Extra snippets, when the Gateway sends any.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    extra_snippets: Vec<String>,
}

/// The HTTP client every search goes through, with the deadline of one
/// search.
///
/// It is built once, so every run's search tool shares one connection
/// pool. Each search is one `POST` to the address its
/// [`WebSearchResource`] names, under a 30-second deadline. The deadline
/// covers the whole exchange, including reading the reply body. The
/// search vendor's credential stays in the Gateway. This client sends
/// only the Gateway's key.
///
/// The client honors the system's proxy settings, which it reads once, as
/// it is built: the `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY`, and
/// `NO_PROXY` environment variables, and where those name no proxy, the
/// proxy settings of Windows and of macOS. A proxy that applies to the
/// Gateway's address receives the request, and the key in it.
///
/// The client sets no redirect policy, so it follows a redirect as its
/// HTTP library does: at most 10 of them, and without the key once one
/// leaves the scheme, the host, or the port of the Gateway's address.
#[derive(Clone, Debug)]
pub(crate) struct SearchClient {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The deadline of one search, body included.
    timeout: Duration,
}

impl SearchClient {
    /// Builds the client with the deadline every search has.
    ///
    /// # Errors
    /// Returns the HTTP client's own error when it cannot be built, which
    /// happens when the TLS backend fails to start.
    pub(crate) fn try_new() -> Result<SearchClient, reqwest::Error> {
        SearchClient::with_timeout(REQUEST_TIMEOUT)
    }

    /// Builds the client with `timeout` as the deadline of one search.
    ///
    /// It goes through the HTTP client's builder, because
    /// `reqwest::Client::new` panics where the builder returns an error.
    /// It names rustls as the TLS backend, as the fetch client does, so the
    /// client uses it even in a build that turns on another one. It sets
    /// no proxy and leaves the system's proxy settings on.
    fn with_timeout(timeout: Duration) -> Result<SearchClient, reqwest::Error> {
        let http = reqwest::Client::builder().tls_backend_rustls().build()?;
        Ok(SearchClient { http, timeout })
    }

    /// Runs one web search: posts `request` as JSON to the address
    /// `gateway` names, and returns the Gateway's parsed reply. `request`
    /// serializes as the Gateway's web search request, which the checked
    /// arguments of the search tool do.
    ///
    /// # Errors
    /// Returns a [`ToolError`] whose message a model reads. The Gateway's
    /// error body in that message is truncated to a size limit and its
    /// control characters are escaped, and this client never writes the
    /// key into a message. Its kind is:
    /// - `Transport` when sending the request fails
    ///   (`web_search: request failed`) or reading a successful reply
    ///   fails (`web_search: reading response failed`), including when the
    ///   deadline passes;
    /// - `Backend` when the Gateway answers a failure status
    ///   (`web_search: backend returned {code}: {body}`, or
    ///   `web_search: backend returned {code}, and its error body could
    ///   not be read`), or a success body that is over 256 KiB
    ///   (`web_search: response body exceeded {limit} bytes`), is invalid
    ///   UTF-8 (`web_search: response body was not valid UTF-8`), fails to
    ///   parse as a search reply (`web_search: malformed search
    ///   response`), or holds a result whose `url` is blank
    ///   (`web_search: malformed search response: result {index} has an
    ///   empty url`).
    pub(crate) async fn search(
        self,
        gateway: WebSearchResource,
        request: impl Serialize,
    ) -> Result<SearchResults, ToolError> {
        let WebSearchResource { url, api_key, .. } = gateway;
        let mut post = self.http.post(url).timeout(self.timeout).json(&request);
        // An empty key is no key: the Gateway answers 401 to an
        // `Authorization` header that holds anything but its key, an empty
        // bearer included.
        if let Some(key) = api_key.filter(|key| !key.is_empty()) {
            post = post.bearer_auth(key);
        }
        let response = post
            .send()
            .await
            .map_err(|_| ToolError::new(ToolErrorKind::Transport, "web_search: request failed"))?;

        let status = response.status();
        if !status.is_success() {
            let code = status.as_u16();
            // The error body is external gateway content: bound the read and
            // sanitize control characters.
            match read_bounded(response, MAX_ERROR_BODY).await {
                Some(body) => {
                    let body = if body.is_empty() {
                        "(empty body)".to_owned()
                    } else {
                        sanitize_diagnostic(&body)
                    };
                    return Err(ToolError::new(
                        ToolErrorKind::Backend,
                        format!("web_search: backend returned {code}: {body}"),
                    ));
                }
                None => {
                    return Err(ToolError::new(
                        ToolErrorKind::Backend,
                        format!(
                            "web_search: backend returned {code}, and its error body could not be read"
                        ),
                    ));
                }
            }
        }

        // Success bodies hold third-party content: bound them (rejecting cap
        // overflow), then parse the promised JSON shape.
        let body = read_capped(response, MAX_RESPONSE_BODY).await?;
        let results: SearchResults = serde_json::from_str(&body).map_err(|_| {
            ToolError::new(
                ToolErrorKind::Backend,
                "web_search: malformed search response",
            )
        })?;
        if let Some(index) = results
            .results
            .iter()
            .position(|result| result.url.trim().is_empty())
        {
            return Err(ToolError::new(
                ToolErrorKind::Backend,
                format!("web_search: malformed search response: result {index} has an empty url"),
            ));
        }
        Ok(results)
    }
}

/// Escapes control characters in an external diagnostic body so a hostile
/// gateway cannot inject terminal/log control sequences or forge multiline
/// records through an error `Display`.
fn sanitize_diagnostic(body: &str) -> String {
    let mut out = String::with_capacity(body.len());
    for c in body.chars() {
        match c {
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if c.is_control() => {
                use std::fmt::Write as _;
                let _ = write!(out, "\\u{{{:04x}}}", u32::from(c));
            }
            c => out.push(c),
        }
    }
    out
}

/// Reads at most `limit` bytes of a diagnostic body, stopping early once the cap
/// is reached. Used for the error path, where a truncated, lossy rendering is an
/// acceptable diagnostic. Returns `None` when the read fails.
async fn read_bounded(mut response: reqwest::Response, limit: usize) -> Option<String> {
    let mut buffer: Vec<u8> = Vec::new();
    while buffer.len() < limit {
        let chunk = response.chunk().await.ok()?;
        let Some(chunk) = chunk else { break };
        let take = (limit - buffer.len()).min(chunk.len());
        buffer.extend_from_slice(&chunk[..take]);
        if take < chunk.len() {
            break;
        }
    }
    Some(String::from_utf8_lossy(&buffer).into_owned())
}

/// Reads a success body, rejecting it once it would exceed `limit` bytes rather
/// than truncating (a truncated JSON document is not a valid result set), and
/// requiring valid UTF-8.
async fn read_capped(mut response: reqwest::Response, limit: usize) -> Result<String, ToolError> {
    let mut buffer: Vec<u8> = Vec::new();
    while let Some(chunk) = response.chunk().await.map_err(|_| {
        ToolError::new(
            ToolErrorKind::Transport,
            "web_search: reading response failed",
        )
    })? {
        if buffer.len() + chunk.len() > limit {
            return Err(ToolError::new(
                ToolErrorKind::Backend,
                format!("web_search: response body exceeded {limit} bytes"),
            ));
        }
        buffer.extend_from_slice(&chunk);
    }
    String::from_utf8(buffer).map_err(|_| {
        ToolError::new(
            ToolErrorKind::Backend,
            "web_search: response body was not valid UTF-8",
        )
    })
}

#[cfg(test)]
#[path = "gateway-tests.rs"]
mod tests;
