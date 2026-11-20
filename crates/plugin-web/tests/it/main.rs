//! The Plugin as a Host that links the crate in reaches it: through
//! [`Web`], the one item the crate makes public, handed to the SDK's
//! entry point.

use plugin_web::Web;
use runvil_plugin_sdk::testing;

#[test]
fn a_host_that_links_the_crate_loads_the_plugin_and_finds_its_tool() {
    let loaded = testing::load::<Web>().expect("the Plugin loads");
    assert_eq!(loaded.name(), "web");
    let pack = loaded
        .new_pack("runvil/web")
        .expect("the Pack instance is made");
    let tools: Vec<&str> = pack.tools().iter().map(|tool| tool.name.as_str()).collect();
    assert_eq!(tools, ["fetch"]);
}
