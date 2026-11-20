//! The search tool through the SDK's fake Host: a search of the Gateway
//! that the Host's resources name, the resources under which a search
//! fails and sends nothing, the `Authorization` header an entry's key
//! gives a request, the texts that hold no key, and the abort of a dropped
//! call's search.
//!
//! The mock Gateway runs on a runtime the test owns, and the test's own
//! thread has none. So a search that reached the Gateway can only have
//! been sent from the Plugin's runtime.

use std::collections::BTreeSet;
use std::sync::Arc;

use runvil_plugin_sdk::sys::RV_OK;
use runvil_plugin_sdk::testing::PackRun;
use runvil_plugin_sdk::{Plugin, ToolError, ToolErrorKind, ToolOutput};
use serde_json::{Value, json};
use tokio::runtime::Runtime;

use super::{Event, PACK, Reply, WAIT, Web, loaded, schema, serve_on_a_port};
use crate::mock_gateway::{MockServer, ROUTE, Seen, recording_gateway, resources};

/// A key that the mock Gateway is reached with, and that no text a test
/// can read may hold.
const KEY: &str = "gateway-key-7f3a9c";

/// What the tool answers for [`reply`]: the same results, as compact JSON.
const ANSWER: &str = r#"{"query":"rust","results":[{"title":"Rust","url":"https://www.rust-lang.org/","description":"A language."}]}"#;

/// The reply of the mock Gateway: one result.
fn reply() -> Value {
    json!({
        "query": "rust",
        "results": [{
            "title": "Rust",
            "url": "https://www.rust-lang.org/",
            "description": "A language."
        }]
    })
}

/// A mock Gateway on a runtime the test owns. It answers every search
/// with [`reply`] and records what it received.
struct Gateway {
    seen: Seen,
    server: MockServer,
    /// Runs the server. It is the last field, so the server's task is
    /// aborted before the runtime is dropped.
    _runtime: Runtime,
}

impl Gateway {
    /// Starts the Gateway on a runtime of its own.
    fn start() -> Gateway {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_all()
            .build()
            .expect("the test's runtime starts");
        let seen = Seen::default();
        let server = runtime.block_on(MockServer::spawn(recording_gateway(
            Arc::clone(&seen),
            reply(),
        )));
        Gateway {
            seen,
            server,
            _runtime: runtime,
        }
    }

    /// The address of the Gateway's web search route.
    fn url(&self) -> String {
        self.server.url()
    }

    /// The `Authorization` header and the body of each search the Gateway
    /// has received.
    fn seen(&self) -> Vec<(Option<String>, String)> {
        self.seen
            .lock()
            .expect("the capture lock is healthy")
            .clone()
    }
}

/// Runs one `search` call for `rust` on `pack` to its end, as the Harness
/// does.
fn search(pack: &PackRun<'_>) -> Result<ToolOutput, ToolError> {
    pack.call("search", &json!({ "query": "rust" }).to_string())
        .expect("the call follows the contract")
}

/// The failure of a search under a Host whose resources hold no entry.
fn no_web_search() -> ToolError {
    ToolError::new(
        ToolErrorKind::Transport,
        "web_search: the host offers no web search right now",
    )
}

/// A model reads the names of the parameters and which of them it must
/// send. The test of the tool itself pins the whole schema.
#[test]
fn the_search_tools_schema_reaches_the_host_as_json_text() {
    let loaded = loaded();
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    let schema = schema(&pack, "search");
    let properties: BTreeSet<&str> = schema["properties"]
        .as_object()
        .expect("the schema lists its properties")
        .keys()
        .map(String::as_str)
        .collect();
    assert_eq!(
        properties,
        BTreeSet::from([
            "count",
            "country",
            "exclude_domains",
            "freshness",
            "include_domains",
            "query",
            "safesearch",
            "search_lang"
        ])
    );
    assert_eq!(schema["required"], json!(["query"]));
    assert_eq!(schema["additionalProperties"], json!(false));
}

#[test]
fn a_search_posts_its_arguments_to_the_gateway_the_resources_name_and_answers_its_results() {
    let gateway = Gateway::start();
    let loaded = loaded();
    let pack = loaded
        .new_pack(PACK)
        .expect("the Pack instance is made")
        .with_resources(resources(&gateway.url(), Some(KEY)));
    // The test has no tokio runtime of its own, so the POST can only have
    // gone out from the Plugin's runtime, and the call came to its end
    // only because the task of its search woke it.
    let arguments = json!({ "query": "rust", "count": 3, "include_domains": [] });
    assert_eq!(
        pack.call("search", &arguments.to_string()),
        Ok(Ok(ToolOutput::untrusted(ANSWER)))
    );
    assert_eq!(
        gateway.seen(),
        [(
            Some(format!("Bearer {KEY}")),
            r#"{"query":"rust","count":3}"#.to_owned()
        )]
    );
}

#[test]
fn resources_without_the_entry_fail_a_search_as_transport_and_send_no_request() {
    let gateway = Gateway::start();
    let loaded = loaded();
    // A fake Host that is given no resources lends every call `{}`.
    let pack = loaded.new_pack(PACK).expect("the Pack instance is made");
    assert_eq!(search(&pack), Err(no_web_search()));

    for resources in [
        "{}".to_owned(),
        // A resource of another name is not the entry, though it names
        // the Gateway.
        json!({ "acme/web-search": { "version": 1, "url": gateway.url(), "api_key": KEY } })
            .to_string(),
    ] {
        let pack = loaded
            .new_pack(PACK)
            .expect("the Pack instance is made")
            .with_resources(resources.clone());
        assert_eq!(search(&pack), Err(no_web_search()), "{resources}");
    }
    assert!(gateway.seen().is_empty(), "no request is sent");
}

