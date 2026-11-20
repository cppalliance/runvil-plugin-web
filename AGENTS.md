# runvil-plugin-web

The web Plugin for runvil, built as a dynamic library. It depends on `runvil-plugin-sdk`, from `cppalliance/runvil-plugin-api`, and on no Engine, Harness, or Host crate. The Gateway is a separate product in its own repository, `cppalliance/runvil-gateway`.

## Definitions

These terms have exactly one meaning each, everywhere in this repository, as in the Engine's repository. Write them capitalized.

- **Engine**: the crates that parse a prompt and step a run. They never see a Plugin.
- **Harness**: the crates that step the Engine and perform its effects. The Harness's loader is the only code that calls this Plugin's entry point in production.
- **Host**: an application that runs prompts through the Harness, such as Workshop. The Host chooses which Plugins load.
- **Pack**: code which installs an optional native prelude, an optional Lua prelude, and zero or more tools.
- **Plugin**: an entry point which installs zero or more Packs. A library Plugin is a dynamic library that exports `runvil_plugin`.

Anything else gets a qualified lowercase name, such as Rust's built-in test harness. Names defined outside this repository are used exactly as defined.

## Principles

- Design for human review. A reviewer reads one crate at a time: its code, its surface, and the items it names from other crates. Keep all three small.
- Do more with less. Before building, check whether an existing facility subsumes the work: reuse one, else make the smallest improvement that enables the feature, else add a new facility, which needs a material benefit beyond tidiness.
- Error and status messages assume a model reads them: concise, factual, and self-contained, naming what is missing or unmet with required versus actual.
- There are no users and no stored data to preserve, so backward compatibility does not matter: change formats, schemas, and APIs freely.

## Engineering

- Prefer types and compiler checks, then behavior tests. Gate on yes/no properties, never on a score threshold that ordinary work edits.
- Behavior changes ship with tests in the same change. Preserve behavior tests during refactors.
- The Plugin crate depends on `runvil-plugin-sdk` and outside libraries alone, and writes no `unsafe`. The SDK owns the C boundary. The only `unsafe` in the repository is in the example `crates/plugin-web/examples/load-dll.rs`, which opens the built library as a Host's loader does.
- The fetch tool is the server-side request forgery (SSRF) boundary between a URL that a model chose and the network. The fetch policy checks every URL that a model or a redirect names, and every address such a URL resolves to. An address that is not public is refused unless the policy grants that exact host and address.
- No fetch sends an ambient identity on any hop. The fetch client has no proxy, no cookie store, no automatic `Referer` header, and no default credentials.
- The search tool reaches the Gateway through the `runvil/web-search` entry of the Host's resources, which names the Gateway's address and its key. The tool holds no key of a search vendor.
- No key is printed. The Gateway's key appears in no tool output, no error message, and no `Debug` text, and a message about the Host's resources never quotes them.
- A library Plugin links its own copy of tokio and cannot use the Host's runtime, so the Plugin instance owns one. Every request runs as a task on it, and the tool call that awaits the task aborts it when the call is dropped. The runtime's two worker threads drive the sockets and the timers of every request, so work that only computes for long, such as rendering a page to markdown, runs on a thread the runtime starts for blocking work. Dropping the instance drops the runtime, which joins its threads, so an unload leaves no thread behind.
- A Plugin shares its process with the Host, so nothing here installs process-global state, such as a panic hook, a logger, or a default crypto provider (`clippy.toml` bans the functions that install them), and nothing starts a thread that outlives the Plugin's unload. What every run shares lives in the Plugin instance and never in a `static`, so an unload frees it.
- Comments explain a non-obvious constraint, ordering requirement, or workaround. A platform or external-bug workaround cites its upstream issue URL.

## Verification

- Tests: `cargo nextest run --locked --workspace --all-features`, with `cargo-nextest` installed. It runs no doctest, so an example in a doc comment is fenced as `text`.
- Lint: `cargo clippy --locked --workspace --all-targets --all-features`, with `CARGO_BUILD_WARNINGS=deny`.
- Format: `cargo fmt --all --check`.
- Docs: `cargo doc --locked --workspace --no-deps --all-features --document-private-items`, with `RUSTDOCFLAGS="-D warnings"`. Almost every item here is private, so the flag is what makes rustdoc check their docs. Clippy does not run rustdoc lints, so never skip this.
- The library Plugin: `cargo build --locked -p plugin-web --release`.
- Supply chain: `cargo deny check`, with `cargo-deny` installed. It holds the dependency tree to `deny.toml`: its advisories, its licenses, and its sources. CI also runs `cargo audit`, and it checks that `ring` stays out of the dependency graph, so the library links one rustls crypto backend.
- Custom lints: `cargo dylint --all -- --locked --workspace --all-targets --all-features`, with `cargo-dylint` and `dylint-link` installed. The lints live in `cppalliance/dylints`, at the `rev` the root `Cargo.toml`'s `[workspace.metadata.dylint]` pins.
- Structural checks: `cargo test -p build-checks`.
- The library as a Host loads it, by hand: after the release build, `cargo run --locked -p plugin-web --release --example load-dll -- <library> <https URL>` opens the built library, loads the Plugin from it, and fetches the page. It needs the network, so CI builds the example and does not run it.

## Structural Rules

- A file holds one concept and has no length limit. Split a file only when it holds a second unrelated concept.
- Size limits sit on functions and types. Clippy holds a function to 100 lines and 7 arguments (`clippy.toml`). `cargo test -p build-checks` holds a type to 25 inherent methods, summed across every `impl` block in its crate, and a struct to 12 named fields. Split an oversized type by responsibility; a new entry in the allowlist in `crates/build-checks/src/type_size.rs` is a design decision with a reason, never a way to make a change pass.
- Production code uses the standard module layout: `foo.rs`, with `foo/bar.rs` for its children. No `#[path]`, and no inline `mod` blocks. A module other than the crate root never has exactly one child file (tests aside): merge it, or make it a peer module `foo_bar.rs` declared beside `foo.rs`.
- Test code goes at the bottom of the file under test in a `#[cfg(test)] mod tests { }` block when practical (if in doubt, beside). Otherwise it goes beside that file, in `foo-tests.rs`, wired with `#[path = "foo-tests.rs"] mod tests;`, or in `foo/tests.rs`. A big test file may split by topic into siblings named after it, `<stem>-<topic>.rs`: `foo-tests-topic.rs` beside `foo-tests.rs`, and `fetch/tests-body.rs` beside `fetch/tests.rs`. These are the only files wired with `#[path]`, so under `src/` a hyphen in a file name means test code. A helper that the tests of several modules share is a module of its own, declared under `#[cfg(test)]` in the crate root. Top-level `tests/` and `examples/` trees follow Cargo's conventions.
- Pseudo-random test input comes only from a fixed seed that the failure prints.
