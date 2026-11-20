//! Tests for the Plugin and its Pack: the Plugin loads and names itself,
//! the causes a refused load names, the Pack it inserts, the two tools an
//! instance lists and the fetch tool's schema, the calls the fetch tool
//! refuses before any request is sent, a fetch from a server on this
//! machine, the abort of a dropped call's fetch, and the threads the
//! instance leaves behind when it is dropped, which are none. The tests of
//! the search tool sit in the child module and share these fixtures.
//!
//! The tests of the Pack and its calls go through the SDK's fake Host.
//! Every test is a plain `#[test]` with no tokio runtime of its own. So a
//! request can only have run on the Plugin's runtime, and the Plugin is
//! dropped outside an async context, as a Host drops it. A server that a
//! test sends a request to runs on a thread or a runtime the test owns.

use std::cell::RefCell;
use std::io::{Read, Write};
use std::net::{IpAddr, Ipv4Addr, TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::mpsc::{self, Receiver};
use std::thread;
use std::time::Duration;

use runvil_plugin_sdk::sys::RV_OK;
use runvil_plugin_sdk::testing::{self, Loaded, PackRun};
use runvil_plugin_sdk::{Pack, PackInfo, Plugin, Refusal, ToolError, ToolErrorKind, ToolOutput};
use serde_json::{Value, json};

use super::{Web, chain};
use crate::config::FetchConfig;

/// The name the Plugin inserts its Pack under.
const PACK: &str = "runvil/web";

/// How long a test waits for a server or a thread to report. A report
/// that is coming comes far sooner. It is shorter than the 20 seconds a
/// fetch has under either policy and the 30 seconds a search has, so a
/// connection that closes within it was not closed by that timeout.
const WAIT: Duration = Duration::from_secs(10);

/// The ceiling on returned text under [`loopback_policy`]. It differs
/// from the default policy's 40,000, so a schema shows which policy its
/// instance enforces.
const LOOPBACK_MAX_CHARS: usize = 10_000;

/// A page whose article is long enough for extraction to keep it.
const ARTICLE: &str = r"
    <html><body>
      <article>
        <h1>Loopback Test Article</h1>
        <p>This is the first substantial paragraph of a loopback test page,
           deliberately long enough to be treated as real article content.</p>
        <p>A second paragraph continues the prose so the extractor keeps the
           body and the character count stays comfortably above threshold.</p>
      </article>
    </body></html>
";

/// The Plugin, loaded as the Harness loads it.
fn loaded() -> Loaded {
    testing::load::<Web>().expect("the Plugin loads")
}

/// The web Plugin over [`loopback_policy`] in place of the default
/// policy, so that a test can fetch from a server on this machine. Only a
/// test can make one: the Plugin a Host loads is [`Web`], over the default
/// policy.
struct Loopback(Web);

impl Plugin for Loopback {
    const NAME: &'static str = Web::NAME;
    const VERSION: &'static str = Web::VERSION;
    const PACKS: &'static [PackInfo] = Web::PACKS;

    fn load() -> Result<Loopback, Refusal> {
        Web::with_fetch_config(loopback_policy()).map(Loopback)
    }

    fn new_pack(&self, pack: &str) -> Result<Box<dyn Pack>, Refusal> {
        self.0.new_pack(pack)
    }
}

/// A policy that reaches a server on this machine: it allows plain
/// `http`, and `localhost` at the loopback address. It allows every port,
/// because the system picks the port of a test's server, and a load takes
/// no argument that could name it.
fn loopback_policy() -> FetchConfig {
    FetchConfig::builder()
        .allow_http(true)
        .allow_ports(1..=u16::MAX)
        .allow_host_address("localhost", IpAddr::V4(Ipv4Addr::LOCALHOST))
        .max_chars(LOOPBACK_MAX_CHARS)
        .build()
        .expect("the loopback policy is valid")
}

/// The Plugin over [`loopback_policy`], loaded as the Harness loads it.
fn loaded_for_loopback() -> Loaded {
    testing::load::<Loopback>().expect("the Plugin loads")
}

/// Runs one `fetch` call for `url` on `pack` to its end, as the Harness
/// does.
fn fetch(pack: &PackRun<'_>, url: &str) -> Result<ToolOutput, ToolError> {
    pack.call("fetch", &json!({ "url": url }).to_string())
        .expect("the call follows the contract")
}

/// The parameter schema of the tool named `tool` that `pack` lists.
fn schema(pack: &PackRun<'_>, tool: &str) -> Value {
    let listed = pack
        .tools()
        .iter()
        .find(|listed| listed.name == tool)
        .unwrap_or_else(|| panic!("the Pack lists a tool named {tool}"));
    serde_json::from_str(&listed.parameters).expect("the schema is JSON")
}

/// What a served connection does once the request has arrived.
enum Reply {
    /// Answers an HTML page with this status and this body, then closes.
    Page(&'static str, &'static str),
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
/// Returns the URL of that port under the host name `localhost`, which
/// [`loopback_policy`] allows, and what the connection reports.
fn serve(reply: Reply) -> (String, Receiver<Event>) {
    let (port, events) = serve_on_a_port(reply);
    (format!("http://localhost:{port}/"), events)
}

/// Serves one connection on a loopback port, on a thread of its own.
/// Returns that port and what the connection reports.
fn serve_on_a_port(reply: Reply) -> (u16, Receiver<Event>) {
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
                    "HTTP/1.1 {status}\r\nContent-Type: text/html; charset=utf-8\r\n\
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
    (port, events)
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

/// A cause, as an error keeps one behind its own text.
#[derive(Debug, thiserror::Error)]
#[error("the TLS backend did not start")]
struct Cause;

/// An error whose text holds none of its cause's.
#[derive(Debug, thiserror::Error)]
#[error("the client was not built")]
struct Silent(#[source] Cause);

/// An error whose text ends with its cause's already.
#[derive(Debug, thiserror::Error)]
#[error("the client was not built: {0}")]
struct Echoing(#[source] Cause);

/// A refused load's reason ends with the text of [`chain`], which is all
/// a Host learns of why the fetch client was not built.
#[test]
fn the_reason_of_a_refused_load_names_each_cause_of_its_error_once() {
    let reason = "the client was not built: the TLS backend did not start";
    assert_eq!(chain(&Silent(Cause)), reason);
    assert_eq!(chain(&Echoing(Cause)), reason);
}

#[test]
fn the_plugin_inserts_the_one_pack_runvil_web() {
    let loaded = loaded();
    let packs: Vec<(&str, &str)> = loaded
        .packs()
        .iter()
        .map(|pack| (pack.name.as_str(), pack.description.as_str()))
        .collect();
    assert_eq!(packs, [(PACK, "Search the web and fetch pages")]);
}

#[test]
fn an_instance_lists_fetch_and_search_under_the_names_the_host_derives() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    let tools: Vec<(&str, Option<&str>, &str, bool)> = pack
        .tools()
        .iter()
        .map(|tool| {
            (
                tool.name.as_str(),
                tool.wire_name.as_deref(),
                tool.description.as_str(),
                tool.changes_files,
            )
        })
        .collect();
    // No wire name is set, so the Host derives `web_fetch` and
    // `web_search` from the tools' ids, `web/fetch` and `web/search`.
    assert_eq!(
        tools,
        [
            (
                "fetch",
                None,
                "Fetch a web page and return its main content as markdown.",
                false
            ),
            (
                "search",
                None,
                "Search the web and return a list of results (title, url, description).",
                false
            )
        ]
    );
}

/// The whole schema, word for word, because a model reads every
/// description in it.
#[test]
fn the_fetch_tools_schema_is_the_one_a_model_reads_under_the_default_policy() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        schema(&pack, "fetch"),
        json!({
            "type": "object",
            "properties": {
                "url": {
                    "type": "string",
                    "description": "The URL to fetch."
                },
                "max_chars": {
                    "type": "integer",
                    "minimum": 1,
                    "maximum": 40_000,
                    "description": "Maximum number of characters of text to return for this call. Clamped to the configured ceiling. Longer text is truncated on a character boundary and the result is flagged as truncated."
                },
                "raw": {
                    "type": "boolean",
                    "description": "Skip article extraction and render the whole HTML document. Use for a page that is mostly a table or list, where extraction would discard the content. Ignored for non-HTML responses. Defaults to false."
                }
            },
            "required": ["url"]
        })
    );
}

#[test]
fn a_custom_fetch_policy_reaches_the_fetch_tools_schema() {
    let loaded = loaded_for_loopback();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        schema(&pack, "fetch")["properties"]["max_chars"]["maximum"],
        LOOPBACK_MAX_CHARS,
        "the fetch tool enforces the custom policy"
    );
}

/// The arguments and the URLs that the default policy refuses before the
/// fetch task starts. Each fails its call as invalid arguments, and its
/// message is pinned word for word, because a model reads it. A call
/// with several faults reports the first: the `url` argument, then
/// `max_chars`, then `raw`, and only then what the policy makes of the
/// URL.
#[test]
fn a_call_the_tool_refuses_fails_as_invalid_arguments_with_its_message_word_for_word() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    let literal = "https://1.2.3.4/";
    let cases = [
        (json!({}), "web_fetch: missing url argument"),
        (json!(null), "web_fetch: missing url argument"),
        (json!({ "url": 7 }), "web_fetch: missing url argument"),
        (
            json!({ "max_chars": 0, "raw": "yes" }),
            "web_fetch: missing url argument",
        ),
        (
            json!({ "url": literal, "max_chars": 0, "raw": "yes" }),
            "web_fetch: max_chars must be a positive integer",
        ),
        (
            json!({ "url": literal, "raw": "yes" }),
            "web_fetch: raw must be a boolean",
        ),
        (json!({ "url": "not a url" }), "invalid url"),
        (
            json!({ "url": "https://user:pass@example.com/" }),
            "url must not contain userinfo",
        ),
        (
            json!({ "url": "https://example.com:8080/" }),
            "port not allowed: 8080",
        ),
        (
            json!({ "url": literal }),
            "ip literal host not allowed: 1.2.3.4",
        ),
    ];
    for (arguments, message) in cases {
        assert_eq!(
            pack.call("fetch", &arguments.to_string()),
            Ok(Err(ToolError::new(
                ToolErrorKind::InvalidArguments,
                message
            ))),
            "{arguments}"
        );
    }
}

