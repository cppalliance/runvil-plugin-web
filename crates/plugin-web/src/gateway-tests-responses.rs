//! Gateway response handling: a refused connection or a stalled gateway is
//! a transport failure, a success body with the wrong shape or invalid
//! UTF-8 is a backend failure, an oversized success body is rejected
//! rather than truncated, a success body that fails or stalls mid-read is
//! a transport failure, and an error body is bounded and sanitized, and
//! says so when it is empty or cannot be read. Each failure is the whole
//! of what a model reads: its kind, and its message word for word.

use super::*;

use std::net::SocketAddr;
use std::time::Duration;

use runvil_plugin_sdk::ToolOutput;

use crate::gateway::{MAX_ERROR_BODY, MAX_RESPONSE_BODY, SearchClient};
use crate::mock_gateway::{KEY, resources, tool_over};

/// A search of the Gateway at `url` under a deadline of 200 milliseconds.
async fn search_under_a_short_deadline(url: &str) -> Result<ToolOutput, ToolError> {
    let client =
        SearchClient::with_timeout(Duration::from_millis(200)).expect("the search client builds");
    tool_over(client)
        .call(query("hi"), &resources(url, Some(KEY)))
        .await
}

#[tokio::test]
async fn a_refused_connection_is_a_transport_failure() {
    // Bind then drop the listener so the port is closed and the connection
    // is refused deterministically.
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    drop(listener);

    let failed = search(&format!("http://{addr}{ROUTE}"), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: request failed"
        )),
        "a refused connection must surface as an error"
    );
}

#[tokio::test]
async fn stalling_gateway_times_out_as_transport() {
    async fn web_search() -> Json<Value> {
        std::future::pending().await
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    // The gateway never answers, so only the deadline ends the search.
    let failed = search_under_a_short_deadline(&mock.url()).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: request failed"
        )),
        "a stalled gateway must surface as an error"
    );
}

#[tokio::test]
async fn malformed_success_json_is_a_backend_failure() {
    async fn web_search() -> Json<Value> {
        // Missing the required `results` array: valid JSON, wrong shape.
        Json(serde_json::json!({ "unexpected": true }))
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let failed = search(&mock.url(), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: malformed search response"
        )),
        "a wrong-shaped success body must be rejected"
    );
}

#[tokio::test]
async fn non_utf8_success_body_is_a_backend_failure() {
    async fn web_search() -> Vec<u8> {
        vec![b'{', 0xff, 0xfe, b'}']
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let failed = search(&mock.url(), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: response body was not valid UTF-8"
        )),
        "a success body that is not UTF-8 must be rejected"
    );
}

#[tokio::test]
async fn oversized_success_body_is_rejected() {
    async fn web_search() -> String {
        "x".repeat(MAX_RESPONSE_BODY + 4096)
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let failed = search(&mock.url(), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: response body exceeded 262144 bytes"
        )),
        "an oversized success body must be rejected, not truncated, and the error must name the cap"
    );
}

#[tokio::test]
async fn an_error_status_is_a_backend_failure_that_names_it_and_quotes_its_body() {
    async fn web_search() -> (axum::http::StatusCode, &'static str) {
        (axum::http::StatusCode::BAD_GATEWAY, "upstream down")
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let failed = search(&mock.url(), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: backend returned 502: upstream down"
        )),
        "a 502 fails the search"
    );
}

#[tokio::test]
async fn an_error_status_with_no_body_says_that_the_body_is_empty() {
    // The Gateway answers a wrong path with a bare 404.
    let mock = MockServer::spawn(Router::new()).await;

    let failed = search(&mock.url(), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: backend returned 404: (empty body)"
        ))
    );
}

#[tokio::test]
async fn oversized_error_body_is_bounded_and_sanitized() {
    async fn web_search() -> (axum::http::StatusCode, String) {
        // Oversized and control-laden so both bounding and sanitization run.
        let mut body = "line-one\nline-two\ttab".to_owned();
        body.push_str(&"e".repeat(MAX_ERROR_BODY * 4));
        (axum::http::StatusCode::INTERNAL_SERVER_ERROR, body)
    }
    let mock = MockServer::spawn(Router::new().route(ROUTE, post(web_search))).await;

    let err = search(&mock.url(), query("hi"))
        .await
        .expect_err("a 500 response must surface as an error");
    assert_eq!(err.kind(), ToolErrorKind::Backend);
    let message = err.message();
    assert!(
        message.starts_with("web_search: backend returned 500: line-one\\nline-two\\ttab"),
        "error must name the status and escape the body: {message}"
    );
    assert!(
        !message.contains('\n') && !message.contains('\t'),
        "control characters must be escaped, got: {message}"
    );
    assert!(
        message.len() < MAX_ERROR_BODY + 128,
        "the error-path body must be bounded, got {} bytes",
        message.len()
    );
}

/// What a raw TCP mock does once it has sent the start of its body.
enum Then {
    /// Drops the connection, so the body read fails partway.
    Closes,
    /// Keeps the connection open and sends no more, so the body read
    /// stalls.
    Stalls,
}

/// A raw TCP mock that answers with `status`, promises a large body via
/// `Content-Length`, sends a few bytes, and `then` drops the connection or
/// holds it open.
async fn spawn_truncated_reply(
    status: &'static str,
    then: Then,
) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    let handle = tokio::spawn(async move {
        use tokio::io::{AsyncReadExt as _, AsyncWriteExt as _};
        if let Ok((mut socket, _)) = listener.accept().await {
            let mut buf = [0u8; 1024];
            let _ = socket.read(&mut buf).await;
            let header = format!("HTTP/1.1 {status}\r\nContent-Length: 100000\r\n\r\n");
            let _ = socket.write_all(header.as_bytes()).await;
            let _ = socket.write_all(b"partial").await;
            let _ = socket.flush().await;
            if matches!(then, Then::Stalls) {
                std::future::pending::<()>().await;
            }
        }
    });
    (addr, handle)
}

#[tokio::test]
async fn success_body_read_failure_is_a_transport_failure() {
    let (addr, handle) = spawn_truncated_reply("200 OK", Then::Closes).await;

    let failed = search(&format!("http://{addr}{ROUTE}"), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: reading response failed"
        )),
        "a truncated 200 body must surface as an error"
    );
    handle.abort();
}

/// The deadline covers the whole exchange: a reply whose head came in
/// time still fails when its body does not.
#[tokio::test]
async fn a_success_body_that_stalls_past_the_deadline_is_a_transport_failure() {
    let (addr, handle) = spawn_truncated_reply("200 OK", Then::Stalls).await;

    let failed = search_under_a_short_deadline(&format!("http://{addr}{ROUTE}")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Transport,
            "web_search: reading response failed"
        ))
    );
    handle.abort();
}

#[tokio::test]
async fn error_body_read_failure_is_a_backend_failure_that_says_so() {
    let (addr, handle) = spawn_truncated_reply("500 Internal Server Error", Then::Closes).await;

    let failed = search(&format!("http://{addr}{ROUTE}"), query("hi")).await;
    assert_eq!(
        failed,
        Err(ToolError::new(
            ToolErrorKind::Backend,
            "web_search: backend returned 500, and its error body could not be read"
        )),
        "a truncated 500 body must surface as an error"
    );
    handle.abort();
}
