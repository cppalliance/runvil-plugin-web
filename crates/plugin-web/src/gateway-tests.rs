//! The entry of the Host's resources that says where a search goes: the
//! JSON a Host writes, what reads as no entry, what reads as an entry of
//! another version, and the text that never leaves it. Then the search
//! tool against an axum mock gateway, reached through such an entry: the
//! bearer and the request body on the wire, the parsed reply, and where
//! a redirect takes the key.

use super::WebSearchResource;
use crate::mock_gateway::{MockServer, ROUTE, Seen, recording_gateway, search};

use std::sync::Arc;

use axum::Json;
use axum::Router;
use axum::http::HeaderMap;
use axum::response::Redirect;
use axum::routing::post;
use runvil_plugin_sdk::{ToolError, ToolErrorKind};
use serde_json::Value;

/// The resources of a Host that offers web search, as the Harness writes
/// them for a Gateway on this machine. The name of the entry and its
/// shape are written in two repositories, because a Plugin may not depend
/// on a Harness crate, so the Harness pins this same JSON in the test of
/// the function that offers the entry.
const RESOURCES: &str = r#"{"runvil/web-search":{"api_key":"...","url":"http://127.0.0.1:52814/v1/tools/web_search","version":1}}"#;

/// A key that no message and no `Debug` text may hold.
const SECRET: &str = "super-secret-token";

/// The failure of a search under a Host whose resources hold no entry.
fn no_web_search() -> ToolError {
    ToolError::new(
        ToolErrorKind::Transport,
        "web_search: the host offers no web search right now",
    )
}

/// The failure of a search whose entry is not version 1.
fn not_version_1() -> ToolError {
    ToolError::new(
        ToolErrorKind::Other,
        "web_search: the host's runvil/web-search entry is not version 1",
    )
}

/// The failure `resources` are refused with.
fn refused(resources: &str) -> ToolError {
    WebSearchResource::read(resources)
        .err()
        .unwrap_or_else(|| panic!("the resources must be refused: {resources}"))
}

#[test]
fn the_entry_a_host_writes_reads_as_version_1_with_its_address_and_its_key() {
    let entry = WebSearchResource::read(RESOURCES).expect("the entry reads");
    assert_eq!(entry.version, 1);
    assert_eq!(entry.url, "http://127.0.0.1:52814/v1/tools/web_search");
    assert_eq!(entry.api_key.as_deref(), Some("..."));
}

#[test]
fn an_entry_reads_without_a_key_with_an_empty_one_and_beside_what_it_does_not_know() {
    let url = "http://127.0.0.1:52814/v1/tools/web_search";
    for (resources, api_key) in [
        (
            serde_json::json!({ "runvil/web-search": { "version": 1, "url": url } }),
            None,
        ),
        (
            serde_json::json!({ "runvil/web-search": { "version": 1, "url": url, "api_key": null } }),
            None,
        ),
        (
            serde_json::json!({ "runvil/web-search": { "version": 1, "url": url, "api_key": "" } }),
            Some(""),
        ),
        // A resource of another name, and a field version 1 does not have.
        (
            serde_json::json!({
                "acme/other": { "version": 7 },
                "runvil/web-search": { "version": 1, "url": url, "api_key": "k", "region": "eu" }
            }),
            Some("k"),
        ),
    ] {
        let entry = WebSearchResource::read(&resources.to_string())
            .unwrap_or_else(|error| panic!("the entry must read: {resources}: {error}"));
        assert_eq!(entry.url, url, "{resources}");
        assert_eq!(entry.api_key.as_deref(), api_key, "{resources}");
    }
}

