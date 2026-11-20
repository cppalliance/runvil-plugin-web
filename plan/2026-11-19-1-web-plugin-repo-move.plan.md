---
name: Web Plugin Repo Move
overview: Move the web Pack out of runvil-harness into runvil-plugin-web as a DLL Plugin, with its git history. To make that possible, the contract hands every tool call a JSON text of Host resources and a waker, and a Host fills those resources with one call for the Gateway. A call that waits on the network is parked and not stepped over and over. Host services are erased, and mcp takes the runtime as a plain argument.
todos:
  - id: contract
    content: "runvil-plugin-api: add `resources` and `waker` (the new `RvWaker`) to `RvToolCallArgs`; `RV_PENDING` parks a call; rewrite the contract rules the waker changes; versions to 0.6; regenerate the header; update the layout tests in the crate and under `tests/c-layout`; fill the new fields wherever the SDK builds the struct, so the workspace builds"
    status: completed
  - id: sdk
    content: "runvil-plugin-api: `ToolCall::resources` over a private type whose `Debug` prints nothing; the SDK polls with a real `Waker` over the Host's `RvWaker`; `next_step` wakes itself; a step that leaves a file answer untaken with no request at the Host wakes the call; the fake Host gets `PackRun::with_resources` and `Call::wait`; README, AGENTS, and doc text; tests that fail when a wake is missing; a timeout on the CI test job"
    status: completed
  - id: web-history
    content: "runvil-plugin-web: filter a scratch clone of runvil-harness to the web code and its three old plans with `git filter-repo`, pull the result into the empty repo, then add the repo files from runvil-plugin-bashkit and the dylint, cargo-deny, nextest, and clippy pieces from runvil-harness"
    status: completed
  - id: web-proof
    content: "runvil-plugin-web: a skeleton `Web` Plugin with its own tokio runtime and reqwest linked builds as a DLL on all three systems in CI, and an example run by hand loads the built DLL and fetches one `https://` page"
    status: completed
  - id: loader
    content: "runvil-harness, one commit: pin `runvil-plugin-sys` 0.6; `HostResources` beside the loader, re-exported as `runvil::harness::HostResources`; the two insert functions take it; `Steps` lends the text and a waker slot that the `Plugin` owns until unload; a pending call parks; the loader's test `block_on` waits for a real wake; the test Plugin gets the new fields, a `steps` tool that wakes, and a tool that returns the resources; a test for every wake timing; `public-api.txt`"
    status: completed
  - id: gateway-helper
    content: "runvil-harness: `provide_gateway_resources` in `harness-gateway-client`, with a test that pins the entry's JSON"
    status: completed
  - id: web-fetch
    content: "runvil-plugin-web: port the fetch tool, its policy modules, and their tests onto the SDK; `load` refuses where a client cannot be built; the blocked networks table leaves its `static`; each `ToolError` message is today's model text, word for word, and causes are dropped"
    status: completed
  - id: web-search
    content: "runvil-plugin-web: port the search tool; read `runvil/web-search` from `call.resources()`; move the Gateway POST files into the crate under new names and run the POST on the Plugin's runtime; one request type and one results type; plain `#[test]` tests through the fake Host for a search, an absent entry, another version, and an entry with no key"
    status: completed
  - id: harness-remove
    content: "runvil-harness, after `web-history`: delete `crates/plugin-web`, `GatewaySearch`, the `build-xtask` rule that lets the Gateway client name a Plugin crate, and the workspace dependencies only they used; fix the xtask tests, README, `tools/docs-claims.mjs`, and the `decisions` pages; move issue 23 to the new repo"
    status: completed
  - id: host-services
    content: "runvil-harness, after `harness-remove`: erase `HostServices`, `ServiceKey`, `ServiceId`, and `TOKIO_RUNTIME` with no shim and no comment that names them; `plugin_mcp::install` takes the server list and a tokio `Handle`; add the crate doc that says why mcp is compiled into the Host; rewrite every doc and test that named a service; finish with the search"
    status: completed
  - id: repin-plugins
    content: "runvil-plugin-std: pin `runvil-plugin-sdk` at 0.6. runvil-plugin-bashkit: pin it, wake at the poll cap in `session.rs`, make the `stepped` test helper step then wait, add a test that a `sleep` parks, and fix the docs that state the old rule. Both: a timeout on the CI test job"
    status: completed
  - id: workshop
    content: "runvil-workshop, one commit: every pin; the `workshop-plugin-web` crate and its registrations; `LIBRARIES`, the installer and bundle lists, `deny.toml`, `HARNESS_PUBLIC` and its tests; `agents.rs` builds `HostResources`, hands mcp the runtime, and drops `plugin_web::install`, `GatewaySearchProvider`, and `services()`; the server tests that need the web library"
    status: completed
  - id: runvil-installer
    content: "runvil, one commit after the Workshop commit is on `origin`: add web to the Plugin lists in `cell.ps1` and `cell.sh`, and move the Workshop pin"
    status: completed
  - id: end-to-end
    content: "Run the check at the end: `--plugins` lists web, a real search and fetch work, CPU stays idle during a slow fetch and a bashkit `sleep 5`, and with the Gateway stopped search fails and fetch still works"
    status: completed
  - id: docs
    content: "runvil-docs: chapter 13's two sections on the search provider and the unavailable Pack, and chapters 4 and 12 where they name a Host-wide service; runvil-classroom: the plugin architecture tutorial; the Workshop design page that lists the pinned crates, wherever it lives now"
    status: completed
isProject: false
---

# Move the web Pack to runvil-plugin-web

## What exists today

- **Who loads Plugins.** The loader is `harness-plugin-loader`, a private crate of the Harness in [runvil-harness/crates/harness-internal/plugin-loader](runvil-harness/crates/harness-internal/plugin-loader). A Host calls it through `runvil::harness::insert_plugin_dir` (a folder of DLLs) or `runvil::harness::insert_plugin` (compiled into the Host). It is the only code that calls a Plugin's entry point.
- **What a DLL gets from the Host.** On each tool call: the tool name, the arguments as JSON text, and the file functions. Nothing else, and nothing when it loads.
- **What the two Rust Plugins get.** web and mcp are `install` functions. Each receives a `config` value and `HostServices`, a map of Rust objects keyed by names such as `runvil/search-provider`. A Rust object cannot cross to a DLL, so this is no help.
- **How a call waits.** `RV_PENDING` means "step me again". A call that waits on a file request is parked until the Host has the answer. A call that waits on anything else is stepped again right away, over and over.

## The whole API change, in one place

Added:

- **Contract:** `RvToolCallArgs::resources`, `RvToolCallArgs::waker`, and the struct `RvWaker`.
- **SDK:** `ToolCall::resources`. In the fake Host, `PackRun::with_resources` and `Call::wait`.
- **Harness:** `HostResources::new` and `HostResources::provide`.
- **Gateway client:** `provide_gateway_resources`.

Changed:

