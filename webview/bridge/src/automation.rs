//! In-page scripts used by the runtime's browser automation surface.

pub const PRELOAD: &str = include_str!("../assets/automation-preload.js");
pub const SNAPSHOT_SCRIPT: &str = include_str!("../assets/automation-snapshot.js");

pub fn click_script(reference: &str) -> String {
    include_str!("../assets/automation-click.js").replace(
        "__EGO_REFERENCE__",
        &serde_json::to_string(reference).expect("a string is JSON serializable"),
    )
}

pub fn fill_script(reference: &str, value: &str) -> String {
    let template = include_str!("../assets/automation-fill.js");
    let (prefix, rest) = template
        .split_once("__EGO_REFERENCE__")
        .expect("reference placeholder");
    let (middle, suffix) = rest.split_once("__EGO_VALUE__").expect("value placeholder");
    format!(
        "{prefix}{}{middle}{}{suffix}",
        serde_json::to_string(reference).expect("JSON string"),
        serde_json::to_string(value).expect("JSON string")
    )
}

#[cfg(test)]
mod tests {
    #[test]
    fn substitution_does_not_rescan_parameters() {
        let script = super::fill_script("__EGO_VALUE__", "\";throw new Error('injection')//");
        assert!(script.contains("const reference = \"__EGO_VALUE__\";"));
        assert!(script.contains("const value = \"\\\";throw new Error('injection')//\";"));
    }
}
