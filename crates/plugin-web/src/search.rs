//! The `web_search` tool: validate a search's arguments, send them to the
//! Gateway that the Host's resources name, on the Plugin's runtime, and
//! return the results as untrusted compact JSON in the Gateway's shape.
//!
//! [`SearchRequest`] is the arguments as the model sent them, once they
//! deserialized and passed their bounds. Nothing sits between the tool and
//! the Gateway, so the same value serializes as the body of the Gateway's
//! web search request, and the Gateway's reply is written as the tool's
//! output.
//!
//! The Host polls a call on a thread of its own, which has no context of
//! the Plugin's runtime, and the HTTP client needs that context. So a call
//! sends no request itself: it runs the search as a task on the runtime,
//! through [`task::run`], and awaits the task, which wakes the call when
//! it ends.

use runvil_plugin_sdk::{ToolError, ToolErrorKind, ToolOutput};
use tokio::runtime::Handle;

use crate::gateway::{SearchClient, WebSearchResource};
use crate::task;

/// The largest accepted `query` string, in characters (Brave's documented cap).
const MAX_QUERY_LEN: usize = 400;
/// The inclusive upper bound on the requested result `count`.
const MAX_COUNT: u32 = 20;
/// The largest accepted free-form string argument (country, language, domain).
const MAX_STRING_LEN: usize = 128;
/// The largest number of hostnames accepted in a domain include/exclude list.
const MAX_DOMAINS: usize = 20;

/// A tool that searches the web through the Gateway.
///
/// Each call validates its arguments, reads where the search goes from the
/// Host's resources, and returns the Gateway's results as untrusted
/// output. It holds the shared [`SearchClient`], whose deadline covers
/// every search, and the handle of the Plugin's runtime, which every
/// search is spawned onto.
#[derive(Debug, Clone)]
pub(crate) struct WebSearch {
    /// The client every search goes through.
    client: SearchClient,
    /// The Plugin's runtime, which runs each search.
    runtime: Handle,
}

impl WebSearch {
    /// Builds the tool over `client`, spawning every search onto `runtime`.
    pub(crate) fn new(client: SearchClient, runtime: Handle) -> WebSearch {
        WebSearch { client, runtime }
    }

    /// The one sentence the model reads to decide when to search.
    #[expect(
        clippy::unused_self,
        reason = "both tools' descriptions and schemas are read through the tool, as the fetch tool's schema reads its policy"
    )]
    pub(crate) fn description(&self) -> &'static str {
        // Keep this sentence aligned with shipped prompts/picker fixtures; knobs
        // live in parameters_schema so capability bind stays stable.
        "Search the web and return a list of results (title, url, description)."
    }

    /// The JSON Schema for a search's arguments.
    #[expect(
        clippy::unused_self,
        reason = "both tools' descriptions and schemas are read through the tool, as the fetch tool's schema reads its policy"
    )]
    pub(crate) fn parameters_schema(&self) -> serde_json::Value {
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
    }

    /// Validates `args`, reads where the search goes from `resources`, the
    /// JSON text of the Host's resources, runs the search on the Plugin's
    /// runtime, and returns the results as untrusted compact JSON.
    /// Dropping the returned future aborts the search.
    ///
    /// # Errors
    /// Returns a [`ToolError`] for invalid arguments, for resources whose
    /// entry is absent or is not version 1, for a search the Gateway fails
    /// or answers with a malformed response, and for a search task that
    /// did not finish.
    pub(crate) async fn call(
        &self,
        args: serde_json::Value,
        resources: &str,
    ) -> Result<ToolOutput, ToolError> {
        // Validate and normalize arguments before the entry is read, so a
        // call with invalid arguments fails the same way under every Host;
        // only a validated request reaches the Gateway.
        let request = SearchRequest::from_args(args)?;
        let gateway = WebSearchResource::read(resources)?;

        // The task is all the call awaits, so the call is parked until the
        // task ends, which wakes it.
        let search = self.client.clone().search(gateway, request);
        let Some(searched) = task::run(&self.runtime, search).await else {
            return Err(ToolError::new(
                ToolErrorKind::Other,
                "web_search: the search task did not finish",
            ));
        };
        let body = serde_json::to_string(&searched?).map_err(|_| {
            ToolError::new(
                ToolErrorKind::Other,
                "web_search: the results could not be rendered",
            )
        })?;

        // The results embed third-party titles, URLs, and descriptions, so
        // the body is marked untrusted: it is nonce-wrapped before it can
        // reach model input.
        Ok(ToolOutput::untrusted(body))
    }
}

