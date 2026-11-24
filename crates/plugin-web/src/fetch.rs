//! The `web_fetch` tool: [`FetchClient`], the hardened HTTP client and the
//! policy it enforces, built once, and [`WebFetch`], that client bound to
//! the Plugin's runtime, whose [`call`](WebFetch::call) performs one fetch.
//!
//! `FetchClient` composes the URL-admission policy, the guarded DNS resolver,
//! the per-hop redirect policy, and the bounded body reads into one safe
//! fetch. The client is built with no ambient proxy, no automatic `Referer`,
//! no cookie store, and no default credentials, so no request includes an
//! ambient identity on any hop. `WebFetch` spawns each fetch onto the Plugin's
//! runtime and aborts it when the call is dropped.
//!
//! The Host polls a call on a thread of its own, which has no context of
//! the Plugin's runtime, and the client needs that context for its sockets,
//! its timers, and its host lookups. So a call sends no request itself: it
//! runs the fetch as a task on the runtime, through [`task::run`], and
//! awaits the task, which needs no context and wakes the call when it
//! ends. The task renders an HTML page on a thread the runtime starts for
//! blocking work, so that the rendering, which only computes, holds no
//! worker thread.

use std::sync::Arc;

use reqwest::header::CONTENT_TYPE;
use runvil_plugin_sdk::{ToolError, ToolErrorKind, ToolOutput};
use tokio::runtime::Handle;
use url::Url;

use crate::config::{ConfigError, FetchConfig};
use crate::error::{Disposition, FetchError, SafeUrl};
use crate::redirect::redirect_policy;
use crate::resolver::GuardedResolver;
use crate::response::{
    Extraction, Route, classify, decode_body, extract_html, read_body_capped, read_body_truncating,
    truncate_to_chars,
};
use crate::task;
use crate::url_policy::check_url;

/// The result [`WebFetch::call`] returns: untrusted page text on success,
/// a narrow [`ToolError`] on a hard failure.
type CallResult = Result<ToolOutput, ToolError>;

/// The hardened HTTP client the fetch tool sends every request through,
/// with the validated [`FetchConfig`] policy it enforces.
///
/// It is built once, so every run's fetch tool shares one connection pool.
#[derive(Debug, Clone)]
pub(crate) struct FetchClient {
    /// The HTTP client used for outbound requests.
    http: reqwest::Client,
    /// The validated security policy applied to each fetch.
    config: Arc<FetchConfig>,
}

/// Builds the hardened HTTP client for `config`.
///
/// Installs the [`GuardedResolver`] so every connection is made only to an
/// address the policy allows, and the per-hop [`redirect_policy`]. Disables
/// ambient proxies (`no_proxy`) and automatic `Referer` (`referer(false)`), and
/// sets no cookie store and no default headers, so no request sends an ambient
/// identity on any hop, including after a redirect. Names rustls as the TLS
/// backend, so the client uses it even in a build that turns on another one.
fn build_client(config: &Arc<FetchConfig>) -> Result<reqwest::Client, reqwest::Error> {
    let resolver = Arc::new(GuardedResolver::system(Arc::clone(config)));
    reqwest::Client::builder()
        .tls_backend_rustls()
        .dns_resolver(resolver)
        .redirect(redirect_policy((**config).clone()))
        .no_proxy()
        .referer(false)
        .connect_timeout(config.connect_timeout())
        .timeout(config.timeout())
        .pool_idle_timeout(config.pool_idle_timeout())
        .user_agent(config.user_agent())
        .build()
}

impl FetchClient {
    /// Builds the client over a validated policy.
    ///
    /// # Errors
    /// Returns [`ConfigError`] if the HTTP client cannot be built for `config`
    /// (for example a TLS backend that fails to initialize). The policy itself
    /// is already validated by [`FetchConfig`] construction, so no policy field
    /// can trigger a failure here.
    pub(crate) fn try_with_config(config: FetchConfig) -> Result<FetchClient, ConfigError> {
        let config = Arc::new(config);
        let http = build_client(&config).map_err(ConfigError::client_build)?;
        Ok(FetchClient { http, config })
    }

    /// Builds the client over an injected [`Lookup`] for tests.
    ///
    /// [`Lookup`]: crate::resolver::Lookup
    #[cfg(test)]
    fn with_lookup<L: crate::resolver::Lookup>(config: FetchConfig, lookup: L) -> FetchClient {
        let config = Arc::new(config);
        let resolver = Arc::new(GuardedResolver::new(lookup, Arc::clone(&config)));
        let http = reqwest::Client::builder()
            .tls_backend_rustls()
            .dns_resolver(resolver)
            .redirect(redirect_policy((*config).clone()))
            .no_proxy()
            .referer(false)
            .connect_timeout(config.connect_timeout())
            .timeout(config.timeout())
            .pool_idle_timeout(config.pool_idle_timeout())
            .user_agent(config.user_agent())
            .build()
            .expect("the test client builds with a valid policy");
        FetchClient { http, config }
    }

