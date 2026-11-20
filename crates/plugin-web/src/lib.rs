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
//! to no address the policy allows. A call whose fetch task panics fails
//! with `web_fetch: the fetch task did not finish`. Anything else that
//! goes wrong is an untrusted output that says so, such as an HTTP error
//! status, a timeout, a refused redirect, a body over the size cap, or a
//! type the tool does not render. The model reads it and can try another
//! URL.
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
//! # Where a fetch runs
//!
//! The Host polls a tool call on a thread of its own, and that thread
//! has no context of the Plugin's tokio runtime. A library Plugin links
//! its own copy of tokio, so the Host's runtime is no help to it. The
//! HTTP client needs that context for its sockets, its timers, and its
//! host lookups, so a call does not send its request itself. It spawns
//! the fetch as a task on the Plugin's runtime and awaits the task's
//! handle, which needs no context.
//!
//! While the fetch is in flight, the call is parked: its step ended
//! pending, and the Host sends the next step once the task ends, which
//! wakes the call. A call the Host abandons drops its future, which
//! aborts the task, and the connection closes with the task.
//!
//! The runtime has two worker threads, and it starts more for blocking
//! work: a host name is resolved on one of those. A page is rendered to
//! markdown inside its fetch task, on a worker thread, so a large page
//! holds one of the two workers while it renders. Dropping the Plugin
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
//! - The fetch policy checks every URL that a model or a redirect names
//!   before a request goes to it, and every address a host name resolves
//!   to, on every hop. An address that is not public is refused unless
//!   the policy grants that exact host and address. A load uses the
//!   default policy alone, which grants none.
//! - No fetch sends an ambient identity on any hop. The client has no
//!   proxy, no cookie store, no automatic `Referer` header, and no
//!   default credentials.
//! - No output and no failure names the address a host name resolved to,
//!   or the range that blocks it.
//! - Every output of the tool is untrusted.
//! - The Plugin instance owns a tokio runtime with two worker threads,
//!   with I/O and time on, and every fetch runs as a task on it.
//!   Dropping the instance drops the runtime, which joins its threads,
//!   so an unload leaves no thread behind.
//! - A call awaits the task of its fetch, and dropping the call aborts
//!   the task.
//! - Production code keeps nothing in a `static`, so an unload frees
//!   everything the Plugin holds. The table of blocked ranges is a
//!   constant.
//! - `load` refuses when the fetch client or the runtime cannot be
//!   built, and it never panics.

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
mod redirect;
mod resolver;
mod response;
mod url_policy;
mod web;

pub use crate::web::Web;

#[cfg(feature = "dll")]
runvil_plugin_sdk::export_plugin!(Web);
