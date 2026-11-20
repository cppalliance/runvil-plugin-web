//! `plugin-web` - the web Plugin. It inserts one Pack, `runvil/web`,
//! which gives a run a tool that fetches a page from the web.
//!
//! The Pack's id is `web`, and it has one tool:
//!
//! | Lua id | Name the model calls |
//! |---|---|
//! | `web/fetch` | `web_fetch` |
//!
//! A script calls the tool by its Lua id, `tools.call("web/fetch", { url
//! = "https://example.com/" })`, and offers it to the model with
//! `tools.offer("web/fetch")`. The tool sets no wire name, so the Host
//! derives the name the model calls it by from the Lua id.
//!
//! The Pack has no prelude, so declaring it, as in `uses: [web]`, only
//! makes it required. It takes no configuration.
//!
//! `cargo build -p plugin-web --release` writes `plugin_web.dll` on
//! Windows, `libplugin_web.so` on Linux, and `libplugin_web.dylib` on
//! macOS to `target/release`; that one file is the library Plugin.
//!
//! # This build
//!
//! This build proves one thing: a library Plugin can link an HTTP client
//! with TLS and run it on a runtime of its own. `lib.rs` is the whole
//! crate. It declares none of the other files under `src/` as a module,
//! so they are not compiled, and neither are the files under
//! `crates/harness-gateway-client/src/`, which belong to no crate. Those
//! files name Harness crates, such as `runvil-plugin`, the Harness's
//! Rust Plugin API, and this repository depends on no Harness crate.
//!
//! A `fetch` call takes `{"url": ...}`, sends a GET, and answers the
//! response's status and the first 300 characters of its body, as
//! untrusted output. A status that reports a failure, such as 404, is an
//! output too. The call fails when its arguments name no URL that
//! parses, and when the request gets no response.
//!
//! The tool applies no fetch policy. It sends the request to whatever
//! address the URL names, and it follows redirects as its client does by
//! default. So this build is for the tests and for the example
//! `load-dll`, and it does not belong in a Host's Plugin directory.
//!
//! # Where a request runs
//!
//! The Host polls a tool call on a thread of its own, and that thread
//! has no context of the Plugin's tokio runtime. A library Plugin links
//! its own copy of tokio, so the Host's runtime is no help to it. The
//! HTTP client needs that context for its sockets and its timers, so a
//! call does not send its request itself. It spawns the request as a
//! task on the Plugin's runtime and awaits the task's handle, which
//! needs no context.
//!
//! While the request is in flight, each step of the call ends pending,
//! and the Host sends the next step. A call the Host abandons drops its
//! future, which aborts the task, and the connection closes with the
//! task.
//!
//! The runtime has two worker threads, and it starts more for blocking
//! work: a host name is resolved on one of those. Dropping the Plugin
//! instance drops the runtime, which waits for every one of its threads,
//! so an unload returns only when a lookup that is in flight has ended.
//!
//! ## Invariants
//!
//! - Depends on `runvil-plugin-sdk` and outside libraries alone, and
//!   writes no `unsafe`.
//! - The `dll` feature exports `runvil_plugin`. A Host that links the
//!   crate in turns the feature off and hands
//!   `runvil_plugin_sdk::entry::<Web>` to the Harness.
//! - The Plugin instance owns a tokio runtime with two worker threads,
//!   with I/O and time on, and every request runs as a task on it.
//!   Dropping the instance drops the runtime, which joins its threads,
//!   so an unload leaves no thread behind. Production code keeps nothing
//!   in a `static`.
//! - A call awaits the task of its request, and dropping the call aborts
//!   the task.
//! - The client has no proxy, no cookie store, no automatic `Referer`
//!   header, and no default credentials, so no request sends an ambient
//!   identity.
//! - `load` refuses when the runtime or the client cannot be built, and
//!   it never panics.

use std::error::Error;
use std::time::Duration;

use reqwest::{Client, RequestBuilder, Url};
use runvil_plugin_sdk::{
    BoxFuture, Pack, PackInfo, Plugin, Refusal, ToolCall, ToolError, ToolErrorKind, ToolInfo,
    ToolOutput,
};
use serde_json::{Value, json};
use tokio::runtime::{Handle, Runtime};
use tokio::task::JoinHandle;

/// The Pack's name and description. Its name is `runvil/web`, so its Pack
/// id is `web`, and a prompt reaches its tool as `web/fetch`.
const INFO: PackInfo = PackInfo::new("runvil/web").description("Fetch pages from the web");

/// The tool's name within the Pack, the last segment of its Lua id.
const FETCH: &str = "fetch";

/// The time a request has to open its connection.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(5);