#[test]
fn resources_without_the_entry_fail_as_transport_with_the_message_word_for_word() {
    let url = "http://127.0.0.1:52814/v1/tools/web_search";
    for resources in [
        "{}".to_owned(),
        // The entry of a resource with another name is not this one.
        serde_json::json!({ "acme/web-search": { "version": 1, "url": url, "api_key": SECRET } })
            .to_string(),
        // Text that is not the JSON text of an object holds no entry.
        "null".to_owned(),
        "[]".to_owned(),
        String::new(),
        format!("not json, with {SECRET} in it"),
    ] {
        assert_eq!(refused(&resources), no_web_search(), "{resources}");
    }
}

#[test]
fn an_entry_of_another_version_or_shape_fails_as_other_with_the_message_word_for_word() {
    let url = "http://127.0.0.1:52814/v1/tools/web_search";
    for entry in [
        // Another version, in the shape of version 1 and in a later one.
        serde_json::json!({ "version": 2, "url": url, "api_key": SECRET }),
        serde_json::json!({ "version": 2, "endpoints": [url], "token": SECRET }),
        serde_json::json!({ "version": 0, "url": url }),
        // A version that is not the number 1.
        serde_json::json!({ "version": "1", "url": url }),
        serde_json::json!({ "version": 1.5, "url": url }),
        serde_json::json!({ "version": -1, "url": url }),
        serde_json::json!({ "url": url, "api_key": SECRET }),
        // Version 1, and not its shape.
        serde_json::json!({ "version": 1 }),
        serde_json::json!({ "version": 1, "url": 52814 }),
        serde_json::json!({ "version": 1, "url": url, "api_key": 7 }),
        // An entry that is no object.
        serde_json::json!(null),
        serde_json::json!(SECRET),
        serde_json::json!([1, url]),
    ] {
        let resources = serde_json::json!({ "runvil/web-search": entry }).to_string();
        assert_eq!(refused(&resources), not_version_1(), "{resources}");
    }
}

#[test]
fn no_failure_and_no_debug_text_of_an_entry_holds_its_key_or_its_address() {
    let url = format!("http://127.0.0.1:52814/v1/tools/web_search?key={SECRET}");
    let version_1 =
        serde_json::json!({ "runvil/web-search": { "version": 1, "url": url, "api_key": SECRET } })
            .to_string();
    let entry = WebSearchResource::read(&version_1).expect("the entry reads");
    let rendered = format!("{entry:?}");
    assert_eq!(rendered, "WebSearchResource { version: 1, .. }");

    let version_2 =
        serde_json::json!({ "runvil/web-search": { "version": 2, "url": url, "api_key": SECRET } })
            .to_string();
    let read = WebSearchResource::read(&version_2);
    for shown in [
        format!("{read:?}"),
        refused(&version_2).to_string(),
        refused(&format!("{version_1} trailing text")).to_string(),
    ] {
        assert!(
            !shown.contains(SECRET) && !shown.contains("52814"),
            "the text quotes the resources: {shown}"
        );
    }
}

/// The arguments of a search that names its `query` and nothing else.
fn query(text: &str) -> Value {
    serde_json::json!({ "query": text })
}

/// A router serving the canned success result at the search endpoint.
fn success_router() -> Router {
    async fn web_search(headers: HeaderMap, Json(body): Json<Value>) -> Json<Value> {
        let auth = headers
            .get("authorization")
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        assert_eq!(
            auth, "Bearer tok",
            "expected the bearer token to be forwarded"
        );
        assert_eq!(
            body.get("query").and_then(Value::as_str),
            Some("hi"),
            "expected the query to be forwarded in the body"
        );
        Json(serde_json::json!({
            "query": "hi",
            "results": [
                { "title": "T", "url": "https://e.com", "description": "D" }
            ]
        }))
    }
    Router::new().route(ROUTE, post(web_search))
}

#[tokio::test]
async fn forwards_query_and_returns_parsed_results() {
    let mock = MockServer::spawn(success_router()).await;

    let output = search(&mock.url(), query("hi"))
        .await
        .expect("search should succeed");

    assert_eq!(
        output.text(),
        r#"{"query":"hi","results":[{"title":"T","url":"https://e.com","description":"D"}]}"#,
        "expected the canned result to survive the round-trip"
    );
}