#[test]
fn an_entry_of_version_2_fails_a_search_as_other_and_sends_no_request() {
    let gateway = Gateway::start();
    let loaded = loaded();
    let version_2 =
        json!({ "runvil/web-search": { "version": 2, "url": gateway.url(), "api_key": KEY } });
    let pack = loaded
        .new_pack(PACK)
        .expect("the Pack instance is made")
        .with_resources(version_2.to_string());
    assert_eq!(
        search(&pack),
        Err(ToolError::new(
            ToolErrorKind::Other,
            "web_search: the host's runvil/web-search entry is not version 1"
        ))
    );
    assert!(gateway.seen().is_empty(), "no request is sent");
}

/// The Gateway admits a caller on its own machine that sends no
/// credentials, and it answers 401 to any `Authorization` header that is
/// not its key. So an entry with no key, or with an empty one, sends no
/// header at all.
#[test]
fn a_search_has_the_bearer_of_the_entrys_key_and_no_authorization_header_without_one() {
    let gateway = Gateway::start();
    let loaded = loaded();
    let url = gateway.url();
    let cases = [
        (json!({ "version": 1, "url": url }), None),
        (json!({ "version": 1, "url": url, "api_key": null }), None),
        (json!({ "version": 1, "url": url, "api_key": "" }), None),
        (
            json!({ "version": 1, "url": url, "api_key": KEY }),
            Some(format!("Bearer {KEY}")),
        ),
    ];
    for (entry, authorization) in cases {
        let pack = loaded
            .new_pack(PACK)
            .expect("the Pack instance is made")
            .with_resources(json!({ "runvil/web-search": entry }).to_string());
        assert_eq!(search(&pack), Ok(ToolOutput::untrusted(ANSWER)), "{entry}");
        let seen = gateway.seen();
        let (sent, _body) = seen.last().expect("the search reached the Gateway");
        assert_eq!(*sent, authorization, "{entry}");
    }
}

#[test]
fn no_debug_text_and_no_failure_holds_the_gateways_key() {
    let gateway = Gateway::start();
    let loaded = loaded();
    let pack = loaded
        .new_pack(PACK)
        .expect("the Pack instance is made")
        .with_resources(resources(&gateway.url(), Some(KEY)));
    let answered = search(&pack);
    assert_eq!(
        gateway.seen(),
        [(
            Some(format!("Bearer {KEY}")),
            r#"{"query":"rust"}"#.to_owned()
        )],
        "the key went out, in the header of the request"
    );

    // An entry the tool does not read, with the key in it.
    let version_2 =
        json!({ "runvil/web-search": { "version": 2, "url": gateway.url(), "api_key": KEY } });
    let other_version = loaded
        .new_pack(PACK)
        .expect("the Pack instance is made")
        .with_resources(version_2.to_string());
    let refused = search(&other_version);

    // A Gateway that is gone: its port is closed, so the request fails.
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port binds");
    let port = listener
        .local_addr()
        .expect("the listener has an address")
        .port();
    drop(listener);
    let gone = loaded
        .new_pack(PACK)
        .expect("the Pack instance is made")
        .with_resources(resources(
            &format!("http://127.0.0.1:{port}{ROUTE}"),
            Some(KEY),
        ));
    let failed = search(&gone);
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: request failed"
        ))
    );

    let in_flight = pack.start("search", &json!({ "query": "rust" }).to_string());
    let web = Web::load().expect("the Plugin loads");
    for shown in [
        format!("{loaded:?}"),
        format!("{pack:?}"),
        format!("{other_version:?}"),
        format!("{gone:?}"),
        format!("{in_flight:?}"),
        format!("{answered:?}"),
        format!("{refused:?}"),
        format!("{failed:?}"),
        format!("{web:?}"),
    ] {
        assert!(!shown.contains(KEY), "the text holds the key: {shown}");
    }
}

#[test]
fn a_call_that_is_dropped_aborts_its_search_and_the_plugin_unloads() {
    let (port, events) = serve_on_a_port(Reply::Hold);
    let loaded = loaded();
    {
        let pack = loaded
            .new_pack(PACK)
            .expect("the Pack instance is made")
            .with_resources(resources(&format!("http://127.0.0.1:{port}{ROUTE}"), None));
        let mut call = pack.start("search", &json!({ "query": "rust" }).to_string());
        assert_eq!(
            call.step(),
            Ok(None),
            "the call is parked while the Gateway holds its answer"
        );
        // The call gets no further step, so the request can only have
        // gone out from the Plugin's runtime while the call was parked.
        assert_eq!(
            events.recv_timeout(WAIT),
            Ok(Event::Requested),
            "the search runs while its call is parked"
        );
        drop(call);
        // Only the abort of the search's task closes the connection
        // before the deadline of the search does.
        assert_eq!(
            events.recv_timeout(WAIT),
            Ok(Event::Closed),
            "dropping the call aborts its search, which closes the connection"
        );
    }
    assert_eq!(loaded.unload(), RV_OK);
}
