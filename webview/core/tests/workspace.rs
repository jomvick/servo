#[test]
fn workspace_exposes_version() {
    assert!(!webview_core::CRATE_VERSION.is_empty());
}