/// The time a whole request has, from its start to the last byte a call
/// reads of its body.
const TIMEOUT: Duration = Duration::from_secs(20);

/// The `User-Agent` header of every request.
const USER_AGENT: &str = concat!("runvil-plugin-web/", env!("CARGO_PKG_VERSION"));

/// How many characters of a body a call answers.
const BODY_CHARS: usize = 300;

/// How many bytes of a body a call reads before it stops. A character is
/// at most four bytes of UTF-8, so this many bytes hold the first
/// [`BODY_CHARS`] characters of any body.
const BODY_BYTES: usize = 4 * BODY_CHARS;

/// How a `fetch` call ends: the status and the start of the body, or the
/// failure a model reads.
type FetchResult = Result<ToolOutput, ToolError>;

/// The Plugin instance the Host holds while the Plugin is loaded. It
/// holds what every run shares: the HTTP client and the runtime that
/// runs every request. Nothing outlives the instance.
#[derive(Debug)]
pub struct Web {
    /// The client every fetch goes through, built once, so every run
    /// shares one connection pool.
    client: Client,
    /// Runs every request: a call spawns its request here and awaits the
    /// task. Two worker threads, with I/O and time on. Dropping the
    /// instance drops it, which joins its threads, so an unload leaves no
    /// thread behind.
    requests: Runtime,
}

impl Plugin for Web {
    /// The name the Plugin reports.
    const NAME: &'static str = "web";
    /// This crate's version.
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const PACKS: &'static [PackInfo] = &[INFO];

    /// Starts the runtime and builds the client. A thread the system
    /// will not start refuses the load, and so does a client that cannot
    /// be built, which happens when the TLS backend fails to start.
    fn load() -> Result<Web, Refusal> {
        let requests = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .thread_name("plugin-web-requests")
            .enable_io()
            .enable_time()
            .build()
            .map_err(|error| {
                Refusal::new(format!(
                    "web could not start the threads that run its requests: {error}"
                ))
            })?;
        let client = Client::builder()
            .tls_backend_rustls()
            .no_proxy()
            .referer(false)
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(TIMEOUT)
            .user_agent(USER_AGENT)
            .build()
            .map_err(|error| {
                Refusal::new(format!(
                    "web could not build its HTTP client: {}",
                    chain(&error)
                ))
            })?;
        Ok(Web { client, requests })
    }

    fn new_pack(&self, _pack: &str) -> Result<Box<dyn Pack>, Refusal> {
        Ok(Box::new(WebPack {
            client: self.client.clone(),
            requests: self.requests.handle().clone(),
        }))
    }
}

#[cfg(feature = "dll")]
runvil_plugin_sdk::export_plugin!(Web);

/// One run's instance of the Pack. It holds only what every run shares:
/// a clone of the client, which shares the one connection pool, and the
/// handle of the Plugin's runtime.
struct WebPack {
    client: Client,
    /// The Plugin's runtime, which runs every request.
    requests: Handle,
}

impl Pack for WebPack {
    fn tools(&self) -> Vec<ToolInfo> {
        let parameters = json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                }
            },
            "required": ["url"]
        });
        vec![ToolInfo::new(
            FETCH,
            "Fetch a web page and return its HTTP status and the start of its body.",
            parameters.to_string(),
        )]
    }

    fn call<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            if call.tool() != FETCH {
                return Err(ToolError::new(
                    ToolErrorKind::Other,
                    format!("web has no tool named {}", call.tool()),
                ));
            }
            self.fetch(call.arguments()).await
        })
    }
}

impl WebPack {
    /// Fetches the URL that `arguments` names: spawns the request on the
    /// Plugin's runtime and awaits the task. Dropping the returned future
    /// aborts the task.
    async fn fetch(&self, arguments: &str) -> FetchResult {
        let url = url_argument(arguments)?;
        let mut task = AbortOnDrop(self.requests.spawn(get(self.client.get(url))));
        match (&mut task.0).await {
            Ok(result) => result,
            Err(_) => Err(ToolError::new(
                ToolErrorKind::Other,
                "web_fetch: the fetch task did not finish",
            )),
        }
    }
}

/// The task of a request, aborted when the call that awaits it is
/// dropped.
struct AbortOnDrop(JoinHandle<FetchResult>);

impl Drop for AbortOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// The URL that `arguments`, the JSON text of a call's arguments, names
/// under `url`.
fn url_argument(arguments: &str) -> Result<Url, ToolError> {
    let url = serde_json::from_str::<Value>(arguments)
        .ok()
        .and_then(|arguments| arguments.get("url")?.as_str().map(str::to_owned))
        .ok_or_else(|| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "web_fetch: missing url argument",
            )
        })?;
    Url::parse(&url).map_err(|error| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            format!("web_fetch: the url {url} is not valid: {error}"),
        )
    })
}