/// The Plugin a Host loads resolves the name on its own runtime, and the
/// default policy drops every address of this machine, so nothing is
/// sent. The failure names the host and no address.
#[test]
fn the_plugin_a_host_loads_refuses_a_name_that_resolves_to_this_machine() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        fetch(&pack, "https://localhost/"),
        Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "host localhost has no allowed address"
        ))
    );
}

#[test]
fn a_scheme_the_default_policy_refuses_is_an_untrusted_output_that_says_so() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        fetch(&pack, "http://example.com/"),
        Ok(ToolOutput::untrusted("scheme not allowed: http"))
    );
}

#[test]
fn arguments_that_are_not_json_fail_their_call_as_invalid_arguments() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        pack.call("fetch", "{url: https://example.com/}"),
        Ok(Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "web: the call's arguments are not valid JSON"
        )))
    );
}

#[test]
fn a_fetch_answers_a_page_as_untrusted_markdown_under_its_header() {
    let (url, _events) = serve(Reply::Page("200 OK", ARTICLE));
    let loaded = loaded_for_loopback();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    // The test has no tokio runtime of its own, so the fetch can only
    // have run on the Plugin's, and the call came to its end only because
    // the fetch's task woke it.
    let output = fetch(&pack, &url).expect("the fetch succeeds");
    assert!(!output.is_trusted(), "a page is text from outside");
    let (header, text) = output
        .text()
        .split_once("\n\n")
        .expect("a blank line ends the header");
    assert_eq!(
        header,
        format!("url: {url}\ntruncated: false\nextraction: readability")
    );
    assert!(text.contains("first substantial paragraph"), "{text}");
}