- **Contract:** what `RV_PENDING` means, and the version, 0.6.
- **Harness:** `insert_plugin_dir` and `insert_plugin` take a `&HostResources`.
- **mcp:** `plugin_mcp::install` takes the server list and a tokio `Handle`.

Removed:

- `HostServices`, `ServiceKey`, `ServiceId`, and `TOKIO_RUNTIME`.
- `GatewaySearch`.
- The harness crate `plugin-web`, with its ten public items: `install`, `SEARCH_PROVIDER`, `SearchProvider`, `SearchQuery`, `SearchResults`, `SearchResult`, `SearchError`, `SearchErrorKind`, `Freshness`, and `SafeSearch`.

## Where each new piece goes

- The two new contract fields and `RvWaker`: `runvil-plugin-sys` (repo `runvil-plugin-api`).
- The getter a Plugin author calls: `runvil-plugin-sdk` (same repo).
- The map a Host fills: `HostResources`, beside the loader, which a Host sees as `runvil::harness::HostResources`.
- Handing that map to DLLs: the loader's `insert_plugin_dir` and `insert_plugin`.
- The Gateway convenience: `harness-gateway-client`. No new repo.
- The waker: the struct and its field in `runvil-plugin-sys`, the Rust `Waker` over it in `runvil-plugin-sdk`, and the parking in the loader.
- The web Pack: `runvil-plugin-web`.

```mermaid
flowchart LR
    Host[Host] -->|"provide_gateway_resources"| Res[HostResources]
    Res -->|"read when a call starts"| Loader[Loader]
    Loader -->|"resources text"| Args[RvToolCallArgs]
    Args -->|"copied once per call"| Call[ToolCall]
    Call -->|"call.resources()"| Search[web search tool]
    Search -->|"POST url with Bearer api_key"| Gateway[Gateway]
```

## 1. The contract: every tool call is lent the Host's resources

File: [runvil-plugin-api/crates/runvil-plugin-sys/src/lib.rs](runvil-plugin-api/crates/runvil-plugin-sys/src/lib.rs)

**Today.** `RvToolCallArgs` gives a step the tool name, the arguments, `finish`, and the nine file functions. No struct in the contract holds anything else from the Host.

**After.** `RvToolCallArgs` ends with two more fields: the resources text and the call's waker. Every Plugin gets both, and one that does not care never reads them. Section 9 has the waker's rules. No number goes with the text: a Plugin that wants to keep what it parsed compares the text with the last one it saw. The contract becomes 0.6, so the constant and both crate versions move, and the header is regenerated with `cargo xtask header`. The layout checks change in both places that hold them: the unit tests in the crate, and the two hand-kept tables under `tests/c-layout`. The struct grows from 136 to 168 bytes on 64-bit targets.

```rust
pub const RV_API_MINOR: u32 = 6;

#[repr(C)]
pub struct RvToolCallArgs {
    // ... every field of 0.5, unchanged, through `file_copy` ...

    /// The Host's resources as they were when the call started: the JSON
    /// text of an object with one key for each resource the Host
    /// provides, such as `runvil/web-search`, and `{}` when it provides
    /// none. Every step of one call gets the same text. Like `tool` and
    /// `arguments`, it is valid for the step.
    pub resources: RvStr,
    /// The waker of this call. Every step of one call gets the same one.
    /// Section 9 has `RvWaker` and its rules.
    pub waker: RvWaker,
}
```

## 2. The SDK: one getter on `ToolCall`

Files: [runvil-plugin-api/crates/runvil-plugin-sdk/src/tool.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/tool.rs), [runvil-plugin-api/crates/runvil-plugin-sdk/src/entry/tool.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/entry/tool.rs), [runvil-plugin-api/crates/runvil-plugin-sdk/src/testing.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/testing.rs)

**Today.** `ToolCall` has `tool()`, `arguments()`, and the file methods. The SDK copies the tool name and the arguments once, on a call's first step.

**After.** The Host's text is valid for one step only, so on a call's first step the SDK copies it into the call's own state. The new getter lends from that copy, never from the Host's memory. The borrow ends with the call, so the compiler stops a tool from keeping the text longer, and a tool that wants to must copy or parse it. A text that is not UTF-8 is a misuse, as it is for `tool` and `arguments`. The SDK still parses no JSON, so a Plugin parses the text with its own library. The text can hold the Gateway's key, so it sits in one private type whose `Debug` prints none of it. That one type covers `CallState`, `ToolCall`, and the fake Host's `PackRun` and `Call`.

```rust
/// The Host's resources text. Its `Debug` prints none of it, because
/// the text can hold a key.
struct Resources(String);

/// What one call keeps for all of its steps, in the SDK's own memory.
pub(crate) struct CallState {
    pub(crate) tool: String,
    pub(crate) arguments: String,
    pub(crate) resources: Resources,
    pub(crate) requests: Requests,
}

impl<'a> ToolCall<'a> {
    /// The Host's resources as they were when the call started: the JSON
    /// text of an object keyed by resource name, `{}` when there are none.
    pub fn resources(&self) -> &'a str;
}

// testing, the fake Host
impl PackRun<'_> {
    /// The instance with `json` as the Host's resources for every call
    /// made from here on.
    pub fn with_resources(self, json: impl Into<String>) -> Self;
}
```

How a Plugin reads it:

```rust
let resources: MyResources = serde_json::from_str(call.resources())?;
```

## 3. What a Host fills: `HostResources`

File: new `resources.rs` in [runvil-harness/crates/harness-internal/plugin-loader/src](runvil-harness/crates/harness-internal/plugin-loader/src), re-exported in [runvil-harness/crates/runvil/src/lib.rs](runvil-harness/crates/runvil/src/lib.rs) under `harness`

**Today.** `HostServices` is the one map a Host fills for its Plugins. It stores Rust objects and finds them by type, and only the `install` functions of web and mcp read it. Section 10 erases it.

**After.** `HostResources` is what a Host fills for DLL Plugins: JSON values under `vendor/name` names. It has one way to offer a value, which is a function the loader calls each time a tool call starts. A Host whose value never changes passes a function that returns it. It is a plain value with no lock: a Host fills it, then hands it to the loader. It lives beside the loader, which is its only reader, so the function that reads it is not public. Nothing in it or its docs refers to `HostServices`.

```rust
/// The resources a Host offers every Plugin it loads through the C
/// contract: JSON values under `vendor/name` names. A clone shares the
/// functions. Its `Debug` lists the names and never a value.
#[derive(Clone, Default)]
pub struct HostResources { /* ... */ }

impl HostResources {
    pub fn new() -> HostResources;

    /// Offers what `read` returns under `name`. The loader calls `read`
    /// each time a tool call starts, on the run's own task, so it
    /// returns at once. `None` leaves the resource out of that call.
    ///
    /// # Errors
    /// Refuses a `name` that is not a vendor and a name joined by `/`,
    /// and a `name` that is offered already.
    pub fn provide(
        &mut self,
        name: &str,
        read: impl Fn() -> Option<Value> + Send + Sync + 'static,
    ) -> Result<(), Error>;
}
```

Three choices here:

