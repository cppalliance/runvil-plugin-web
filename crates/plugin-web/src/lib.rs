//! `plugin-web` - the web Plugin. It inserts one Pack, `runvil/web`,
//! which gives a run two tools: one that searches the web, and one that
//! fetches a page from it.
//!
//! The Pack's id is `web`, and it has two tools:
//!
//! | Lua id | Name the model calls | What a call does |
//! |---|---|---|
//! | `web/fetch` | `web_fetch` | Fetches one page and answers its main content as markdown. |
//! | `web/search` | `web_search` | Runs one web search and answers a list of results. |
//!
//! A script calls a tool by its Lua id, `tools.call("web/fetch", { url
//! = "https://example.com/" })`, and offers it to the model with
//! `tools.offer("web/fetch")`. Neither tool sets a wire name, so the
//! Host derives the name the model calls each by from its Lua id.
//!
//! The Pack has no prelude, so declaring it, as in `uses: [web]`, only
//! makes it required. It takes no configuration. A Host says where a
//! search goes in its resources, which every tool call is lent. The
//! fetch tool needs nothing from a Host.
//!
//! `cargo build -p plugin-web --release` writes `plugin_web.dll` on
//! Windows, `libplugin_web.so` on Linux, and `libplugin_web.dylib` on
//! macOS to `target/release`; that one file is what goes in a Host's
//! Plugin directory.
//!
//! # The fetch tool
//!
//! A call names a `url`. It may also name `max_chars`, the most
//! characters of text to answer, and `raw`, which skips article
//! extraction. The tool sends one GET request and chooses how to render
//! the response from its `Content-Type`. For an HTML page, it extracts
//! the main article with `readabilityrs` and renders it to markdown.
//! When a page yields too little article text, or the call asks for
//! `raw`, `htmd` converts the whole page to markdown. Any other text
//! body, such as JSON, XML, or plain text, is decoded and returned
//! verbatim. The tool refuses every other type, and a response that
//! declares none.
//!
//! The answer is untrusted output: three header lines, then a blank
//! line, then the text. The header lines are `url:`, the URL the text
//! came from once every redirect was followed, `truncated:`, and
//! `extraction:`, how the text was made. The text is cut at 40,000
//! characters, or at `max_chars` when the call names fewer.
//!
//! A call fails as invalid arguments when its arguments do not match the
//! tool's parameters, and when the fetch policy can never fetch its URL:
//! one that does not parse, holds a user name or a password, names a
//! port outside the policy, has an IP address for its host, or resolves
//! to no address the policy allows. A call whose fetch task panics, or
//! whose page makes the rendering panic, fails with `web_fetch: the fetch
//! task did not finish`. Anything else that goes wrong is an untrusted
//! output that says so, such as an HTTP error status, a timeout, a
//! refused redirect, a body over the size cap, or a type the tool does
//! not render. The model reads it and can try another URL.
//!
//! # The fetch policy
//!
//! The model supplies the URL, so the tool is the server-side request
//! forgery (SSRF) boundary between an untrusted argument and the
//! network. It enforces the built-in default fetch policy, and nothing a
//! Host or a prompt passes changes that policy:
//!
//! - The scheme is `https`, the port is 80 or 443, and the URL holds no
//!   user name and no password.
//! - The host is a name and never an IP address. Each address the name
//!   resolves to is checked as the connection is made, and one that is
//!   not public is dropped: loopback, private, link-local, and every
//!   other range in the table in `address.rs`. A name with no address
//!   left is refused.
//! - Each redirect is checked the same way before it is followed. At
//!   most 5 are followed, and none from `https` to `http`.
//! - A body over 8 MiB, counted after decompression, is refused. A body
//!   of flat text, such as `text/plain`, is cut there instead, because
//!   its start is still of use. A connection has 5 seconds to open, and
//!   a whole request has 20 seconds.
//!
//! # The search tool
//!
//! A call names a `query` of at most 400 characters. It may also name
//! `count`, the most results to answer, from 1 to 20; `freshness`, one
//! of `pd`, `pw`, `pm`, and `py`, for the past day, week, month, and
//! year; `country` and `search_lang`, a country code and a language
//! code; `safesearch`, one of `off`, `moderate`, and `strict`; and
//! `include_domains` and `exclude_domains`, each a list of at most 20
//! hostnames, which keep and drop the results from those hosts. The tool
//! refuses an argument it does not know.
//!
//! The tool runs no search of its own. It sends the checked arguments to
//! the Gateway, as one POST of the Gateway's web search request, and the
//! Gateway runs the search with the key of its search vendor, so the
//! Plugin holds no such key. An option the call leaves out, and a domain
//! list that is empty, are left out of the request, and the Gateway
//! applies its own default. The Gateway checks the arguments again by
//! rules of its own, which are stricter in places, such as the form of a
//! country code, and it answers what it refuses with an error status.
//!
//! The answer is untrusted output: the Gateway's reply as compact JSON,
//! an object with the `query` the Gateway ran and a `results` array.
//! Each result has a `title`, a `url`, and a `description`, and it has
//! an `age`, a `site_name`, and `extra_snippets` when the Gateway sent
//! them.
//!
//! A call that fails says why in its message, and its kind is one of
//! four:
//!
//! - Invalid arguments, when the arguments do not match the tool's
//!   parameters. They are checked before anything else, so such a call
//!   fails the same way under every Host.
//! - A transport failure, when the Host offers no web search, with
//!   `web_search: the host offers no web search right now`, when the
//!   request does not get through, and when the Gateway's reply cannot
//!   be read to its end. A search has 30 seconds for the whole exchange,
//!   the read of the reply included.
//! - A backend failure, when the Gateway answers an error status, and
//!   when it answers a reply the tool rejects: one over 256 KiB, one
//!   that is not UTF-8, one that is not the Gateway's web search reply,
//!   or one in which a result has a blank `url`. The failure for an
//!   error status quotes the Gateway's error body, cut at 2000 bytes,
//!   with its control characters escaped, or says that the body could
//!   not be read.
//! - An other failure, when the Host's entry is not version 1, with
//!   `web_search: the host's runvil/web-search entry is not version 1`,
//!   and when the task of the search does not come to its end, with
//!   `web_search: the search task did not finish`.
//!
//! # Where a search goes
//!
//! Every tool call is lent the Host's resources: the JSON text of an
//! object keyed by resource name, as it was when the call started. The
//! search tool reads one entry of it, `runvil/web-search`, and that
//! entry is all a Host writes to give its runs web search:
//!
//! ```text
//! {"runvil/web-search":{"api_key":"...","url":"http://127.0.0.1:52814/v1/tools/web_search","version":1}}
//! ```
//!
//! - `version` is 1. In version 1 a search is one POST of the Gateway's
//!   web search request, as JSON, to `url`, and the reply is the
//!   Gateway's web search reply. The tool checks the version before it
//!   reads anything else of the entry, so an entry of a later version
//!   fails as that.
//! - `url` is the address of the Gateway's web search route.
//! - `api_key` is the Gateway's key, and it is optional. With it, the
//!   request has `Authorization: Bearer <api_key>`. Without it, or with
//!   an empty one, the request has no `Authorization` header at all:
//!   the Gateway admits a caller on its own machine that sends no
//!   credentials, and it answers 401 to any `Authorization` header that
//!   is not its key.
//!
//! A Host with no web search, and a Host whose Gateway is down, leaves
//! the entry out. The Pack is there all the same and its fetch tool
//! works, and a search fails when it is called. Each call reads the
//! entry it was lent, so a Host that moves its Gateway changes where the
//! next search goes.
//!
//! The search client honors the system's proxy settings, which it reads
//! once, when the Plugin loads: the `HTTP_PROXY`, `HTTPS_PROXY`,
//! `ALL_PROXY`, and `NO_PROXY` environment variables, and where those
//! name no proxy, the proxy settings of Windows and of macOS. A proxy
//! that applies to the address in the entry receives the request, and
//! the key in it. The fetch client takes no proxy. The search client
//! also follows a redirect, as its HTTP library does when it is told
//! nothing else: at most 10 of them, and without the key once one leaves
//! the scheme, the host, or the port of the address in the entry.
//!
//! # Where a request runs
//!
//! The Host polls a tool call on a thread of its own, and that thread
//! has no context of the Plugin's tokio runtime. A library Plugin links
//! its own copy of tokio, so the Host's runtime is no help to it. An
//! HTTP client needs that context for its sockets, its timers, and its
//! host lookups, so a call does not send its request itself. It spawns
//! the fetch or the search as a task on the Plugin's runtime and awaits
//! the task's handle, which needs no context.
//!
//! While the request is in flight, the call is parked: its step ended
//! pending, and the Host sends the next step once the task ends, which
//! wakes the call. A call the Host abandons drops its future, which
//! aborts the task, and the connection closes with the task.
//!
//! The runtime has two worker threads, which drive the sockets and the
//! timers of every request, and it starts more threads for blocking
//! work. Two things run on those: a host name is resolved there, and an
//! HTML page is rendered to markdown there. Rendering only computes, and
//! a large page takes seconds of it, so on a worker it would stall every
//! other request. A lookup or a rendering that has started cannot be
//! aborted, so it runs to its end even when its call is dropped.
//! Dropping the Plugin instance drops the runtime, which waits for every
//! one of its threads, so an unload returns only when each lookup and
//! each rendering that is in flight has ended.
//!
//! ## Invariants
//!
//! - Depends on `runvil-plugin-sdk` and outside libraries alone, and
//!   writes no `unsafe`.
//! - The `dll` feature exports `runvil_plugin`. A Host that links the
//!   crate in turns the feature off and hands
//!   `runvil_plugin_sdk::entry::<Web>` to the Harness.
//! - The fetch policy checks every URL that a model or a redirect names
//!   before a request goes to it, and every address a host name resolves
//!   to, on every hop. An address that is not public is refused unless
//!   the policy grants that exact host and address. A load uses the
//!   default policy alone, which grants none.
//! - No fetch sends an ambient identity on any hop. The fetch client has
//!   no proxy, no cookie store, no automatic `Referer` header, and no
//!   default credentials.
//! - No output and no failure names the address a host name resolved to,
//!   or the range that blocks it.
//! - The Plugin holds no key of a search vendor. A search is posted to
//!   the address in the `runvil/web-search` entry of the Host's
//!   resources, and under resources with no entry of version 1 a call
//!   sends nothing.
//! - The Gateway's key is never printed. The Plugin writes it into the
//!   `Authorization` header of a search and into nothing else: no
//!   output, no failure, and no `Debug` text. No failure quotes the
//!   Host's resources. `gateway.rs` alone takes the key out of them, and
//!   the fields it keeps the key in are private to it.
//! - Every output of both tools is untrusted.
//! - The Plugin instance owns a tokio runtime with two worker threads,
//!   with I/O and time on, and every request, a fetch and a search
//!   alike, runs as a task on it. Dropping the instance drops the
//!   runtime, which joins its threads, so an unload leaves no thread
//!   behind.
//! - A call awaits the task of its request, and dropping the call aborts
//!   the task.
//! - An HTML page is rendered, and a host name is resolved, on a thread
//!   the runtime starts for blocking work, never on one of its two
//!   workers, which drive the sockets and the timers of every request.
//! - Production code keeps nothing in a `static`, so an unload frees
//!   everything the Plugin holds. The table of blocked ranges is a
//!   constant.
//! - `load` refuses when the fetch client, the search client, or the
//!   runtime cannot be built, and it never panics.

mod address;
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "the Plugin is handed no configuration, so only the crate's tests build a custom fetch policy"
    )
)]
mod config;
mod error;
mod fetch;
mod gateway;
#[cfg(test)]
mod mock_gateway;
mod redirect;
mod resolver;
mod response;
mod search;
mod task;
mod url_policy;
mod web;

pub use crate::web::Web;

#[cfg(feature = "dll")]
runvil_plugin_sdk::export_plugin!(Web);
