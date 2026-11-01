# ego-lite

A standalone Rust 2024 / GPUI application and authenticated local API, sharing
its Servo runtime with future Komet integration. GPUI and the process allocator
use Komet's dependency versions; the headless feature does not link GPUI.

```sh
cargo run -p ego-lite -- https://example.com
cargo run -p ego-lite --no-default-features --features engine -- \
  --headless --data-dir ./ego-data --listen 127.0.0.1:9223 https://example.com
cargo run -p ego-lite -- call ego.getBrowserVersion '{}'
cargo run -p ego-lite -- --data-dir ./ego-data call ego.listTabs '{"space":1}'
```

`--data-dir`, `--listen` and `--token-file` apply to both the server and `call`.
Use the same values for both processes. With no overrides, the data directory is
`$XDG_DATA_HOME/ego-lite` or `$HOME/.local/share/ego-lite`. The first server startup
creates `rpc.token` inside this directory with Unix mode `0600`. Existing token
files with group/other permissions or symlinks are rejected. Never share this
token: it grants browser automation, including JavaScript evaluation.

The endpoint is HTTP POST `/rpc`, with one JSON-RPC request per HTTP message.
It requires `Authorization: Bearer <token>` and `Content-Type: application/json`.
Responses keep JSON-RPC IDs and `EGO_*` error codes; notifications return HTTP
204 without a body. The `call` command prints the JSON response and exits
unsuccessfully on RPC or HTTP errors. It does not start another browser.

Listeners must use literal loopback addresses (`127.0.0.1` or `::1`), never
`0.0.0.0`. Requests with an `Origin` header or a non-loopback `Host` are rejected.
Request bodies are limited to 1 MiB, concurrent requests to 32, and backend waits
to 30 seconds. Only published `ego.*` methods are accepted; native `ui.*`
commands are never exposed over the network. `sendCDPMessage` explicitly returns
`EGO_CDP_CHANNEL_UNAVAILABLE` rather than pretending to speak CDP.

The engine owns a profile lock until shutdown. Different processes must use
different data directories. The headless process stops on Ctrl-C; the headed
process stops when its window closes. Close clients and drop the `Browser` owner
when embedding; the runtime's thread is not a detached daemon.

Server/authentication tests do not compile Servo or GPUI:

```sh
cargo test -p ego-lite --no-default-features --lib
```