#[tokio::test]
async fn forwards_validated_optional_fields() {
    async fn web_search(Json(body): Json<Value>) -> Json<Value> {
        assert_eq!(body.get("count").and_then(Value::as_u64), Some(5));
        assert_eq!(body.get("freshness").and_then(Value::as_str), Some("pw"));
        assert_eq!(
            body.get("safesearch").and_then(Value::as_str),
            Some("strict")
        );
        assert_eq!(
            body.get("include_domains"),
            Some(&serde_json::json!(["example.com"]))
        );
        Json(serde_json::json!({ "query": "hi", "results": [{ "url": "https://e.com" }] }))
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;
    let request = serde_json::json!({
        "query": "hi",
        "count": 5,
        "freshness": "pw",
        "safesearch": "strict",
        "include_domains": ["example.com"]
    });

    search(&mock.url(), request)
        .await
        .expect("a fully-specified valid request should succeed");
}

#[tokio::test]
async fn parses_every_gateway_result_field_and_defaults_the_optional_ones() {
    async fn web_search() -> Json<Value> {
        // `took_ms` and `rank` are fields the reply does not have today:
        // the Gateway may grow its reply, so an unknown field is ignored.
        Json(serde_json::json!({
            "query": "boost",
            "took_ms": 12,
            "results": [
                {
                    "title": "T",
                    "url": "https://e.com",
                    "description": "D",
                    "age": "2 days ago",
                    "site_name": "e.com",
                    "extra_snippets": ["S1", "S2"],
                    "rank": 1
                },
                { "url": "https://f.com" }
            ]
        }))
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let output = search(&mock.url(), query("boost"))
        .await
        .expect("search should succeed");

    assert_eq!(
        output.text(),
        concat!(
            r#"{"query":"boost","results":["#,
            r#"{"title":"T","url":"https://e.com","description":"D","#,
            r#""age":"2 days ago","site_name":"e.com","extra_snippets":["S1","S2"]},"#,
            r#"{"title":"","url":"https://f.com","description":""}"#,
            "]}"
        )
    );
}

/// The client sets no redirect policy, so its HTTP library decides: it
/// follows a redirect, and it drops the `Authorization` header when the
/// redirect leaves the scheme, the host, or the port it was sent to.
#[tokio::test]
async fn a_redirect_is_followed_with_the_key_within_the_gateways_address_and_without_it_beyond() {
    let reply = serde_json::json!({ "query": "hi", "results": [] });
    let body = r#"{"query":"hi"}"#.to_owned();

    // A gateway that sends a search on to another path of its own address.
    let within = Seen::default();
    let onward = Router::new()
        .route(
            ROUTE,
            post(|| async { Redirect::temporary("/moved/v1/tools/web_search") }),
        )
        .nest(
            "/moved",
            recording_gateway(Arc::clone(&within), reply.clone()),
        );
    let mock = MockServer::spawn(onward).await;
    search(&mock.url(), query("hi"))
        .await
        .expect("the redirect is followed");
    assert_eq!(
        *within.lock().expect("the capture lock is healthy"),
        [(Some("Bearer tok".to_owned()), body.clone())]
    );

    // A gateway that sends a search on to another port of its host.
    let beyond = Seen::default();
    let target = MockServer::spawn(recording_gateway(Arc::clone(&beyond), reply)).await;
    let to = target.url();
    let onward = Router::new().route(
        ROUTE,
        post(move || {
            let to = to.clone();
            async move { Redirect::temporary(&to) }
        }),
    );
    let mock = MockServer::spawn(onward).await;
    search(&mock.url(), query("hi"))
        .await
        .expect("the redirect is followed");
    assert_eq!(
        *beyond.lock().expect("the capture lock is healthy"),
        [(None, body)]
    );
}

#[path = "gateway-tests-responses.rs"]
mod responses;
