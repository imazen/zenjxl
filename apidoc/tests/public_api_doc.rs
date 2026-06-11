//! Public-API surface snapshots for the PARENT package (docs/public-api/).
//! Shared implementation + format docs: the `zenutils-apidoc` crate.
//!
//! zenjxl uses the default configuration: supported surface = default
//! features; features file = all manifest features except `_*`-prefixed
//! internal gates (`__expert` lands in zenjxl.internal.txt).
#[test]
fn public_api_surface_docs_are_current() {
    zenutils_apidoc::ApiDoc::new().workspace_dir("..").run();
}
