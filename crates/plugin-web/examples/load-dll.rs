//! Loads the built library Plugin as a Host's loader opens it, and
//! fetches one `https://` page with it. It is run by hand, from the
//! root of the repository:
//!
//! ```text
//! cargo build --locked -p plugin-web --release
//! cargo run --locked -p plugin-web --release --example load-dll -- target/release/plugin_web.dll https://example.com/
//! ```
//!
//! The library is `plugin_web.dll` on Windows, `libplugin_web.so` on
//! Linux, and `libplugin_web.dylib` on macOS.
//!
//! The example opens the library, resolves `runvil_plugin`, loads the
//! Plugin through the SDK's fake Host, makes the Pack `runvil/web`, calls
//! `fetch` on the URL, and prints what the tool answers. The example
//! names no HTTP client, so the page can only have come through the
//! library: through its own copy of tokio, of the client, and of the TLS
//! stack under the client.
//!
//! This file holds the only `unsafe` in the repository.

#![expect(
    unsafe_code,
    reason = "the example opens the built library and calls its entry point, which is the C boundary a Host's loader crosses"
)]

use std::error::Error;
use std::path::Path;
use std::process::ExitCode;

use libloading::Library;
use runvil_plugin_sdk::sys::PluginEntry;
use runvil_plugin_sdk::testing::{self, Loaded};
use serde_json::json;

/// The Pack the web Plugin inserts.
const PACK: &str = "runvil/web";

/// The tool that fetches a page.
const FETCH: &str = "fetch";

/// The one symbol a library Plugin exports, with the NUL that ends it.
const ENTRY_POINT: &[u8] = b"runvil_plugin\0";

/// A Plugin loaded from its library. The fields drop in the order they
/// are declared, so the Plugin is unloaded before its library closes.
struct Opened {
    plugin: Loaded,
    _library: Library,
}

fn main() -> ExitCode {
    let mut arguments = std::env::args_os().skip(1);
    let (Some(library), Some(url), None) = (arguments.next(), arguments.next(), arguments.next())
    else {
        eprintln!("usage: load-dll <path of the built library> <https URL>");
        return ExitCode::from(2);
    };
    let Some(url) = url.to_str().filter(|url| url.starts_with("https://")) else {
        eprintln!(
            "the URL must begin with https://, because the example shows that the library's TLS works"
        );
        return ExitCode::from(2);
    };
    match fetch(Path::new(&library), url) {
        Ok(()) => ExitCode::SUCCESS,
        Err(message) => {
            eprintln!("{message}");
            ExitCode::FAILURE
        }
    }
}

/// Loads the Plugin in the library at `path`, calls its `fetch` tool on
/// `url`, and prints each step. An error is a sentence that says which
/// step failed and why.
fn fetch(path: &Path, url: &str) -> Result<(), String> {
    let opened = open(path)?;
    let plugin = &opened.plugin;
    println!(
        "loaded the Plugin {} {} from {}",
        plugin.name(),
        plugin.version(),
        path.display()
    );
    // The fake Host panics when it is asked for a Pack or a tool that the
    // Plugin did not add, so both are looked up first.
    if !plugin.packs().iter().any(|pack| pack.name == PACK) {
        return Err(format!("the Plugin inserted no Pack named {PACK}"));
    }
    let pack = plugin
        .new_pack(PACK)
        .map_err(|failure| format!("the Pack {PACK} was not made: {failure:?}"))?;
    if !pack.tools().iter().any(|tool| tool.name == FETCH) {
        return Err(format!("the Pack {PACK} has no tool named {FETCH}"));
    }
    println!("made the Pack {PACK}, and calling its tool {FETCH} on {url}");
    let output = pack
        .call(FETCH, &json!({ "url": url }).to_string())
        .map_err(|failure| format!("the call broke the contract: {failure:?}"))?
        .map_err(|error| format!("the call failed as {:?}: {error}", error.kind()))?;
    let trust = if output.is_trusted() {
        "trusted"
    } else {
        "untrusted"
    };
    println!("the tool answered this {trust} output:\n{}", output.text());
    Ok(())
}

/// Opens the library at `path`, resolves its entry point, and loads the
/// Plugin through the SDK's fake Host. An error is a sentence that says
/// which step failed and why.
fn open(path: &Path) -> Result<Opened, String> {
    let path = std::path::absolute(path).map_err(|error| {
        format!(
            "the path {} could not be made absolute: {error}",
            path.display()
        )
    })?;
    let library = open_library(&path).map_err(|error| {
        format!(
            "the library {} could not be opened: {}",
            path.display(),
            with_cause(&error)
        )
    })?;
    // SAFETY: the C contract gives `runvil_plugin` the shape of
    // `PluginEntry`, and `ENTRY_POINT` ends in NUL.
    let entry: PluginEntry = *unsafe { library.get::<PluginEntry>(ENTRY_POINT) }
        .map_err(|error| format!("runvil_plugin is missing: {}", with_cause(&error)))?;
    // SAFETY: the example is run on a library built from this repository,
    // whose entry point the SDK writes, so it follows the C contract and
    // does not unwind. The entry point is valid while the library is
    // open, and `Opened` closes the library only after it has unloaded
    // the Plugin.
    let plugin = unsafe { testing::load_entry(entry) }
        .map_err(|failure| format!("the Plugin did not load: {failure:?}"))?;
    Ok(Opened {
        plugin,
        _library: library,
    })
}

/// Opens the library at the absolute `path` with the flags the Harness's
/// loader uses, `LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR |
/// LOAD_LIBRARY_SEARCH_SYSTEM32`. With them the library's own
/// dependencies resolve only from its directory and System32, so a
/// library that needs anything else fails to open here as it does there.
#[cfg(target_os = "windows")]
fn open_library(path: &Path) -> Result<Library, libloading::Error> {
    use libloading::os::windows::{
        LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR, LOAD_LIBRARY_SEARCH_SYSTEM32, Library as WindowsLibrary,
    };

    let flags = LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_SYSTEM32;
    // SAFETY: the path is absolute, as LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR
    // requires. Opening a library runs its initialization code, and the
    // example is run on a library built from this repository.
    unsafe { WindowsLibrary::load_with_flags(path, flags) }.map(Library::from)
}

/// Opens the library at `path`, as the Harness's loader does.
#[cfg(not(target_os = "windows"))]
fn open_library(path: &Path) -> Result<Library, libloading::Error> {
    // SAFETY: opening a library runs its initialization code, and the
    // example is run on a library built from this repository.
    unsafe { Library::new(path) }
}

/// The text of `error` and, when it has a cause, the cause's text after
/// it. On Windows `libloading` reports only that the system call failed,
/// and keeps the system's reason, such as a module that could not be
/// found, as the cause.
fn with_cause(error: &libloading::Error) -> String {
    match error.source() {
        Some(cause) => format!("{error}: {cause}"),
        None => error.to_string(),
    }
}
