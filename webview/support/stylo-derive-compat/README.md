# Stylo derive compatibility patch

This is the `stylo_derive` proc-macro crate from Servo Stylo commit
`cd230184a620c3e6c0f25faa07838fddcefec32d` (MPL-2.0), copied without other
source changes. It types the four generated `ToCss` success results as
`Result<(), std::fmt::Error>` to avoid Rust E0282 in the integrated Komet GPUI
and Servo dependency graph.

To apply from the repository root, patch the pinned Stylo Git source:

```toml
[patch."https://github.com/servo/stylo"]
stylo_derive = { path = "webview/support/stylo-derive-compat" }
```

Keep the source revision and crate version unchanged. Remove this local patch
when Stylo's pinned `ToCss` derive emits unambiguous result types.