/// Sends `request` and answers the response's status and the first
/// [`BODY_CHARS`] characters of its body, read as UTF-8. It runs as a
/// task on the Plugin's runtime, whose context the client needs.
async fn get(request: RequestBuilder) -> FetchResult {
    let mut response = request.send().await.map_err(|error| failed(&error))?;
    let status = response.status().as_u16();
    let mut body = Vec::new();
    while body.len() < BODY_BYTES
        && let Some(chunk) = response.chunk().await.map_err(|error| failed(&error))?
    {
        body.extend_from_slice(&chunk);
    }
    let text: String = String::from_utf8_lossy(&body)
        .chars()
        .take(BODY_CHARS)
        .collect();
    Ok(ToolOutput::untrusted(format!("status: {status}\n\n{text}")))
}

/// The failure of a request that got no response, or lost it midway.
fn failed(error: &reqwest::Error) -> ToolError {
    ToolError::new(
        ToolErrorKind::Transport,
        format!("web_fetch: the request failed: {}", chain(error)),
    )
}

/// The text of `error`, and after it the text of each of its causes that
/// adds something. The client keeps the reason for a failure, such as a
/// host name that did not resolve, in the causes of its error.
fn chain(error: &dyn Error) -> String {
    let mut text = error.to_string();
    let mut cause = error.source();
    while let Some(error) = cause {
        let reason = error.to_string();
        if !text.ends_with(&reason) {
            text.push_str(": ");
            text.push_str(&reason);
        }
        cause = error.source();
    }
    text
}

#[cfg(test)]
mod tests {
    use std::io::{Read, Write};
    use std::net::{TcpListener, TcpStream};
    use std::sync::mpsc::{self, Receiver};
    use std::thread;
    use std::time::Duration;

    use runvil_plugin_sdk::sys::RV_OK;
    use runvil_plugin_sdk::testing::{self, Loaded, PackRun};
    use runvil_plugin_sdk::{ToolError, ToolErrorKind, ToolOutput};
    use serde_json::{Value, json};

    use super::{BODY_CHARS, Web};

    /// The name the Plugin inserts its Pack under.
    const PACK: &str = "runvil/web";

    /// The Plugin, loaded as the Harness loads it.
    fn loaded() -> Loaded {
        testing::load::<Web>().expect("the Plugin loads")
    }

    /// Runs one `fetch` call for `url` on `pack` to its end, as the
    /// Harness does.
    fn fetch(pack: &PackRun<'_>, url: &str) -> Result<ToolOutput, ToolError> {
        pack.call("fetch", &json!({ "url": url }).to_string())
            .expect("the call follows the contract")
    }

    /// What a served connection does once the request has arrived.
    enum Reply {
        /// Answers with this status and this body, then closes.
        Page(&'static str, String),
        /// Closes without an answer.
        Nothing,
        /// Stays open until the client closes the connection.
        Hold,
    }

    /// What a served connection reports.
    #[derive(Debug, PartialEq, Eq)]
    enum Event {
        /// The request has arrived.
        Requested,
        /// The client has closed a connection that was held open.
        Closed,
    }

    /// Serves one connection on a loopback port, on a thread of its own.
    /// Returns the URL of that port, and what the connection reports.
    fn serve(reply: Reply) -> (String, Receiver<Event>) {
        let listener = TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
        let port = listener
            .local_addr()
            .expect("the listener has an address")
            .port();
        let (report, events) = mpsc::channel();
        thread::spawn(move || {
            let (mut socket, _) = listener.accept().expect("the fetch connects");
            read_request(&mut socket);
            let _ = report.send(Event::Requested);
            match reply {
                Reply::Page(status, body) => {
                    let head = format!(
                        "HTTP/1.1 {status}\r\nContent-Type: text/plain; charset=utf-8\r\n\
                         Content-Length: {}\r\nConnection: close\r\n\r\n",
                        body.len()
                    );
                    let _ = socket.write_all(head.as_bytes());
                    let _ = socket.write_all(body.as_bytes());
                }
                Reply::Nothing => {}
                Reply::Hold => {
                    let mut buffer = [0; 1024];
                    while socket.read(&mut buffer).is_ok_and(|read| read > 0) {}
                    let _ = report.send(Event::Closed);
                }
            }
        });
        (format!("http://127.0.0.1:{port}/"), events)
    }

