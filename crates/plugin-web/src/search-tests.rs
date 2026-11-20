//! Tests for the `WebSearch` tool against a mock Gateway, reached through
//! its entry of the Host's resources: description and schema, argument
//! validation, the request the Gateway receives, the rendered results,
//! how a failure of the Gateway fails the call, and the call whose search
//! task is lost.

use std::sync::Arc;

use runvil_plugin_sdk::{ToolError, ToolErrorKind};
use serde_json::Value;

use super::{MAX_COUNT, MAX_DOMAINS, MAX_QUERY_LEN, MAX_STRING_LEN, WebSearch};
use crate::gateway::SearchClient;
use crate::mock_gateway::{
    KEY, MockServer, ROUTE, Seen, recording_gateway, resources, search, tool,
};

/// A reply with one result that has a title and a URL.
fn one_result() -> Value {
    serde_json::json!({
        "query": "hi",
        "results": [{ "title": "T", "url": "https://e.com", "description": "D" }]
    })
}

/// A mock Gateway that answers every search with `reply`, and what it
/// has seen of each.
async fn gateway_answering(reply: Value) -> (MockServer, Seen) {
    let seen = Seen::default();
    let mock = MockServer::spawn(recording_gateway(Arc::clone(&seen), reply)).await;
    (mock, seen)
}

/// The body of each search `seen` holds, checked to have had the bearer
/// of [`KEY`].
fn bodies(seen: &Seen) -> Vec<String> {
    let seen = seen.lock().expect("the capture lock is healthy");
    for (auth, _body) in seen.iter() {
        assert_eq!(auth.as_deref(), Some("Bearer tok"));
    }
    seen.iter().map(|(_auth, body)| body.clone()).collect()
}

/// Calls the tool with `args` against a Gateway that would answer,
/// expecting a refusal as invalid arguments, and checks the Gateway was
/// never asked. Returns the message of the refusal.
async fn refused(args: Value) -> String {
    let (mock, seen) = gateway_answering(one_result()).await;
    let err = search(&mock.url(), args)
        .await
        .expect_err("invalid arguments must be refused");
    assert!(
        bodies(&seen).is_empty(),
        "invalid arguments never reach the Gateway"
    );
    assert_eq!(err.kind(), ToolErrorKind::InvalidArguments);
    err.message().to_owned()
}

#[tokio::test]
async fn the_description_and_schema_are_stable_and_faithful() {
    let tool = tool();

    assert_eq!(
        tool.description(),
        "Search the web and return a list of results (title, url, description)."
    );
    assert_eq!(
        tool.parameters_schema(),
        serde_json::json!({
            "type": "object",
            "additionalProperties": false,
            "properties": {
                "query": {
                    "type": "string",
                    "description": "The search query.",
                    "minLength": 1,
                    "maxLength": MAX_QUERY_LEN
                },
                "count": {
                    "type": "integer",
                    "description": "Max number of results.",
                    "minimum": 1,
                    "maximum": MAX_COUNT
                },
                "freshness": {
                    "type": "string",
                    "description": "Freshness filter.",
                    "enum": ["pd", "pw", "pm", "py"]
                },
                "country": {
                    "type": "string",
                    "description": "Country code for the search.",
                    "maxLength": MAX_STRING_LEN
                },
                "search_lang": {
                    "type": "string",
                    "description": "Search language code.",
                    "maxLength": MAX_STRING_LEN
                },
                "safesearch": {
                    "type": "string",
                    "description": "SafeSearch level.",
                    "enum": ["off", "moderate", "strict"]
                },
                "include_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Only keep results from these hostnames."
                },
                "exclude_domains": {
                    "type": "array",
                    "items": { "type": "string" },
                    "maxItems": MAX_DOMAINS,
                    "description": "Drop results from these hostnames."
                }
            },
            "required": ["query"]
        })
    );
}