#[test]
fn a_page_that_answers_an_http_error_is_an_untrusted_output_that_says_so() {
    let (url, _events) = serve(Reply::Page(
        "404 Not Found",
        "<html><body>Not Found</body></html>",
    ));
    let loaded = loaded_for_loopback();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        fetch(&pack, &url),
        Ok(ToolOutput::untrusted(format!(
            "{url} answered HTTP 404; try a different URL"
        )))
    );
}

#[test]
fn a_request_that_gets_no_response_is_an_untrusted_output_that_says_so() {
    let (url, _events) = serve(Reply::Nothing);
    let loaded = loaded_for_loopback();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(
        fetch(&pack, &url),
        Ok(ToolOutput::untrusted(format!(
            "fetch failed for {url}: network error; try a different URL"
        )))
    );
}

#[test]
fn a_call_that_is_dropped_aborts_its_fetch_and_the_plugin_unloads() {
    let (url, events) = serve(Reply::Hold);
    let loaded = loaded_for_loopback();
    {
        let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
        let mut call = pack.start("fetch", &json!({ "url": url }).to_string());
        assert_eq!(
            call.step(),
            Ok(None),
            "the call is parked while the server holds its answer"
        );
        // The call gets no further step, so the request can only have
        // gone out from the Plugin's runtime while the call was parked.
        assert_eq!(
            events.recv_timeout(WAIT),
            Ok(Event::Requested),
            "the fetch runs while its call is parked"
        );
        drop(call);
        // Only the abort of the fetch's task closes the connection
        // before the policy's timeout does.
        assert_eq!(
            events.recv_timeout(WAIT),
            Ok(Event::Closed),
            "dropping the call aborts its fetch, which closes the connection"
        );
    }
    assert_eq!(loaded.unload(), RV_OK);
}