- One method, not a fixed form beside a function form (medium confidence: Workshop, the only production caller, needs the function form, and a fixed value is one closure).
- Values are read when a call starts, not pushed by the Host on change (high confidence: Workshop's broker and search provider already read the Gateway on every use, so it needs no background task).
- Beside the loader, not in `runvil-plugin` (high confidence: only the loader reads it, and `runvil-plugin` is the Rust Plugin API, which a DLL never sees).

## 4. The loader hands the resources to every DLL

Files: [runvil-harness/crates/harness-internal/plugin-loader/src/plugin.rs](runvil-harness/crates/harness-internal/plugin-loader/src/plugin.rs), [runvil-harness/crates/harness-internal/plugin-loader/src/call.rs](runvil-harness/crates/harness-internal/plugin-loader/src/call.rs), [runvil-harness/crates/harness-internal/test-plugin](runvil-harness/crates/harness-internal/test-plugin), [runvil-harness/Cargo.toml](runvil-harness/Cargo.toml) (the `runvil-plugin-sys` pin)

**Today.** The two insert functions take the registry and a directory or an entry point. `Steps`, the future of one tool call, sends each step the tool name and the arguments.

**After.** Both functions take the Host's resources, and each loaded `Plugin` keeps a clone. When a tool call starts, `Steps::new` builds the text once, and every step of that call is lent that same text.

```rust
pub fn insert_plugin_dir(
    packs: &mut PackRegistry,
    directory: &Path,
    resources: &HostResources,
) -> io::Result<PluginReport>;

pub fn insert_plugin(
    packs: &mut PackRegistry,
    entry: PluginEntry,
    resources: &HostResources,
) -> Result<Arc<Plugin>, Error>;
```

Inside the loader:

```rust
// Steps::new, once per tool call
let resources = pack.plugin().resources().text();

// Steps::step, on every step
let args = RvToolCallArgs {
    // ...
    resources: rv_str(&self.resources),
    waker: self.parked.waker(),
};
```

What goes with it:

- **One commit.** Once the pin is 0.6, the struct literal in `Steps::step` must fill both new fields. So the resources and the waker of section 9 land together, with every `0.5` in the test Plugin, the loader's fakes, their tests, and the comments.
- **The field limit.** `Steps` goes from 9 to 11 named fields, and the structural check allows 12.
- **The test Plugin.** It keeps its own copy of the contract, so its `RvToolCallArgs` mirror gains the two fields. It also gains one tool that answers with the resources text, which is how a real DLL proves the text arrives.
- **A `read` that panics.** It drops that one call, because a call starts inside the runner's `catch_unwind`.
- **Call sites.** 14 test call sites in this repo and 3 call sites in Workshop gain the argument.
- **`public-api.txt`.** It is updated with the bless flag of `cargo xtask api`, on the pinned nightly, with `CARGO_BUILD_WARNINGS` unset. A plain run only prints the difference.

## 5. The Gateway convenience for Host authors

File: new `resources.rs` in [runvil-harness/crates/harness-gateway-client/src](runvil-harness/crates/harness-gateway-client/src), exported from its `lib.rs`

**Today.** A Host that wants web search builds `GatewaySearch::new(endpoint, key)` and puts it in `HostServices` under `SEARCH_PROVIDER`, a key that `plugin-web` defines. So the Gateway client depends on the web Plugin.

**After.** A Host makes one call. It passes a function that returns the Gateway to use right now, as the endpoint and key types it already gives `GatewayBroker::new`. The call registers every resource the Gateway backs, so when the Gateway gains a second feature, no Host changes. It lives in this crate because it is the only crate that can read the key out of `SecretString`.

```rust
/// Offers `resources` everything the Gateway serves to Plugins.
/// `current` is asked as each tool call starts. It returns the Gateway
/// to use, or `None` when there is none right now, which leaves the
/// Gateway's resources out of that call.
pub fn provide_gateway_resources(
    resources: &mut HostResources,
    current: impl Fn() -> Option<(GatewayEndpoint, SecretString)> + Send + Sync + 'static,
) -> Result<(), Error>;
```

Everything a Host author writes, for a Gateway that stays where it is:

```rust
let mut resources = HostResources::new();
provide_gateway_resources(&mut resources, move || Some((endpoint.clone(), key.clone())))?;
let report = insert_plugin_dir(&mut packs, plugin_dir, &resources)?;
```

The entry that function writes:

```json
"runvil/web-search": {
  "version": 1,
  "url": "http://127.0.0.1:52814/v1/tools/web_search",
  "api_key": "..."
}
```

- `version` 1 means: POST the Gateway's web search request as JSON to `url`, and the reply is the Gateway's web search reply.
- The version is in the entry and never in the request body. The Gateway refuses a body that holds a field it does not know.
- `api_key` is optional. With it, the request has `Authorization: Bearer <api_key>`. Without it, the request has no `Authorization` header at all, which a Gateway on the same machine accepts by default. An empty or wrong bearer gets a 401 even from the same machine, so "no key" has to mean "no header". The function above always writes a key, as `GatewayBroker::new` always takes one. A Host with no key is issue [#32](https://github.com/cppalliance/runvil-harness/issues/32).
- The name and the shape are written in two repos, because a Plugin may not depend on a Harness crate. The name is a private constant on each side, and a test on each side pins this same JSON.

## 6. The web Plugin in its own repo

Repo: `runvil-plugin-web`. It exists at `cppalliance/runvil-plugin-web` and is empty, and the local clone's `origin` points at it.

**Today.** `plugin-web` in [runvil-harness/crates/plugin-web](runvil-harness/crates/plugin-web) is an `install` function written against `runvil-plugin`. It runs each fetch on the Host's tokio runtime, which it reads from `HostServices`, and it runs each search through the Host's `SearchProvider` object.

**After.** It is a DLL Plugin written against `runvil-plugin-sdk`. It owns a small tokio runtime, as the bashkit Plugin does, because a DLL links its own copy of tokio and cannot use the Host's. The search tool reads the Gateway's address and key from the call's resources and posts to the Gateway itself.

```rust
/// The Plugin instance: what every run shares.
pub struct Web {
    /// The hardened client every fetch goes through, built once.
    fetch: FetchClient,
    /// The client every search goes through.
    search: SearchClient,
    /// Runs every request, fetch and search alike: a call spawns its
    /// work here and awaits the task. Two worker threads, with I/O and
    /// time on. Dropping the instance drops it, which joins its
    /// threads, so an unload leaves no thread behind.
    requests: tokio::runtime::Runtime,
}

impl Plugin for Web {
    const NAME: &'static str = "web";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");
    const PACKS: &'static [PackInfo] =
        &[PackInfo::new("runvil/web").description("Search the web and fetch pages")];

    /// Refuses when a client or the runtime cannot be built.
    fn load() -> Result<Web, Refusal>;
    fn new_pack(&self, pack: &str) -> Result<Box<dyn Pack>, Refusal>;
}

#[cfg(feature = "dll")]
runvil_plugin_sdk::export_plugin!(Web);

/// One run's instance: the tools `fetch` and `search`. Its `call` parses
/// the arguments once and hands each tool a `Value`, as today.
struct WebPack { fetch: WebFetch, search: WebSearch }

impl WebFetch {
    async fn call(&self, args: Value) -> Result<ToolOutput, ToolError>;
}

impl WebSearch {
    async fn call(&self, args: Value, resources: &str) -> Result<ToolOutput, ToolError>;
}

/// The `runvil/web-search` entry of the Host's resources. `api_key` is
/// optional: without it the request has no `Authorization` header.
#[derive(serde::Deserialize)]
struct WebSearchResource { version: u32, url: String, api_key: Option<String> }

impl WebSearchResource {
    /// Reads the entry out of `resources`, checking `version` first.
    /// Fails with a message a model can read when the entry is absent
    /// or is not version 1. The message never quotes the text.
    fn read(resources: &str) -> Result<WebSearchResource, ToolError>;
}
```

Where the resources reach the tool:

```rust
// WebPack::call
"search" => self.search.call(args, call.resources()).await,

// WebSearch::call
let gateway = WebSearchResource::read(resources)?;
```

### How the repo is made

1. **History.** A scratch clone of `runvil-harness` at `master` is filtered with `git filter-repo`, which is installed here, and the result is pulled into the empty local repo. The filter keeps every directory the crate has lived in, and the two files the tools started in, so the history reaches back to 2026-07-29. Filtering to `crates/plugin-web` alone would stop at its last rename, on 2026-10-07. At the tip, `crates/plugin-web` is the only one of those directories left, because each older one was renamed away. The harness has no tags, so none come along. The first commit on top names the harness commit the clone was taken at.
2. **Old plans.** Three old plans whose subject is the web tools come along from `vibe/` and land in `plan/`, flat, the way the bashkit repo keeps its plans: `brave-web-search-tools`, `webfetch-crate-extraction`, and `webfetch-soft-errors`. Harness plans that only mention web stay in the harness.
3. **The Gateway search files.** `search.rs` of `harness-gateway-client` and its three test files come along too, so the code this plan moves into the crate keeps its history. They are the one thing kept from outside the crate's own directories.
4. **Repo files**, in commits on top of the history:
   - From `runvil-plugin-bashkit`, the same kind of repo: the root `Cargo.toml` with its lint tables, `crates/build-checks`, `rustfmt.toml`, `rust-toolchain.toml`, `.cargo/config.toml`, `.gitattributes`, `.gitignore`, `LICENSE`, and the CI jobs for format, clippy, docs, and the test and release build on three systems with the check that the library exports `runvil_plugin`.
   - `AGENTS.md` from bashkit too, with the harness's wording on test file names. This crate splits big test files by topic, such as `tests-body.rs`, and bashkit's wording allows only `foo-tests.rs`.
   - From `runvil-harness`, because web has what they guard:
     - dylint: the `[workspace.metadata.dylint]` table and its CI job.
     - `deny.toml` and the `supply-chain` job, with its check for one rustls crypto backend. Web has the large dependency tree those are for.
     - The `clippy.toml` list of banned process-global installers. Web links rustls, log, and tracing through reqwest.
     - `.config/nextest.toml` and `cargo nextest run`, because the crate's tests run under nextest today, and many of them start servers and wait on timeouts.
     - The `ci-green` job, which gates on every other job.
   - A timeout on the CI test job, so a test that hangs fails in minutes.
   - Left behind: the wording scan, the API surface job, the toolchain floor job, and the git hooks. Each checks something only the harness has.

The filter. The directory list comes from following the crate's files back through their renames, so check it again before running. It selects about 120 commits. Run it in Git Bash, or put it on one line:

```
git filter-repo \
  --path crates/plugin-web --path crates/harness-web \
  --path crates/harness-internal/web --path crates/harness-internal/webfetch --path crates/harness-internal/web-search \
  --path crates/harness/web --path crates/harness/webfetch --path crates/harness/web-search \
  --path crates/promptforge/web --path crates/promptforge/webfetch --path crates/promptforge/web-search \
  --path crates/promptforge-webfetch \
  --path crates/promptforge-core/src/tools/web_fetch.rs \
  --path crates/promptforge-core/src/tools/web_search.rs \
  --path-glob 'crates/harness-gateway-client/src/search*.rs' \
  --path-glob 'vibe/*brave-web-search-tools.md' \
  --path-glob 'vibe/*webfetch-crate-extraction.md' \
  --path-glob 'vibe/*webfetch-soft-errors.md' \
  --path-rename vibe/2026-07/:plan/ --path-rename vibe/2026-08/:plan/ --path-rename vibe/:plan/
```

### The first proof, before anything is ported

- A skeleton `Web` Plugin, with its own tokio runtime and reqwest linked, builds as a DLL on all three systems in CI. The bashkit build settings link the C runtime statically with `rust-lld`, and this is the first DLL to link a TLS stack that way.
- An example that is run by hand loads the built DLL with `libloading`, through the SDK's `testing::load_entry`, and fetches one `https://` page. It is the one place in the repo with `unsafe`, under an `#[expect]` that says why.
- No DLL in the workspace links a TLS stack today, so this comes before 5,600 lines are changed.

### What changes in the crate once it is there

- **Unchanged:** `url_policy.rs`, `redirect.rs`, `response.rs`, and their tests. None of them names a Harness type.
- **`address.rs`.** Its table of blocked networks is a `static` today. The Plugin repos keep nothing in a `static`, so that an unload frees everything, and the parsed table moves into `FetchConfig`.
- **`config.rs` and `search.rs`.** Each wires a second production file with `#[path]` (`config-validate.rs`, `search-request.rs`), which the Plugin repos do not allow. Each pair merges back into one file.
- **Error type swapped** from `runvil_plugin::Error` to the SDK's `ToolError`: `fetch.rs`, `error.rs`, `search.rs`. Each `ToolError` message is the text the model reads today, word for word, because the language guide quotes those texts. A cause that `with_cause` adds today goes to the log text only, so it is dropped and not added to the message.
- **`resolver.rs`.** It takes `BoxFuture` from the SDK and loses its debug log, the crate's one use of `tracing`.
- **No panic in `load`.** `FetchClient::new`, which panics when the client cannot be built, goes. `load` calls `FetchClient::try_with_config` with the default policy and refuses on failure. The search client is built the same way, not with `reqwest::Client::new()`, which also panics.
- **Search runs on the Plugin's runtime.** Today the Gateway POST is awaited inline. In a DLL that panics, because the Host thread that polls the call has no tokio context of the Plugin's. The POST is spawned on `requests` and aborted when the call is dropped, with the helper fetch already has.
- **One request type and one results type.** With the provider gone, nothing sits between the tool and the Gateway. `SearchRequest` serializes straight to the Gateway's request, and one results type reads the reply and writes the tool's output. `SearchQuery`, the three Gateway wire types, and the two mapping functions go. Three things stay as they are: an empty domain list is left out of the request, the fields keep their order, and `age`, `site_name`, and `extra_snippets` are left out of a result when they are empty.
- **The Gateway files get new names when they move in,** because the crate already has a `search.rs` and a `search-tests.rs`.
- **Rewritten:** `web.rs` becomes the `Plugin` and `Pack` impls. `Web::new`, `with_fetch_config`, and the configuration check go with it.
- **Gone:** the `SearchProvider` trait, `SearchError`, and `SEARCH_PROVIDER`.
- **No key in any output:** `WebSearchResource` has no `Debug` that prints the key.

New messages, which have no text today:

- No entry in the resources: `web_search: the host offers no web search right now`, as a transport failure. This covers a Host with no search and a Gateway that is down.
- An entry that is not version 1: `web_search: the host's runvil/web-search entry is not version 1`, as an other failure.
- A search task that was lost: `web_search: the search task did not finish`, as an other failure, the twin of the fetch message.

### Tests

The crate has 91 tests and the Gateway search files have 15.

- **Move unchanged (32):** `address.rs`, `config-tests.rs`, `redirect.rs`, `response.rs`, `url_policy.rs`.
- **One-line swaps (40):** `error.rs`, `resolver.rs`, and the three fetch test files. The fetch tests build `WebFetch` directly on `Handle::current()`, so they keep `#[tokio::test]`.
- **Rewritten (9):** `search-tests.rs`. It is built on a fake `SearchProvider` and asserts on harness error fields. It becomes tests against a mock server through the entry.
- **Rewritten on the fake Host (10):** `web-tests.rs`. Six have no twin and go: the two configuration tests, the missing service test, the duplicate Pack test, the service key test, and the test of a tool the Pack does not offer, which the fake Host refuses to start.
- **Gateway tests (15):** twelve adapt, since they assert on harness error fields and causes. The three that test the provider boundary go.
- **Fake Host tests are plain `#[test]`, never `#[tokio::test]`.** `PackRun::call` blocks its thread, a surrounding tokio runtime would hide a request that was not spawned, and dropping the Plugin inside an async test panics in tokio. The mock server runs on a runtime the test owns.
- **New:** one plain test that runs a search through the fake Host, which is the test that fails if the POST is not spawned. One each for an absent entry, an entry of another version, and an entry with no key, which asserts that the request has no `Authorization` header.

## 7. Workshop

Files: [runvil-workshop/Cargo.toml](runvil-workshop/Cargo.toml), [runvil-workshop/crates/workshop/server/src/agents.rs](runvil-workshop/crates/workshop/server/src/agents.rs), [runvil-workshop/crates/build-workshop/src/main/plugin.rs](runvil-workshop/crates/build-workshop/src/main/plugin.rs), [runvil-workshop/crates/build-xtask/src/tidy.rs](runvil-workshop/crates/build-xtask/src/tidy.rs)

**Today.** Workshop calls `plugin_web::install` with a `GatewaySearchProvider` that re-reads the Gateway on every search. It ships two Plugin DLLs, each built by a small crate (`workshop-plugin-std`, `workshop-plugin-bashkit`) whose only code is one line.

**After.** Web is a third DLL, built by a third such crate and loaded from the Plugin directory. Workshop fills `HostResources` with one function that returns the Gateway it holds right now, with the same checks the search provider makes today.

```rust
// crates/workshop/plugin-web/src/lib.rs: the crate docs, with `## Invariants`, then its one line of code
runvil_plugin_sdk::export_plugin!(web_plugin::Web);

// crates/workshop/server/src/agents.rs
fn resources(registry: &Registry) -> HostResources {
    let mut resources = HostResources::new();
    let registry = registry.clone();
    let _ = provide_gateway_resources(&mut resources, move || reachable_gateway(&registry));
    resources
}

/// The Gateway the registry holds now, when the heartbeat says it is up.
fn reachable_gateway(registry: &Registry) -> Option<(GatewayEndpoint, SecretString)> {
    let handles = registry.state::<GatewayHandles>()?;
    if !handles.health().is_reachable() {
        return None;
    }
    let gateway = UsableGateway::read(&handles)?;
    Some((gateway.endpoint, gateway.key))
}

// load_plugin_dir gains a `resources` parameter
insert_plugin_dir(packs, directory, resources)

// crates/build-workshop/src/main/plugin.rs
pub(crate) const LIBRARIES: [&str; 3] = ["plugin_std", "plugin_bashkit", "plugin_web"];
```

**One commit.** The pins cannot move apart: `runvil-plugin-sys` sets `links`, so Cargo refuses two commits of the contract in one build, and the new harness commit has no `plugin-web` and no `HostServices`. So every pin, the new crate, the packaging lists, and the code in `agents.rs` land together.

Pins and the new crate:

- The root `Cargo.toml`: one `rev` for `runvil-plugin-sdk`, new `rev`s for the harness crates and the std and bashkit Plugins, a `web-plugin` entry with a `version` key like its twins, a `workshop-plugin-web` path entry, and no `plugin-web` entry.
- `crates/workshop/plugin-web`: a manifest like the bashkit twin's, with `[lib] name = "plugin_web"`, and a `lib.rs` whose crate docs hold the `## Invariants` marker. The structural check requires the marker of every Workshop crate, and missing docs are a warning that CI denies.

Where the new crate is registered:

- `members` and `default-members` in the root `Cargo.toml`.
- `PLUGINS` in `crates/build-xtask/src/tidy.rs`. Without it the server may not depend on the crate.
- `PLUGIN_DOC_DIRS` in `crates/workshop/ui/test/docs-claims.mjs`.
- The server's `Cargo.toml`, so that Cargo builds the library.
- `allow-git` in `deny.toml`.

Checks that change:

- `HARNESS_PUBLIC` in [runvil-workshop/crates/build-xtask/src/harness_pin.rs](runvil-workshop/crates/build-xtask/src/harness_pin.rs) drops `plugin-web`. Four tests in `harness_pin-tests.rs` use `plugin-web` in their fixtures or expect its name in a message, and change with it.

Packaging, every place that lists the bashkit library:

- `LIBRARIES`, and the tests in `crates/build-workshop/src/main/tests/plugin.rs` and `tests/bundle.rs` that count or name the libraries.
- `crates/workshop/desktop/installer.nsi`: four lines.
- `.github/workflows/bundle.yml`: the path filter.
- The READMEs, and the desktop crate's `AGENTS.md`.

In `agents.rs`:

- Delete `agents/search.rs` and the `plugin_web::install` call. `services()` goes too, and mcp is handed the runtime directly (section 10).
- `list_plugin_dir` keeps its one parameter and passes an empty `HostResources`.
- `SHIPPED_PACKS` gains `runvil/web`.
- `async-trait` loses its one user and leaves the server's manifest and the root one.

Server tests:

- `agents/search-tests.rs`: its three tests (a replaced Gateway, no registration, the heartbeat down) become tests of `reachable_gateway`.
- `agents/tests.rs`: two tests go (the search provider reason, and the services built outside a runtime). Two tests need a Plugin directory that holds the web library, because web is no longer installed without one. The tests that name the std and bashkit libraries gain a web twin.
- `tests/common/mod.rs`: `PLUGIN_LIBRARIES` gains `plugin_web`. Without it every chat test fails, because the built-in chat agent declares `web`.
- `tests/it/std_real_directory.rs`: one call site gains the argument.
- `tests/it/agents/turns.rs` becomes the one test in CI that goes from the resources, through the DLL, to a Gateway.

The `runvil` repo, in one commit of its own after the Workshop commit is on `origin`:

- [runvil/.github/installers/cell.ps1](runvil/.github/installers/cell.ps1): the Plugin list gains web.
- [runvil/.github/installers/cell.sh](runvil/.github/installers/cell.sh): three loops over `std bashkit` gain `web`.
- The Workshop pin in [runvil/Cargo.toml](runvil/Cargo.toml) moves in the same commit. The install checks run against the pinned Workshop, so a list that gains web before the pin moves fails every one of them.

## 8. What leaves runvil-harness

- The crate [runvil-harness/crates/plugin-web](runvil-harness/crates/plugin-web) and its workspace entry.
- `GatewaySearch`, its three test files, and the `plugin-web` dependency of `harness-gateway-client`, with the crate docs and README that describe them.
- The rule in `crates/build-xtask/src/product.rs` that lets `harness-gateway-client` alone name a Plugin crate, and its test.
- The workspace dependencies only those two used: `readabilityrs`, `htmd`, `mime`, `encoding_rs`, `ipnet`, `flate2`, `async-trait`. The comment on `regex` names `readabilityrs` and changes with them.
- The `plugin-web` line in `tools/docs-claims.mjs`. The script reads every directory it lists, so a missing one makes it throw.
- The mentions in `README.md`, and the `build-xtask` tests that use `plugin-web` as their example.
- `HostServices` and `TOKIO_RUNTIME` go as well, in section 10.
- `Cargo.lock` changes in the same commit, because CI builds locked.
- Nothing here is deleted until the history is in the new repo (section 6).
- Issue [#23](https://github.com/cppalliance/runvil-harness/issues/23), about `web/fetch`, moves to `cppalliance/runvil-plugin-web`.

## 9. Waiting: every tool call gets a waker

Files: [runvil-plugin-api/crates/runvil-plugin-sys/src/lib.rs](runvil-plugin-api/crates/runvil-plugin-sys/src/lib.rs), [runvil-plugin-api/crates/runvil-plugin-sdk/src/entry/tool.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/entry/tool.rs), [runvil-plugin-api/crates/runvil-plugin-sdk/src/tool.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/tool.rs), [runvil-plugin-api/crates/runvil-plugin-sdk/src/testing.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/testing.rs), [runvil-harness/crates/harness-internal/plugin-loader/src/call.rs](runvil-harness/crates/harness-internal/plugin-loader/src/call.rs), [runvil-plugin-bashkit/crates/plugin-bashkit/src/session.rs](runvil-plugin-bashkit/crates/plugin-bashkit/src/session.rs)

**Today.** A tool call is an `async` block that the Host polls one step at a time, and `RV_PENDING` means "step me again". The contract has no waker: the SDK polls the future with `Waker::noop()`, and the loader steps a pending call again as soon as it can. A call that waits on a file request is parked until the Host has the answer. A call that waits on anything of its own, such as a network reply or a timer, is stepped over and over.

**After.** Every step is handed a waker for its call, and `RV_PENDING` means what pending means in Rust: the call is parked. The Host sends the next step after the Plugin wakes the call, or when the answer to its file request is ready. The Host keeps every waker valid until the Plugin unloads, so a wake that comes late is harmless, and no Plugin can crash the process with one. The SDK wraps the waker in an ordinary Rust `Waker`, so a Plugin author writes the same `async` code as today and it parks properly.

The contract:

```rust
/// The tool call is parked. The Host sends its next step after the
/// Plugin wakes the call, or, when the step made a file request, when
/// the answer is ready. A wake during the step counts, so a step that
/// wants the next one right away calls `wake` and then returns this. A
/// step may also come that no wake asked for, but never while a file
/// request waits for its answer. Only `RV_TOOL_CALL` returns it.
pub const RV_PENDING: RvStatus = 7;

/// Wakes one tool call. The Plugin may call `wake` from any thread, any
/// number of times, from the call's first step until
/// `RV_PLUGIN_UNLOAD` returns. The Host keeps what `ctx` points at
/// valid until then. A wake after its call has ended does nothing for
/// that call.
#[repr(C)]
#[derive(Clone, Copy, Debug)]
pub struct RvWaker {
    /// The Host's context for the waker, passed back to `wake`. The
    /// Plugin never reads through it.
    pub ctx: *mut c_void,
    /// Asks the Host to send the call's next step. It only notes the
    /// wake and returns: it calls nothing of the Plugin's and does not
    /// block, so a Plugin may call it while it holds a lock.
    pub wake: unsafe extern "C" fn(ctx: *mut c_void),
}

// A step gets it as `RvToolCallArgs::waker`, shown in section 1.
```

The SDK. A Plugin author sees no new item:

```rust
// entry/tool.rs
/// The Host's waker for one call, in a form a Rust `Waker` can share
/// between threads. It needs no lock: the Host keeps the waker valid
/// until the Plugin unloads.
struct CallWaker(RvWaker);

impl std::task::Wake for CallWaker {
    /// Calls the Host's `wake`.
    fn wake(self: Arc<Self>);
}

// Call::poll, where `Waker::noop()` is today
self.future.as_mut().poll(&mut Context::from_waker(&self.waker))

// tool.rs: the signature stays, and its one `Pending` now wakes the task first
pub fn next_step() -> NextStep;

// testing.rs, the fake Host
impl Call<'_> {
    /// Blocks until the call is due its next step: at once when its last
    /// step made a file request or the Plugin has woken it since, and
    /// otherwise until the Plugin wakes it. Panics with a sentence that
    /// says so when no wake comes within 60 seconds.
    pub fn wait(&self);
}
// `PackRun::call` is `step`, then `wait`, until the call ends.
// The fake Host keeps each call's waker valid until `Loaded` drops, as a
// Host keeps it until the unload.
```

The loader:

```rust
// call.rs
/// What a call's `RvWaker` points at: whether the Plugin woke the call
/// since its last step began, and the task to wake. The `Plugin` owns
/// every slot and frees them after `RV_PLUGIN_UNLOAD` returns.
struct Parked { /* ... */ }

/// The `wake` of every `RvWaker`. Any thread may call it, at any time
/// until the Plugin unloads.
unsafe extern "C" fn wake(ctx: *mut c_void);

// Steps::poll, where the loader wakes itself today
Stepped::Waits => {
    // Parked. A wake that came during the step is seen here, so it
    // is not lost.
    if this.parked.park(cx.waker()) {
        cx.waker().wake_by_ref();
    }
    Poll::Pending
}
```

Rules the loader keeps:

- **Slots.** A call takes a free slot when it starts and gives it back when it ends, so there are only as many slots as calls were ever in flight at once. A wake meant for a call that ended can reach the next call in that slot, which costs that call one step it did not need.
- **No lost wake.** `park` stores the task's waker and reads the flag as one act, so a wake that lands between a step's return and the park still counts.
- **The flag.** It is cleared when a step begins. If only `park` cleared it, a wake while parked would cost two steps.
- **File requests come first.** While a file request waits for its answer, a wake sends no step. The SDK treats a step with no answer as a failed file request, and bashkit wakes during file requests all the time.
- **No lock across a call into the Plugin.** bashkit and tokio call the waker from destructors, on the Host's own thread, inside `RV_TOOL_CANCEL`. The fake Host keeps the same rule.

What has to wake itself now:

- `next_step()` in the SDK. The std Plugin's grep uses it and needs no change of its own.
- The SDK, when a step ends with a file answer nobody has taken and no request at the Host. That happens when the Host refuses a request, and today the next step delivers the refusal.
- The bashkit Plugin's `drive` loop in `session.rs`, at its poll cap. The reply to a finished file operation already wakes the call there, so this wake only states the intent, and no test can fail without it. Its `sleep` then parks.
- The raw test Plugins that return `RV_PENDING` without a file request: the `other` tool of the SDK's `testing/fixture.rs`, and the `steps` tool of the Harness's `test-plugin`. Its `never` tool stays silent, so a real DLL covers a call that parks and is then cancelled.

One limit that goes into the docs of `FileFuture` and `Pack::call`: a file future gets its answer only when a step polls it. So it does not work inside a set that polls a child only after that child's waker fired, such as `FuturesUnordered`. That is true today as well, and no shipped Plugin does it.

Tests, because today's helpers poll in a busy loop and cannot see a missing wake:

- **SDK:** a call that awaits `next_step()` returns from `step` then `wait`. A wake from a second thread ends a `wait`. A wake after the call ended does nothing.
- **Loader:** its test `block_on` waits for a real wake. One test for each wake timing: during a step, between a step's return and the park (on the slot itself), while parked and from another thread, while a file request waits, during `RV_TOOL_CANCEL`, and after the call ended.
- **bashkit:** its `stepped` helper steps then waits, and one new test shows that a `sleep` call parks until its timer fires.
- **CI:** the test job in `runvil-plugin-api`, `runvil-plugin-std`, `runvil-plugin-bashkit`, and `runvil-plugin-web` gets a timeout, so a hang fails in minutes and not in hours.

Contract rules the waker changes, in the docs of `runvil-plugin-sys` and in the `AGENTS.md` of `runvil-plugin-api`:

- A step that returns `RV_PENDING` no longer "asks for another step".
- Each function a Plugin calls back sits beside the `ctx` it takes first. `wake` takes the waker's own `ctx`.
- Functions answer only while their call runs and only on its thread. `wake` is the exception to both halves.
- The `ctx` of a call is gone once the call returns. A waker's `ctx` lives until the unload.
- The Host never reads through a `ctx` it has not matched to the call open on the calling thread. For `wake` it must.
- Every function in the contract returns an `RvStatus`. `wake` returns nothing.
- Every step of one call gets the same `tool` and `arguments`, and now the same `resources` and `waker`.

Other text that states the old rule and changes with it:

- [runvil-plugin-api/README.md](runvil-plugin-api/README.md), including its note for a Plugin written in C, and the docs of `Pack::call` in [runvil-plugin-api/crates/runvil-plugin-sdk/src/pack.rs](runvil-plugin-api/crates/runvil-plugin-sdk/src/pack.rs), which say the future never waits on a waker.
- The comments on `FileFuture::poll` and on the SDK's poll, the fake Host's docs, and the crate docs of the loader.
- The bashkit Plugin's docs in `lib.rs` and `session.rs`. They say a `sleep` is stepped with nothing to do, and that the runtime's one thread only runs timers. bashkit also wakes from tokio's blocking pool, which parses scripts.
- One comment in the std Plugin's test helper `fake.rs`.
- `decisions/tools-and-packs.md` in the harness, which states the old rule once and lists waiting on outside work as future work.

## 10. Host services are erased

Files: [runvil-harness/crates/runvil-plugin/src/service.rs](runvil-harness/crates/runvil-plugin/src/service.rs), [runvil-harness/crates/plugin-mcp/src/lib.rs](runvil-harness/crates/plugin-mcp/src/lib.rs), [runvil-workshop/crates/workshop/server/src/agents.rs](runvil-workshop/crates/workshop/server/src/agents.rs)

**Today.** `HostServices` is a map of Rust objects that a Host fills and hands to the `install` function of each Rust Plugin. Once web has moved, one Plugin reads it, mcp, and it reads one value, the tokio runtime handle. Workshop is the one Host that fills it.

**After.** It is gone, with `ServiceKey`, `ServiceId`, and `TOKIO_RUNTIME`. mcp is compiled into the Host and shares the Host's copy of tokio, so its `install` takes the handle as a plain argument. It also takes the server list as itself, where today Workshop wraps it in an object to fit the three-argument shape. A server list that is neither `null` nor an object still inserts the Pack unavailable, as today.

```rust
// plugin-mcp
pub fn install(
    packs: &mut PackRegistry,
    servers: Value,                    // the `mcpServers` object of an mcp.json file
    runtime: tokio::runtime::Handle,
) -> Result<(), Error>;

// Workshop, in agents.rs
if let Ok(runtime) = tokio::runtime::Handle::try_current() {
    if let Err(error) = plugin_mcp::install(&mut packs, servers, runtime) {
        tracing::warn!(%error, "the mcp Plugin inserted no Pack");
    }
}
```

The crate doc that goes on mcp, in `plugin-mcp/src/lib.rs`. mcp could be a DLL in theory, so it says what the contract lacks, not that a DLL is impossible:

```rust
//! This Pack is compiled into the Host and is not a library Plugin. It
//! uses parts of the Rust Pack API that the C contract has no form for:
//! the caller of a call, the `describes_tools` flag, and a configuration
//! at `install`. Its servers' tools are meant to join a section's
//! Dynamic Set through `Pack::render`, which the contract has no verb
//! for either.
```

What is erased:

- `service.rs`, whole, and the four re-exports in `runvil::plugin`, with their lines in `public-api.txt`.
- The `#[expect(clippy::needless_pass_by_value)]` on `plugin_mcp::install`. The new arguments are consumed, so the expectation would be unfulfilled and fail the build.
- Every doc sentence that names a service or the three-argument `install`: in `runvil-plugin` (`lib.rs`, `pack.rs`, `context.rs`, `registry.rs`), in `crates/runvil/src/lib.rs`, in `runvil-types` (`packs.rs`), in the runner's `lib.rs`, and in `plugin-mcp`, its manifest included.
- The tests that build a services map: the runner's `packs-tests*.rs`, `runvil-plugin/tests/it/pack_info.rs`, and mcp's `tests/it`. Two mcp tests lose their subject, the missing runtime, and go.
- In Workshop: `services()`, the `RUNTIME` key, the `{"mcp": servers}` wrapper, their tests, and the comments in `server/Cargo.toml`, `agents/broker.rs`, and `agents/gateway.rs`.
- In current docs: four harness `decisions` pages (`harness-and-host`, `tools-and-packs`, `models-and-conversations`, `testing-and-debugging`), chapters 4, 12, and 13 of the language guide, and the classroom tutorial.

The rules:

- No shim, alias, or deprecated item stays behind.
- No comment, doc, or new file says that it was removed, or what used to be there. Text that named it is rewritten to describe the code as it is.
- Old dated plans are left as they are, in every repo: `vibe/`, `plan/`, `plans/`, and `lesson-plans/`.
- The step ends with this search in `runvil-harness`, `runvil-workshop`, `runvil-plugin-web`, `runvil-docs`, and `runvil-classroom`. Every hit is read, and none is left that is about Host services or the Host's search provider:

```
git grep -n -i -P '\bHostServices\b|\bServiceKey\b|\bServiceId\b|TOKIO_RUNTIME|tokio-runtime|tokio runtime service|search.provider|SearchProvider|GatewaySearch|Host.s services|Host services|Host-wide|services map|service vocabulary' -- . ':(exclude)vibe' ':(exclude)plan' ':(exclude)plans' ':(exclude)lesson-plans'
```

## Behavior that changes

- **A Host with no runtime.** Today mcp is inserted unavailable, with the reason `MCP needs runvil/tokio-runtime`. After, such a Host does not install mcp, so a prompt that declares it is refused as not installed.
- **A Host without web search.** Today the whole Pack is unavailable and a prompt with `uses: [web]` is refused before it runs. After, the Pack is there, `web/fetch` works, and `web/search` fails when called with `web_search: the host offers no web search right now`.
- **Workshop with its Gateway down.** Today the model reads `web_search: request failed`. After, it reads the message above. For up to 5 seconds after a Gateway stops, the heartbeat still says it is up, and a search fails as a transport error.
- **The configuration check.** `web takes no configuration` goes away, because a DLL Plugin is handed no configuration.
- **Waiting.** A fetch or a search that waits on the network is parked until its reply arrives, as it is today on the Host's runtime. A bashkit `sleep` stops being stepped over and over. A parked call can still be stopped at any time.
- **Error causes.** The model reads the same text as today. A tool error has one text for the model and a longer one for logs, and `with_cause` adds a cause to the log text only. The SDK's `ToolError` has one message, so it gets the model's text and the cause is dropped. No log gets the cause any more.
- **The blocked address log.** The resolver's debug log goes away, because a DLL has nowhere to send it.
- **TLS.** The DLL has its own copy of reqwest and rustls, so the Harness's check for one crypto backend no longer covers it. The new repo's CI runs the same check.
- **Unload.** The Plugin joins its threads when it unloads, so an unload waits for a DNS lookup that is in flight. Today Workshop puts a time limit on shutting its own runtime down, and fetches run there.

## Order of work

**Pushing.** A step is finished when its commit is on `origin`. In `runvil-plugin-web` and `runvil-classroom`, `origin` is `cppalliance` itself. In the other repos it is your fork. Commits go on the current branch.

**Pins.** Each repo pins the one before it by commit. A pin keeps the `cppalliance` address and names the commit on your fork. GitHub answers for a fork's commit under the parent repo's name: its API does so today for harness commit `bdcadfbb`, which only your fork has. Whether Cargo's fetch does the same is not verified, and every commit pinned today is on `cppalliance`, so the first re-pin in step 2 is the test. If it fails, that commit has to reach `cppalliance` before the pin can land. The same question applies to the `runvil` repo's CI, which checks Workshop out by commit from `cppalliance`. Until a pin lands, the next repo builds against the local checkout through a `[patch]` entry that is not committed.

**One contract commit.** Five manifests name `runvil-plugin-api`: the harness, the std, bashkit, and web Plugins, and Workshop. Cargo refuses two commits of the contract in one build, so all five must name the same one. Step 1 ends with that commit, and step 2 does not start before it. A later fix in `runvil-plugin-api` means all five pin again.

1. Two pieces that do not depend on each other:
   - `runvil-plugin-api`: sections 1 and 2, and the contract, SDK, and fake Host parts of section 9. They touch the same files, so they run one after another.
   - `runvil-plugin-web`: the history, the old plans, the repo files, and the first proof (section 6), on the SDK as it is pinned today.
2. After step 1, these four can run side by side:
   - `runvil-harness`, in this order: sections 3 and 4 with the loader part of section 9, as one commit. Then section 5. Then section 8, which deletes the crate and so waits for the history in step 1. Then section 10, which comes after section 8 because web is the other reader of `HostServices`. The `decisions` pages change with each.
   - `runvil-plugin-web`: the port (section 6).
   - `runvil-plugin-std`: pin the SDK at 0.6. No code change is expected.
   - `runvil-plugin-bashkit`: pin the SDK at 0.6, and the changes in section 9.
3. `runvil-workshop`, after everything in step 2, as one commit (sections 7 and 10).
4. `runvil`: the installer lists and the Workshop pin, as one commit, after step 3 is on `origin`.
5. The check at the end, below.
6. Docs:
   - `runvil-docs`: the two sections of [runvil-docs/src/language/13-web-fetch-and-search.md](runvil-docs/src/language/13-web-fetch-and-search.md) that describe the search provider and the unavailable Pack, and chapters 4 and 12 where they name a Host-wide service.
   - `runvil-classroom/tutorials/tutorial-plugin-architecture.md`, which describes web as a Rust `install` Plugin and describes Host services.
   - The Workshop design page that lists the four pinned crates and the two library Plugin crates. It was in `runvil-design`, which is not in the workspace now.

Each repo's checks are the Verification list in its own `AGENTS.md`.

## The check at the end

- `runvil-workshop --plugins` lists std, bashkit, and web.
- In Workshop, with a Gateway running, an agent does one `web/search` and one `web/fetch`, and both answer.
- CPU stays idle while a slow fetch waits, and while a bashkit `sleep 5` runs by itself. A `sleep` in a pipeline is still stepped about 100 times a second, because bashkit checks its budget on a 10 millisecond timer there.
- With the Gateway stopped, `web/fetch` still works and `web/search` fails. In the first 5 seconds it fails as a transport error, and after that with the message that the host offers no web search.

## Not in this plan

- **Moving mcp to the contract.** It stays compiled into the Host, with an `install` function. Section 10 has what the contract would need first.
- **A number that says when the resources changed.** No Plugin reads one. A Plugin that keeps what it parsed compares the text, which is about 100 bytes.
- **A separate Gateway key for each feature.** Every entry holds the Gateway's one bearer key.
- **The search tool's argument rules.** They are looser than the Gateway's in a few places: country and language codes, and the shape of a domain entry. A value the tool accepts and the Gateway refuses comes back as a 400 from the Gateway. They move as they are.
- **File futures inside sets that poll a child only when it was woken.** Section 9 writes the limit down and changes nothing.
