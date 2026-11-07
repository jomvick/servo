# Servo WebView

`webview-core` contains the engine-independent spaces, tabs, ownership, errors,
and snapshot model. `webview-engine` adapts Servo to that model and emits CPU
frames. `webview-engine` runs in its own process and exposes the authenticated local
JSON-RPC bridge. `webview-ui` is a GPUI client that can be embedded in another
GPUI application; the separate process boundary also avoids native font
dependency collisions between Servo and GPUI. `webview-cli` is the standalone
windowed shell using the same GPUI revision as Komet.

Build the Servo process, then run the standalone browser:

```sh
cargo build -p webview-engine --bin servo-webview-engine
cargo run --manifest-path webview/Cargo.toml -p webview-cli -- https://example.com
```

The URL is optional; the default is `https://example.com`. Browser profile data
is stored under the platform's local application-data directory in
`servo-webview/`. The standalone shell provides page rendering, link
navigation, back, forward, and reload. `SERVO_WEBVIEW_ENGINE` can override the
path to the daemon executable. An embedding GPUI application can create
`webview_ui::BrowserView::with_config` with its own RPC endpoint and token.

The engine can also produce a structured accessibility-style snapshot:

```rust
let snapshot = engine.snapshot(tab, &SnapshotOptions::default())?;
```

Same-origin frames are traversed. Frames blocked by the browser's same-origin
policy are represented as unreadable rather than silently omitted. Frames are
rendered to RGBA on the engine thread and converted to GPUI's BGRA format on
the UI thread; retired GPUI image tiles are evicted as frames advance.

The CLI starts the daemon with a cryptographically random token and a loopback-
only WebSocket JSON-RPC server. The server authenticates every connection and
dispatches `ego.*` calls on Servo's owning thread. It binds an ephemeral port by
default (`SERVO_WEBVIEW_PORT=0`) and prints
`SERVO_WEBVIEW_READY ws://127.0.0.1:<port>` to stdout once ready. The CLI waits
for this signal instead of assuming a fixed port. `SERVO_WEBVIEW_PORT` can
select a fixed port; `SERVO_WEBVIEW_TOKEN` can supply a token instead of letting
the CLI generate one. Spaces and tabs are created through `ego.createTaskSpace`
and `ego.createTab`, not implicitly by the daemon. The listener is not exposed
on non-loopback interfaces.

```sh
SERVO_WEBVIEW_PORT=9239 cargo run --manifest-path webview/Cargo.toml -p webview-cli -- https://example.com
```