    /// The fetch tool over this client, spawning every fetch onto `runtime`.
    pub(crate) fn tool(&self, runtime: Handle) -> WebFetch {
        WebFetch {
            client: self.clone(),
            runtime,
        }
    }

    /// Sends the GET for an admitted `url` and renders its response: the
    /// request, the body read, and the extraction a call spawns as one task.
    /// The task hands the rendering of an HTML page to a blocking thread and
    /// awaits it there.
    async fn fetch(self, url: Url, max_chars: usize, raw: bool) -> CallResult {
        let response = match self.http.get(url.clone()).send().await {
            Ok(resp) => resp,
            Err(err) => return map_send_error_to_outcome(&err, url.as_str()),
        };

        let final_url = response.url().clone();

        let status = response.status();
        if !status.is_success() {
            let err = FetchError::HttpStatus {
                url: SafeUrl::new(final_url.as_str()),
                status: status.as_u16(),
            };
            return Ok(ToolOutput::untrusted(err.model_facing()));
        }

        let content_type = response
            .headers()
            .get(CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned);

        let Some(content_type) = content_type else {
            return soft_or_hard(&FetchError::NoContentType {
                url: SafeUrl::new(final_url.as_str()),
            });
        };

        let parsed_mime: mime::Mime = match content_type.parse() {
            Ok(m) => m,
            Err(_) => {
                return soft_or_hard(&FetchError::UnsupportedContentType {
                    url: SafeUrl::new(final_url.as_str()),
                    content_type: content_type.clone(),
                });
            }
        };

        let Some(route) = classify(&parsed_mime) else {
            return soft_or_hard(&FetchError::UnsupportedContentType {
                url: SafeUrl::new(final_url.as_str()),
                content_type: content_type.clone(),
            });
        };

        let charset = parsed_mime
            .get_param(mime::CHARSET)
            .map(|name| name.as_str().to_owned());

        let max_bytes = self.config.max_bytes();
        let (decoded, extraction, size_truncated) = match route {
            Route::Html => {
                let body = match read_body_capped(response, final_url.as_str(), max_bytes).await {
                    Ok(b) => b,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                let base_url = final_url.clone();
                let rendering =
                    on_blocking_thread(move || extract_html(&decoded, Some(&base_url), raw));
                let Some((markdown, extraction)) = rendering.await else {
                    return Err(did_not_finish());
                };
                (markdown, extraction, false)
            }
            Route::Plain { structured: true } => {
                let body = match read_body_capped(response, final_url.as_str(), max_bytes).await {
                    Ok(b) => b,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                (decoded, Extraction::Plain, false)
            }
            Route::Plain { structured: false } => {
                let (body, size_truncated) = match read_body_truncating(response, max_bytes).await {
                    Ok(v) => v,
                    Err(e) => return Ok(body_read_outcome(&e)),
                };
                let decoded = match decode_body(&body, charset.as_deref(), final_url.as_str()) {
                    Ok(d) => d,
                    Err(e) => return soft_or_hard(&e),
                };
                (decoded, Extraction::Plain, size_truncated)
            }
        };

        let (text, char_truncated) = truncate_to_chars(&decoded, max_chars);
        let truncated = size_truncated || char_truncated;

        Ok(ToolOutput::untrusted(format!(
            "url: {final_url}\ntruncated: {truncated}\nextraction: {}\n\n{text}",
            extraction.label()
        )))
    }
}

/// A tool that fetches a web page and returns its main content as markdown.
///
/// It holds the shared [`FetchClient`] and the handle of the Plugin's runtime,
/// which every fetch is spawned onto.
#[derive(Debug, Clone)]
pub(crate) struct WebFetch {
    /// The client and policy every fetch goes through.
    client: FetchClient,
    /// The Plugin's runtime, which runs each fetch.
    runtime: Handle,
}

/// Runs `work` on a thread that the runtime starts for blocking work, and
/// returns what it returns, or `None` when it panicked or the runtime shut
/// down before it ran.
///
/// A fetch task hands over work that only computes, which is the rendering
/// of an HTML page: a large page takes seconds of it. The runtime's worker
/// threads drive the sockets and the timers of every call, so such work on
/// a worker would stall every other fetch. Work that has started cannot be
/// aborted: it runs to its end on its thread, even when the task that
/// awaits it is dropped.
async fn on_blocking_thread<T: Send + 'static>(
    work: impl FnOnce() -> T + Send + 'static,
) -> Option<T> {
    tokio::task::spawn_blocking(work).await.ok()
}

/// The failure of a call whose fetch did not come to its end: the fetch
/// task, or the rendering of its page, panicked or was cancelled.
fn did_not_finish() -> ToolError {
    ToolError::new(
        ToolErrorKind::Other,
        "web_fetch: the fetch task did not finish",
    )
}

/// Maps a [`FetchError`] to a soft tool output or a hard `Err` by its
/// [`Disposition`].
fn soft_or_hard(err: &FetchError) -> CallResult {
    match err.classify() {
        Disposition::SoftOutput => Ok(ToolOutput::untrusted(err.model_facing())),
        Disposition::Hard(kind) => Err(ToolError::new(kind, err.model_facing())),
    }
}

/// Maps a body-read [`FetchError`] into soft, untrusted tool text.
///
/// A body-read failure is a size cap ([`FetchError::TooLarge`]) or a mid-stream
/// transport failure ([`FetchError::BodyRead`]); both are soft and returned as
/// model-facing tool text so the model can try a different URL.
fn body_read_outcome(err: &FetchError) -> ToolOutput {
    ToolOutput::untrusted(err.model_facing())
}

/// Maps a reqwest send error into either a soft tool result or a hard `Err`.
///
/// A refusal produced by the resolver or redirect policy appears as a
/// [`FetchError`] in the error source chain; its [`Disposition`] determines
/// the outcome. A bare transport failure with no such source is soft.
fn map_send_error_to_outcome(err: &reqwest::Error, url: &str) -> CallResult {
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(err);
    while let Some(current) = source {
        if let Some(fetch_err) = current.downcast_ref::<FetchError>() {
            return match fetch_err.classify() {
                Disposition::SoftOutput => Ok(ToolOutput::untrusted(fetch_err.model_facing())),
                Disposition::Hard(kind) => Err(ToolError::new(kind, fetch_err.model_facing())),
            };
        }
        source = current.source();
    }
    if err.is_timeout() {
        return Ok(ToolOutput::untrusted(
            FetchError::Timeout {
                url: SafeUrl::new(url),
            }
            .model_facing(),
        ));
    }
    Ok(ToolOutput::untrusted(format!(
        "fetch failed for {url}: network error; try a different URL"
    )))
}

impl WebFetch {
    /// The one sentence the model reads to decide when to fetch.
    #[expect(
        clippy::unused_self,
        reason = "the description is read through the tool, as the schema is, which reads the tool's policy"
    )]
    pub(crate) fn description(&self) -> &'static str {
        "Fetch a web page and return its main content as markdown."
    }

    /// The JSON Schema for a fetch's arguments, with `max_chars` capped at
    /// the policy's ceiling.
    pub(crate) fn parameters_schema(&self) -> serde_json::Value {
        let ceiling = self.client.config.max_chars();
        serde_json::json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": ceiling,
                    "description": "Maximum number of characters of text to return for this call. Clamped to the configured ceiling. Longer text is truncated on a character boundary and the result is flagged as truncated."
                },
                "raw": {
                    "type": "boolean",
                    "description": "Skip article extraction and render the whole HTML document. Use for a page that is mostly a table or list, where extraction would discard the content. Ignored for non-HTML responses. Defaults to false."
                }
            },
            "required": ["url"]
        })
    }

    /// Fetches the URL `args` names on the Plugin's runtime and renders its
    /// response as untrusted text. Dropping the returned future aborts the
    /// fetch.
    ///
    /// # Errors
    /// Returns a [`ToolError`] for invalid arguments, a URL the policy
    /// refuses outright, or a fetch task that did not finish. A page the
    /// fetch could not render is a soft, untrusted output instead.
    pub(crate) async fn call(&self, args: serde_json::Value) -> CallResult {
        let url = args
            .get("url")
            .and_then(serde_json::Value::as_str)
            .ok_or_else(|| {
                ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "web_fetch: missing url argument",
                )
            })?;

        // A per-call `max_chars` is clamped to the configured ceiling; absent,
        // the ceiling itself applies.
        let max_chars = parse_max_chars(&args, self.client.config.max_chars())?;

        let raw = parse_raw(&args)?;

        let url = match check_url(url, &self.client.config) {
            Ok(u) => u,
            Err(err) => return soft_or_hard(&err),
        };

        // The task is all the call awaits, so the call is parked until the
        // task ends, which wakes it.
        let fetch = self.client.clone().fetch(url, max_chars, raw);
        match task::run(&self.runtime, fetch).await {
            Some(result) => result,
            None => Err(did_not_finish()),
        }
    }
}

/// Parses the optional `max_chars` argument, clamped to `ceiling`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `max_chars` is present but is
/// not a positive integer.
fn parse_max_chars(args: &serde_json::Value, ceiling: usize) -> Result<usize, ToolError> {
    let Some(value) = args.get("max_chars") else {
        return Ok(ceiling);
    };
    if value.is_null() {
        return Ok(ceiling);
    }
    let n = value.as_u64().filter(|n| *n >= 1).ok_or_else(|| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            "web_fetch: max_chars must be a positive integer",
        )
    })?;
    let requested = usize::try_from(n).unwrap_or(usize::MAX);
    Ok(requested.min(ceiling))
}

/// Parses the optional `raw` argument, defaulting to `false`.
///
/// # Errors
/// Returns an invalid-arguments [`ToolError`] if `raw` is present and is
/// neither null nor a boolean.
fn parse_raw(args: &serde_json::Value) -> Result<bool, ToolError> {
    match args.get("raw") {
        None => Ok(false),
        Some(value) if value.is_null() => Ok(false),
        Some(value) => value.as_bool().ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "web_fetch: raw must be a boolean",
            )
        }),
    }
}

#[cfg(test)]
mod tests;