/// The search request as the model sent it, once it deserialized. It is
/// also the Gateway's web search request: a validated value serializes as
/// that request's body.
///
/// `deny_unknown_fields` means an argument the tool does not model is rejected
/// (rather than silently forwarded), and the typed optional fields reject a
/// wrong JSON type at deserialization. [`SearchRequest::validate`] then enforces
/// the string, count, and domain bounds.
///
/// Serialized, the fields keep the order of the Gateway's request. Only
/// `query` is always there: an absent option or an empty domain list is
/// left out of the body, and the Gateway applies its own default.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(deny_unknown_fields)]
struct SearchRequest {
    /// The search query.
    query: String,
    /// Maximum number of results.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    count: Option<u32>,
    /// Freshness filter.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    freshness: Option<Freshness>,
    /// Country code for the search.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    country: Option<String>,
    /// Search language code.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    search_lang: Option<String>,
    /// SafeSearch level.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    safesearch: Option<SafeSearch>,
    /// Only keep results from these hostnames.
    #[serde(default, skip_serializing_if = "is_none_or_empty")]
    include_domains: Option<Vec<String>>,
    /// Drops results from these hostnames.
    #[serde(default, skip_serializing_if = "is_none_or_empty")]
    exclude_domains: Option<Vec<String>>,
}

/// Whether a domain list is left out of the Gateway's request: the model
/// sent none, or sent an empty one, which keeps every result as none does.
#[expect(
    clippy::ref_option,
    reason = "serde hands `skip_serializing_if` a reference to the field"
)]
fn is_none_or_empty(domains: &Option<Vec<String>>) -> bool {
    domains.as_ref().is_none_or(Vec::is_empty)
}

/// How recent the results of a search must be.
///
/// It is read from the model's arguments. Any token other than `pd`, `pw`,
/// `pm`, or `py` fails to deserialize, so that token is an invalid argument
/// and never reaches the Gateway.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum Freshness {
    /// Past day.
    Pd,
    /// Past week.
    Pw,
    /// Past month.
    Pm,
    /// Past year.
    Py,
}

/// The SafeSearch filtering level of a search.
///
/// It is read from the model's arguments. Any token other than `off`,
/// `moderate`, or `strict` fails to deserialize, so that token is an
/// invalid argument and never reaches the Gateway.
#[derive(Debug, serde::Deserialize, serde::Serialize)]
#[serde(rename_all = "lowercase")]
enum SafeSearch {
    /// Filtering turned off.
    Off,
    /// Moderate filtering.
    Moderate,
    /// Strict filtering.
    Strict,
}

impl SearchRequest {
    /// Deserializes and validates the raw call arguments.
    fn from_args(args: serde_json::Value) -> Result<SearchRequest, ToolError> {
        let request: SearchRequest = serde_json::from_value(args).map_err(|_| {
            ToolError::new(
                ToolErrorKind::InvalidArguments,
                "web_search: invalid arguments",
            )
        })?;
        request.validate()?;
        Ok(request)
    }

    /// Enforces the bounds the type alone cannot express.
    fn validate(&self) -> Result<(), ToolError> {
        let invalid = |message: String| ToolError::new(ToolErrorKind::InvalidArguments, message);
        if self.query.trim().is_empty() {
            return Err(invalid("web_search: query must not be empty".to_owned()));
        }
        if self.query.chars().count() > MAX_QUERY_LEN {
            return Err(invalid(format!(
                "web_search: query exceeds {MAX_QUERY_LEN} characters"
            )));
        }
        if let Some(count) = self.count
            && !(1..=MAX_COUNT).contains(&count)
        {
            return Err(invalid(format!(
                "web_search: count must be between 1 and {MAX_COUNT}"
            )));
        }
        for (field, value) in [
            ("country", &self.country),
            ("search_lang", &self.search_lang),
        ] {
            if let Some(value) = value
                && (value.trim().is_empty() || value.chars().count() > MAX_STRING_LEN)
            {
                return Err(invalid(format!(
                    "web_search: {field} must be 1..={MAX_STRING_LEN} characters"
                )));
            }
        }
        for (field, domains) in [
            ("include_domains", &self.include_domains),
            ("exclude_domains", &self.exclude_domains),
        ] {
            if let Some(domains) = domains {
                if domains.len() > MAX_DOMAINS {
                    return Err(invalid(format!(
                        "web_search: {field} may list at most {MAX_DOMAINS} hostnames"
                    )));
                }
                for domain in domains {
                    let bad = domain.trim().is_empty()
                        || domain.chars().count() > MAX_STRING_LEN
                        || domain.contains('/')
                        || domain.chars().any(|c| c.is_whitespace() || c.is_control());
                    if bad {
                        return Err(invalid(format!(
                            "web_search: {field} contains an invalid hostname"
                        )));
                    }
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "search-tests.rs"]
mod tests;