/// How long the blocking thread of
/// [`dropping_the_instance_ends_every_thread_of_its_runtime`] is still at
/// work after the instance begins to drop.
const IN_FLIGHT: Duration = Duration::from_millis(200);

thread_local! {
    /// Counts its thread as ended. A thread drops its locals as it exits,
    /// so the count goes up when a marked thread is gone.
    static ENDS: RefCell<Option<CountsOnDrop>> = const { RefCell::new(None) };
}

/// Adds one to its count when it is dropped.
struct CountsOnDrop(Arc<AtomicUsize>);

impl Drop for CountsOnDrop {
    fn drop(&mut self) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

/// Marks the thread this runs on, so that the thread adds one to `ended`
/// as it exits.
fn mark_this_thread(ended: &Arc<AtomicUsize>) {
    ENDS.with(|ends| *ends.borrow_mut() = Some(CountsOnDrop(Arc::clone(ended))));
}

/// An unload drops the instance, and the library closes right after, so a
/// thread that is still running then crashes the process. The drop of the
/// instance has to wait for every thread of the runtime: its two workers,
/// and a blocking thread, the kind a host name is resolved on, that is
/// still at work.
#[test]
fn dropping_the_instance_ends_every_thread_of_its_runtime() {
    let web = Web::load().expect("the Plugin loads");
    let ended = Arc::new(AtomicUsize::new(0));
    let (marked, marks) = mpsc::channel();

    // A worker runs one task at a time. Each of these two tasks holds its
    // thread until both have marked theirs, so they run on the two
    // workers.
    let releases: Vec<_> = (0..2)
        .map(|_| {
            let (release, released) = mpsc::channel::<()>();
            let (ended, marked) = (Arc::clone(&ended), marked.clone());
            web.requests.spawn(async move {
                mark_this_thread(&ended);
                let _ = marked.send(());
                let _ = released.recv_timeout(WAIT);
            });
            release
        })
        .collect();
    for _ in 0..2 {
        marks
            .recv_timeout(WAIT)
            .expect("each worker runs one of the two tasks");
    }
    drop(releases);

    let ended_at_work = Arc::clone(&ended);
    web.requests.spawn_blocking(move || {
        mark_this_thread(&ended_at_work);
        let _ = marked.send(());
        thread::sleep(IN_FLIGHT);
    });
    marks
        .recv_timeout(WAIT)
        .expect("the blocking thread starts its work");

    assert_eq!(
        ended.load(Ordering::SeqCst),
        0,
        "every marked thread is running"
    );
    drop(web);
    assert_eq!(
        ended.load(Ordering::SeqCst),
        3,
        "the drop returned before a thread of the runtime had ended"
    );
}

#[path = "web-tests-search.rs"]
mod search;
