//! The crate's Plugin, [`Web`], and the Pack it inserts. [`Web`] is the
//! instance the Host holds while the Plugin is loaded: it owns the fetch
//! client and the runtime that every fetch runs on. [`WebPack`] is one
//! run's instance of the Pack `runvil/web`: it lists the Pack's tool and
//! performs its calls.
//!
//! The fetch client is built once, when the Plugin loads, over the
//! default fetch policy. The Plugin is handed no configuration, so
//! nothing a Host or a prompt passes reaches that policy.

use std::error::Error;

use runvil_plugin_sdk::{
    BoxFuture, Pack, PackInfo, Plugin, Refusal, ToolCall, ToolError, ToolErrorKind, ToolInfo,
    ToolOutput,
};
use serde_json::Value;
use tokio::runtime::Runtime;

use crate::config::FetchConfig;
use crate::fetch::{FetchClient, WebFetch};

/// The Pack's name and description. Its name is `runvil/web`, so its Pack
/// id is `web`, and a prompt reaches its tool as `web/fetch`.
const INFO: PackInfo = PackInfo::new("runvil/web").description("Fetch pages from the web");

/// The fetch tool's name within the Pack, the last segment of its Lua id.
const FETCH: &str = "fetch";

/// The Plugin instance the Host holds while the Plugin is loaded. It
/// holds what every run shares: the fetch client and the runtime that
/// runs every fetch. Nothing outlives the instance.
#[derive(Debug)]
pub struct Web {
    /// The hardened client every fetch goes through, built once, so every
    /// run shares one connection pool.
    fetch: FetchClient,
    /// Runs every fetch: a call spawns its fetch here and awaits the
    /// task. Two worker threads, with I/O and time on. Dropping the
    /// instance drops it, which joins its threads, so an unload leaves no
    /// thread behind.
    requests: Runtime,
}

impl Web {
    /// The instance whose fetch tool enforces `policy`. [`Plugin::load`]
    /// passes the default policy, and only the crate's tests pass
    /// another.
    ///
    /// # Errors
    /// Returns a [`Refusal`] when the fetch client cannot be built, which
    /// happens when the TLS backend fails to start, and when the system
    /// will not start a thread of the runtime.
    fn with_fetch_config(policy: FetchConfig) -> Result<Web, Refusal> {
        let fetch = FetchClient::try_with_config(policy).map_err(|error| {
            Refusal::new(format!(
                "web could not build its fetch client: {}",
                chain(&error)
            ))
        })?;
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
        Ok(Web { fetch, requests })
    }
}

impl Plugin for Web {
    /// The name the Plugin reports.
    const NAME: &'static str = "web";
    /// This crate's version.
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const PACKS: &'static [PackInfo] = &[INFO];

    /// Builds the fetch client over the default policy and starts the
    /// runtime. It refuses the load when either cannot be built, and it
    /// never panics.
    fn load() -> Result<Web, Refusal> {
        Web::with_fetch_config(FetchConfig::default())
    }

    fn new_pack(&self, _pack: &str) -> Result<Box<dyn Pack>, Refusal> {
        Ok(Box::new(WebPack {
            fetch: self.fetch.tool(self.requests.handle().clone()),
        }))
    }
}

/// One run's instance of the Pack: its one tool, `fetch`. It holds only
/// what every run shares: a clone of the fetch client, which shares the
/// one connection pool, and the handle of the Plugin's runtime.
struct WebPack {
    fetch: WebFetch,
}

impl Pack for WebPack {
    fn tools(&self) -> Vec<ToolInfo> {
        vec![ToolInfo::new(
            FETCH,
            self.fetch.description(),
            self.fetch.parameters_schema().to_string(),
        )]
    }

    fn call<'a>(&'a self, call: ToolCall<'a>) -> BoxFuture<'a, Result<ToolOutput, ToolError>> {
        Box::pin(async move {
            // The arguments are parsed once, here, and the tool reads the
            // value.
            let args = arguments(call.arguments())?;
            match call.tool() {
                FETCH => self.fetch.call(args).await,
                other => Err(ToolError::new(
                    ToolErrorKind::Other,
                    format!("web has no tool named {other}"),
                )),
            }
        })
    }
}

/// The value of `text`, the JSON text of a call's arguments. The Harness
/// sends the text of a value it holds, so only another Host can send text
/// that is not JSON.
fn arguments(text: &str) -> Result<Value, ToolError> {
    serde_json::from_str(text).map_err(|_| {
        ToolError::new(
            ToolErrorKind::InvalidArguments,
            "web: the call's arguments are not valid JSON",
        )
    })
}

/// The text of `error`, and after it the text of each of its causes that
/// adds something. The client keeps the reason it could not be built,
/// such as a TLS backend that did not start, in the causes of its error.
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
#[path = "web-tests.rs"]
mod tests;
