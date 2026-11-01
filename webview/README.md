# Servo webview / ego-lite

An embeddable Servo browser with a standalone native application. The standalone
shell uses the same Rust 2024, GPUI fork and platform backend as Komet, rather
than a second web UI framework or an Electron wrapper. Servo remains the web
engine; it is not rewritten in GPUI.

## Build and run (Linux)

Install the Rust toolchain declared at the repository root. On Ubuntu 24.04:

```sh
./webview/scripts/setup-linux.sh
cargo run -p ego-lite -- https://example.com
# No display server or GPUI dependencies in the executable:
cargo run -p ego-lite --no-default-features --features engine -- --headless
```

Use `--data-dir PATH` for a persistent profile and `--listen 127.0.0.1:9223` to
choose the local API address. The API uses a private token file; the CLI reads
that file, so tokens need not appear in shell history:

```sh
cargo run -p ego-lite -- call ego.listTaskSpaces '{}'
cargo run -p ego-lite -- call ego.listTabs '{"space":1}'
cargo run -p ego-lite -- call ego.snapshot '{"tab":1}'
cargo run -p ego-lite -- call ego.evaluate '{"tab":1,"expression":"document.title"}'
```

See [`cli/README.md`](cli/README.md) for the exact transport and CLI options.
Do **not** build `ego-lite` together with `servoshell` in one Cargo invocation:
feature unification would enable Servo's global allocator as well as the
standalone application's allocator. Select packages explicitly, not `--workspace`.

## Layers and future Komet integration

| Crate | Responsibility | GPUI dependency |
| --- | --- | --- |
| `webview-core` | Spaces, tabs, ownership, snapshot model and errors | None |
| `webview-bridge` | JSON-RPC contract and page automation scripts | None |
| `webview-engine` | Servo, profile, rendering and native input | None |
| `webview-runtime` | Dedicated engine thread, bounded commands, latest frames | None |
| `webview-ui` | Native standalone browser shell | Komet's pinned GPUI |
| `ego-lite` | Executable, authenticated loopback API and CLI client | Optional |

Komet can depend on `webview-runtime` by path or by a pinned revision of this
fork. Keep the `Browser` owner alive for the lifetime of the browser panel:

```rust,ignore
use webview_runtime::{Browser, RuntimeConfig};
let browser = Browser::start(RuntimeConfig {
    data_dir: komet_profile_directory,
    initial_url: "about:blank".into(),
})?;
let browser_client = browser.client();
// On a background executor, never in a GPUI render/input callback:
let spaces = browser_client.request("ego.listTaskSpaces", serde_json::json!({}))?;
// The GPUI panel reads the latest Arc<RgbaImage> and uploads BGRA pixels.
let frame = browser_client.latest_frame(1);
// Drop panels/clients, then shut down when the application quits.
browser.shutdown()?;
```

For a GPUI host, `webview_ui::BrowserView::new(browser.client(), cx)` creates an
embeddable panel inside the existing application. Keep the `Browser` owner in
host state. Do not call `webview_ui::run` from Komet: that helper starts the
standalone application loop. The UI component polls only the shared latest
frame at display cadence; metadata RPC calls run on its worker, not GPUI's thread.

The engine and its `Rc`/GL objects never cross threads. Only commands, JSON and
`Arc<RgbaImage>` do. Idle runtime work is awakened by commands or Servo events;
there is no engine polling loop. Each tab keeps only its latest frame at this
boundary. The UI owns display cadence and converts RGBA to GPUI's BGRA.

When embedding from a checkout, the host workspace must also apply the small
FreeType compatibility facade from this fork:

```toml
[dependencies]
webview-runtime = { path = "vendor/servo/webview/runtime" }

[patch.crates-io]
freetype-sys = { path = "vendor/servo/webview/support/freetype-sys-compat" }

[patch."https://github.com/servo/stylo"]
stylo_derive = { path = "vendor/servo/webview/support/stylo-derive-compat" }
```

Komet's pinned GPUI font loader requests `freetype-sys` 0.20 while Servo uses
0.23. Cargo forbids two bindings with `links = "freetype"`. The facade reexports
0.23's FFI types under the older package version without building or linking a
second native library. Neither GPUI's revision nor Servo's font code is changed.
A narrowly patched copy of Stylo's derive crate adds explicit `fmt::Error`
types to generated `ToCss` results so the combined GPUI/Servo graph compiles.
Its source revision and license are recorded alongside the patch. The workspace
lockfile also aligns `accesskit` to the compatible 0.24.1 patch.
Do not copy this workspace's Cargo environment wholesale into Komet; the external
consumer build remains a separate integration verification step.

Komet's dependency pins are reused verbatim:

- `gpui` / `gpui_platform`: `jomvick/zed` at
  `e2ddcc6805f8c5088e62a60dfe517abcccd61a9a`.
- Standalone process allocator: `mimalloc` 0.1. The engine does not install a
  process-global allocator; Komet keeps ownership of its allocator.

## API and boundaries

The network API accepts only `ego.*` methods. Internal `ui.*` commands are
in-process only, so handing a task space to the user prevents agents from
navigating, evaluating scripts or interacting with its tabs. Profile isolation
is process-level: all spaces in this process share the selected default profile;
start another process with a different data directory for isolation. Concurrent
processes must not open the same profile. The engine selects a Rustls provider
before network startup unless the host has already selected one; this prevents
provider ambiguity when Komet's HTTP dependencies enable both Ring and AWS-LC.

`ego.sendCDPMessage` returns `EGO_CDP_CHANNEL_UNAVAILABLE`; this is not a Chrome
DevTools server. The bounded DOM snapshot is not Servo's accessibility tree.
Cross-origin frame limitations and action-reference behavior are documented with
the automation implementation; inaccessible frames must not be reported as if
they had been inspected. A native UI is not a security sandbox: treat browsed
pages and automation expressions as untrusted code.

## Verification

```sh
cargo test -p webview-core -p webview-bridge
cargo test -p ego-lite --no-default-features --lib
cargo test -p webview-engine
cargo test -p webview-runtime --test browser
cargo check -p ego-lite
```

The last three commands compile Servo (and the final one also compiles GPUI),
which is substantially more expensive than the model/protocol tests. A fresh
build can take tens of minutes. See [`../docs/adr/0001-architecture.md`](../docs/adr/0001-architecture.md)
for engine constraints and measured rendering tradeoffs.