    /// Reads `socket` to the blank line that ends the head of a request.
    fn read_request(socket: &mut TcpStream) {
        let mut request = Vec::new();
        let mut buffer = [0; 1024];
        while !request.windows(4).any(|window| window == b"\r\n\r\n") {
            let read = socket.read(&mut buffer).expect("the request arrives");
            assert!(read > 0, "the connection closed before the request ended");
            request.extend_from_slice(&buffer[..read]);
        }
    }

    #[test]
    fn the_plugin_loads_as_web_at_its_version_and_unloads() {
        let loaded = loaded();
        assert_eq!(loaded.name(), "web");
        assert_eq!(loaded.version(), env!("CARGO_PKG_VERSION"));
        assert_eq!(loaded.unload(), RV_OK);
    }

    #[test]
    fn the_plugin_inserts_the_one_pack_runvil_web_with_the_one_tool_fetch() {
        let loaded = loaded();
        let packs: Vec<(&str, &str)> = loaded
            .packs()
            .iter()
            .map(|pack| (pack.name.as_str(), pack.description.as_str()))
            .collect();
        assert_eq!(packs, [(PACK, "Fetch pages from the web")]);

        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        let tools: Vec<(&str, Option<&str>, bool)> = pack
            .tools()
            .iter()
            .map(|tool| {
                (
                    tool.name.as_str(),
                    tool.wire_name.as_deref(),
                    tool.changes_files,
                )
            })
            .collect();
        assert_eq!(tools, [("fetch", None, false)]);
        let schema: Value =
            serde_json::from_str(&pack.tools()[0].parameters).expect("the schema is JSON");
        assert_eq!(schema["required"], json!(["url"]));
        assert_eq!(schema["properties"]["url"]["type"], "string");
    }

    #[test]
    fn a_fetch_answers_the_status_and_the_start_of_the_body_as_untrusted_output() {
        // A body of two-byte characters, longer than a call reads.
        let (url, _events) = serve(Reply::Page("200 OK", "é".repeat(4 * BODY_CHARS)));
        let loaded = loaded();
        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        // The test has no tokio runtime of its own, so the request can
        // only have run on the Plugin's.
        assert_eq!(
            fetch(&pack, &url),
            Ok(ToolOutput::untrusted(format!(
                "status: 200\n\n{}",
                "é".repeat(BODY_CHARS)
            )))
        );
    }

    #[test]
    fn a_status_that_reports_a_failure_is_an_output_too() {
        let (url, _events) = serve(Reply::Page("404 Not Found", "no such page".to_owned()));
        let loaded = loaded();
        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        assert_eq!(
            fetch(&pack, &url),
            Ok(ToolOutput::untrusted("status: 404\n\nno such page"))
        );
    }

    #[test]
    fn a_call_that_names_no_url_fails_as_invalid_arguments() {
        let loaded = loaded();
        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        for arguments in ["{}", r#"{"url":7}"#] {
            assert_eq!(
                pack.call("fetch", arguments),
                Ok(Err(ToolError::new(
                    ToolErrorKind::InvalidArguments,
                    "web_fetch: missing url argument"
                ))),
                "{arguments}"
            );
        }

        let error = fetch(&pack, "not a url").expect_err("the call fails");
        assert_eq!(error.kind(), ToolErrorKind::InvalidArguments);
        assert!(
            error
                .message()
                .starts_with("web_fetch: the url not a url is not valid: "),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_request_that_gets_no_response_fails_as_a_transport_failure() {
        let (url, _events) = serve(Reply::Nothing);
        let loaded = loaded();
        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        let error = fetch(&pack, &url).expect_err("the fetch fails");
        assert_eq!(error.kind(), ToolErrorKind::Transport);
        assert!(
            error
                .message()
                .starts_with("web_fetch: the request failed: "),
            "{}",
            error.message()
        );
    }

    #[test]
    fn a_call_that_is_dropped_aborts_its_request_and_the_plugin_unloads() {
        let (url, events) = serve(Reply::Hold);
        let loaded = loaded();
        {
            let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
            let mut call = pack.start("fetch", &json!({ "url": url }).to_string());
            // The call is stepped until its request has reached the
            // server, which never answers it.
            while events.try_recv() != Ok(Event::Requested) {
                assert_eq!(call.step(), Ok(None), "the held request does not finish");
                thread::sleep(Duration::from_millis(1));
            }
            drop(call);
            // Only the abort of the request's task closes the connection
            // before the client's timeout does.
            assert_eq!(
                events.recv_timeout(Duration::from_secs(5)),
                Ok(Event::Closed),
                "dropping the call aborts its request, which closes the connection"
            );
        }
        assert_eq!(loaded.unload(), RV_OK);
    }
}