#[tokio::test]
async fn the_gateway_receives_the_validated_arguments_as_its_request() {
    let (mock, seen) = gateway_answering(one_result()).await;
    search(
        &mock.url(),
        serde_json::json!({
            "query": "hi",
            "count": 3,
            "freshness": "pw",
            "country": "de",
            "search_lang": "en",
            "safesearch": "strict",
            "include_domains": ["e.com"],
            "exclude_domains": ["g.com"]
        }),
    )
    .await
    .expect("a fully-specified valid request should succeed");

    // The body as the Gateway received it: the field names of the
    // Gateway's request, in the order of that request.
    assert_eq!(
        bodies(&seen),
        [concat!(
            r#"{"query":"hi","count":3,"freshness":"pw","country":"de","search_lang":"en","#,
            r#""safesearch":"strict","include_domains":["e.com"],"exclude_domains":["g.com"]}"#
        )]
    );
}

#[tokio::test]
async fn an_option_the_call_leaves_out_and_an_empty_domain_list_are_left_out_of_the_request() {
    let (mock, seen) = gateway_answering(one_result()).await;
    for args in [
        serde_json::json!({ "query": "hi" }),
        serde_json::json!({
            "query": "hi",
            "count": null,
            "freshness": null,
            "country": null,
            "search_lang": null,
            "safesearch": null,
            "include_domains": null,
            "exclude_domains": null
        }),
        serde_json::json!({ "query": "hi", "include_domains": [], "exclude_domains": [] }),
    ] {
        search(&mock.url(), args.clone())
            .await
            .unwrap_or_else(|error| panic!("the search must succeed: {args}: {error}"));
    }

    assert_eq!(bodies(&seen), [r#"{"query":"hi"}"#; 3]);
}

/// A reply with two results: one with every optional field, one with none.
fn full_and_bare_results() -> Value {
    serde_json::json!({
        "query": "rust",
        "results": [
            {
                "title": "Rust",
                "url": "https://www.rust-lang.org/",
                "description": "A language.",
                "age": "2 days ago",
                "site_name": "www.rust-lang.org",
                "extra_snippets": ["Fast.", "Safe."]
            },
            { "url": "https://e.com" }
        ]
    })
}

#[tokio::test]
async fn results_render_as_the_gateways_serialization_and_are_untrusted() {
    let (mock, _seen) = gateway_answering(full_and_bare_results()).await;
    let output = search(&mock.url(), serde_json::json!({ "query": "rust" }))
        .await
        .expect("the Gateway's results render");

    assert!(
        !output.is_trusted(),
        "external search content must be marked untrusted"
    );
    // The Gateway's `WebSearchResponse` serialization of the same results:
    // its field order, with an absent `age` or `site_name` and empty
    // `extra_snippets` left out.
    assert_eq!(
        output.text(),
        concat!(
            r#"{"query":"rust","results":["#,
            r#"{"title":"Rust","url":"https://www.rust-lang.org/","description":"A language.","#,
            r#""age":"2 days ago","site_name":"www.rust-lang.org","extra_snippets":["Fast.","Safe."]},"#,
            r#"{"title":"","url":"https://e.com","description":""}"#,
            "]}"
        )
    );
}

/// A reply whose second result has a blank URL.
fn blank_url_result() -> Value {
    serde_json::json!({
        "query": "hi",
        "results": [{ "url": "https://e.com" }, { "url": "  " }]
    })
}

#[tokio::test]
async fn success_body_with_empty_url_is_rejected() {
    let (mock, _seen) = gateway_answering(blank_url_result()).await;
    let err = search(&mock.url(), serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("an empty result url must be rejected");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert_eq!(
        err.to_string(),
        "web_search: malformed search response: result 1 has an empty url"
    );
}

#[tokio::test]
async fn a_failure_of_the_gateway_fails_the_call_with_its_kind_and_its_message() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);
    let err = search(
        &format!("http://{addr}{ROUTE}"),
        serde_json::json!({ "query": "hi" }),
    )
    .await
    .expect_err("a transport failure fails the call");
    assert_eq!(err.kind(), ToolErrorKind::Transport);
    assert_eq!(err.message(), "web_search: request failed");

    // A mock with no route answers every search with a bare 404.
    let mock = MockServer::spawn(axum::Router::new()).await;
    let err = search(&mock.url(), serde_json::json!({ "query": "hi" }))
        .await
        .expect_err("a backend failure fails the call");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    assert_eq!(
        err.message(),
        "web_search: backend returned 404: (empty body)"
    );
}

#[tokio::test]
async fn rejects_missing_query() {
    assert_eq!(
        refused(serde_json::json!({ "count": 3 })).await,
        "web_search: invalid arguments"
    );
    // Arguments that are no value at all hold no query.
    assert_eq!(
        refused(serde_json::json!(null)).await,
        "web_search: invalid arguments"
    );
}

#[tokio::test]
async fn rejects_empty_and_oversized_query() {
    assert_eq!(
        refused(serde_json::json!({ "query": "   " })).await,
        "web_search: query must not be empty"
    );
    let long = "x".repeat(MAX_QUERY_LEN + 1);
    assert_eq!(
        refused(serde_json::json!({ "query": long })).await,
        "web_search: query exceeds 400 characters"
    );
}

#[tokio::test]
async fn rejects_unknown_fields_and_bad_optional_types() {
    // Unknown field. The message names no field: it is the same text for
    // every argument that does not deserialize.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "nonsense": 1 })).await,
        "web_search: invalid arguments"
    );
    // Wrong type for count.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": "five" })).await,
        "web_search: invalid arguments"
    );
    // Out-of-range count.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": MAX_COUNT + 1 })).await,
        "web_search: count must be between 1 and 20"
    );
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "count": 0 })).await,
        "web_search: count must be between 1 and 20"
    );
    // Unknown enum values.
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "freshness": "yesterday" })).await,
        "web_search: invalid arguments"
    );
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "safesearch": "maybe" })).await,
        "web_search: invalid arguments"
    );
}

#[tokio::test]
async fn rejects_blank_and_oversized_country_and_language_codes() {
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "country": " " })).await,
        "web_search: country must be 1..=128 characters"
    );
    let long = "x".repeat(MAX_STRING_LEN + 1);
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "search_lang": long })).await,
        "web_search: search_lang must be 1..=128 characters"
    );
}

#[tokio::test]
async fn rejects_invalid_domain_lists() {
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "include_domains": ["ok.com", "bad/host"] }))
            .await,
        "web_search: include_domains contains an invalid hostname"
    );
    let many: Vec<String> = (0..30).map(|i| format!("h{i}.com")).collect();
    assert_eq!(
        refused(serde_json::json!({ "query": "hi", "exclude_domains": many })).await,
        "web_search: exclude_domains may list at most 20 hostnames"
    );
    // A hostname that is blank, holds whitespace, or is too long.
    let long = "x".repeat(MAX_STRING_LEN + 1);
    for domain in ["", "a b.com", long.as_str()] {
        assert_eq!(
            refused(serde_json::json!({ "query": "hi", "exclude_domains": [domain] })).await,
            "web_search: exclude_domains contains an invalid hostname",
            "{domain:?}"
        );
    }
}

/// The arguments are checked before the entry is read, so a call the tool
/// refuses fails the same way under a Host with no web search.
#[tokio::test]
async fn arguments_are_validated_before_the_entry_is_read() {
    let tool = tool();
    assert_eq!(
        tool.call(serde_json::json!({ "query": " " }), "{}").await,
        Err(ToolError::new(
            ToolErrorKind::InvalidArguments,
            "web_search: query must not be empty"
        ))
    );
    assert_eq!(
        tool.call(serde_json::json!({ "query": "hi" }), "{}").await,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: the host offers no web search right now"
        ))
    );
}

/// A runtime that has shut down drops a task as it is spawned, which is
/// how a search task is lost without a panic of its own.
#[tokio::test]
async fn a_call_whose_search_task_is_lost_fails_as_other_and_says_so() {
    let gone = tokio::runtime::Builder::new_current_thread()
        .build()
        .expect("a runtime builds");
    let handle = gone.handle().clone();
    gone.shutdown_background();
    let client = SearchClient::try_new().expect("the search client builds");
    let tool = WebSearch::new(client, handle);

    let (mock, seen) = gateway_answering(one_result()).await;
    assert_eq!(
        tool.call(
            serde_json::json!({ "query": "hi" }),
            &resources(&mock.url(), Some(KEY))
        )
        .await,
        Err(ToolError::new(
            ToolErrorKind::Other,
            "web_search: the search task did not finish"
        ))
    );
    assert!(bodies(&seen).is_empty(), "a lost task sends no request");
}
