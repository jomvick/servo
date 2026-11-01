use std::net::{IpAddr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use axum::Router;
use axum::body::Bytes;
use axum::extract::{DefaultBodyLimit, State};
use axum::http::{HeaderMap, HeaderValue, StatusCode, header};
use axum::response::{IntoResponse, Response};
use axum::routing::post;
use serde_json::Value;
use tokio::sync::Semaphore;
use webview_bridge::{ErrorObject, Reply, Request};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;
pub const MAX_CONCURRENT_REQUESTS: usize = 32;
pub const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);

pub trait RpcBackend: Send + Sync + 'static {
    fn request_rpc(&self, method: &str, params: Value) -> Result<Value, ErrorObject>;
}

#[derive(Clone)]
pub struct ServerState {
    backend: Arc<dyn RpcBackend>,
    token: Arc<str>,
    listen: SocketAddr,
    concurrency: Arc<Semaphore>,
}

impl ServerState {
    pub fn new(backend: impl RpcBackend, token: impl Into<String>, listen: SocketAddr) -> Self {
        Self::with_shared_backend(Arc::new(backend), token, listen)
    }

    pub fn with_shared_backend(
        backend: Arc<dyn RpcBackend>,
        token: impl Into<String>,
        listen: SocketAddr,
    ) -> Self {
        Self {
            backend,
            token: Arc::from(token.into()),
            listen,
            concurrency: Arc::new(Semaphore::new(MAX_CONCURRENT_REQUESTS)),
        }
    }
}

pub fn router(state: ServerState) -> Router {
    Router::new()
        .route("/rpc", post(handle_rpc))
        .layer(DefaultBodyLimit::max(MAX_REQUEST_BYTES))
        .with_state(state)
}

async fn handle_rpc(State(state): State<ServerState>, headers: HeaderMap, body: Bytes) -> Response {
    if headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !valid_host(headers.get(header::HOST), state.listen) {
        return StatusCode::FORBIDDEN.into_response();
    }
    if !valid_token(headers.get(header::AUTHORIZATION), &state.token) {
        return StatusCode::UNAUTHORIZED.into_response();
    }
    if !valid_content_type(headers.get(header::CONTENT_TYPE)) {
        return StatusCode::UNSUPPORTED_MEDIA_TYPE.into_response();
    }

    let raw = match std::str::from_utf8(&body) {
        Ok(raw) => raw,
        Err(_) => {
            return rpc_response(Reply::error(None, ErrorObject::rpc(-32700, "Parse error")));
        },
    };
    let request = match Request::parse(raw) {
        Ok(request) => request,
        Err(reply) => return rpc_response(reply),
    };
    if let Err(error) = request.call() {
        return if request.id.is_none() {
            StatusCode::NO_CONTENT.into_response()
        } else {
            rpc_response(Reply::error(request.id, error))
        };
    }
    let id = request.id.clone();
    let method = request.method;
    let params = request.params;

    let permit = match state.concurrency.clone().try_acquire_owned() {
        Ok(permit) => permit,
        Err(_) => return StatusCode::SERVICE_UNAVAILABLE.into_response(),
    };
    let backend = state.backend.clone();
    let result = tokio::time::timeout(
        REQUEST_TIMEOUT,
        tokio::task::spawn_blocking(move || {
            let _permit = permit;
            backend.request_rpc(&method, params)
        }),
    )
    .await;

    if id.is_none() {
        return StatusCode::NO_CONTENT.into_response();
    }
    let reply = match result {
        Ok(Ok(Ok(value))) => Reply::ok(id.expect("checked above"), value),
        Ok(Ok(Err(error))) => Reply::error(id, error),
        Ok(Err(_)) => Reply::error(id, ErrorObject::rpc(-32603, "Internal error")),
        Err(_) => Reply::error(id, ErrorObject::rpc(-32603, "Request timed out")),
    };
    rpc_response(reply)
}

fn rpc_response(reply: Reply) -> Response {
    let mut response = (StatusCode::OK, reply.to_json()).into_response();
    response.headers_mut().insert(
        header::CONTENT_TYPE,
        HeaderValue::from_static("application/json"),
    );
    response
}

fn valid_token(header: Option<&HeaderValue>, expected: &str) -> bool {
    let Some(value) = header.and_then(|v| v.to_str().ok()) else {
        return false;
    };
    let Some(token) = value.strip_prefix("Bearer ") else {
        return false;
    };
    constant_time_eq(token.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(left: &[u8], right: &[u8]) -> bool {
    let mut difference = left.len() ^ right.len();
    for index in 0..left.len().max(right.len()) {
        difference |= usize::from(
            left.get(index).copied().unwrap_or(0) ^ right.get(index).copied().unwrap_or(0),
        );
    }
    difference == 0
}

fn valid_content_type(value: Option<&HeaderValue>) -> bool {
    value
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .is_some_and(|value| value.trim().eq_ignore_ascii_case("application/json"))
}

fn valid_host(value: Option<&HeaderValue>, listen: SocketAddr) -> bool {
    let Some(host) = value.and_then(|value| value.to_str().ok()) else {
        return false;
    };
    let Ok(authority) = host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    if authority.port_u16() != Some(listen.port()) {
        return false;
    }
    match authority.host().trim_matches(['[', ']']).parse::<IpAddr>() {
        Ok(ip) => ip.is_loopback(),
        Err(_) => authority.host().eq_ignore_ascii_case("localhost"),
    }
}

pub fn ensure_loopback(listen: SocketAddr) -> anyhow::Result<SocketAddr> {
    anyhow::ensure!(
        listen.ip().is_loopback(),
        "--listen must be a loopback address"
    );
    Ok(listen)
}

pub fn token_file_path(
    data_dir: &std::path::Path,
    token_file: Option<&std::path::Path>,
) -> std::path::PathBuf {
    token_file
        .map(std::path::Path::to_path_buf)
        .unwrap_or_else(|| data_dir.join("rpc.token"))
}

pub fn load_or_create_token(path: &std::path::Path) -> anyhow::Result<String> {
    use std::fs::{self, OpenOptions};
    use std::io::{Read, Write};

    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)?;
    }
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    match options.open(path) {
        Ok(mut file) => {
            set_private_permissions(&file)?;
            let token = hex::encode(rand::random::<[u8; 32]>());
            file.write_all(token.as_bytes())?;
            file.write_all(b"\n")?;
            file.sync_all()?;
            Ok(token)
        },
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
            let metadata = fs::symlink_metadata(path)?;
            anyhow::ensure!(metadata.is_file(), "token file must be a regular file");
            ensure_private_permissions(&metadata)?;
            let mut token = String::new();
            fs::File::open(path)?.read_to_string(&mut token)?;
            let token = token.trim().to_owned();
            anyhow::ensure!(
                token.len() == 64 && token.bytes().all(|byte| byte.is_ascii_hexdigit()),
                "token file must contain a 64-character hexadecimal token"
            );
            Ok(token)
        },
        Err(error) => Err(error.into()),
    }
}

#[cfg(unix)]
fn set_private_permissions(file: &std::fs::File) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    file.set_permissions(std::fs::Permissions::from_mode(0o600))
}

#[cfg(not(unix))]
fn set_private_permissions(_: &std::fs::File) -> std::io::Result<()> {
    Ok(())
}

#[cfg(unix)]
fn ensure_private_permissions(metadata: &std::fs::Metadata) -> anyhow::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    anyhow::ensure!(
        metadata.permissions().mode() & 0o077 == 0,
        "token file permissions must be 0600 or stricter"
    );
    Ok(())
}

#[cfg(not(unix))]
fn ensure_private_permissions(_: &std::fs::Metadata) -> anyhow::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicUsize, Ordering};

    use axum::body::Body;
    use axum::http::{Request as HttpRequest, header};
    use http_body_util::BodyExt;
    use serde_json::json;
    use tower::ServiceExt;

    use super::*;

    const LISTEN: SocketAddr = SocketAddr::V4(std::net::SocketAddrV4::new(
        std::net::Ipv4Addr::LOCALHOST,
        9223,
    ));
    const TOKEN: &str = "a1";

    #[derive(Default)]
    struct FakeBackend(AtomicUsize);

    impl RpcBackend for FakeBackend {
        fn request_rpc(&self, method: &str, params: Value) -> Result<Value, ErrorObject> {
            self.0.fetch_add(1, Ordering::SeqCst);
            Ok(json!({ "method": method, "params": params }))
        }
    }

    fn request(body: &str) -> HttpRequest<Body> {
        HttpRequest::post("/rpc")
            .header(header::HOST, "127.0.0.1:9223")
            .header(header::AUTHORIZATION, "Bearer a1")
            .header(header::CONTENT_TYPE, "application/json")
            .body(Body::from(body.to_owned()))
            .unwrap()
    }

    #[tokio::test]
    async fn auth_origin_host_and_method_are_restricted() {
        let backend = Arc::new(FakeBackend::default());
        let app = router(ServerState::with_shared_backend(
            backend.clone(),
            TOKEN,
            LISTEN,
        ));

        let mut missing_auth =
            request(r#"{"jsonrpc":"2.0","id":1,"method":"ego.getBrowserVersion"}"#);
        missing_auth.headers_mut().remove(header::AUTHORIZATION);
        assert_eq!(
            app.clone().oneshot(missing_auth).await.unwrap().status(),
            StatusCode::UNAUTHORIZED
        );

        let mut browser_origin =
            request(r#"{"jsonrpc":"2.0","id":1,"method":"ego.getBrowserVersion"}"#);
        browser_origin
            .headers_mut()
            .insert(header::ORIGIN, HeaderValue::from_static("http://localhost"));
        assert_eq!(
            app.clone().oneshot(browser_origin).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let mut bad_host = request(r#"{"jsonrpc":"2.0","id":1,"method":"ego.getBrowserVersion"}"#);
        bad_host
            .headers_mut()
            .insert(header::HOST, HeaderValue::from_static("example.com:9223"));
        assert_eq!(
            app.clone().oneshot(bad_host).await.unwrap().status(),
            StatusCode::FORBIDDEN
        );

        let response = app
            .clone()
            .oneshot(request(
                r#"{"jsonrpc":"2.0","id":1,"method":"ui.navigate"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
        let body = response.into_body().collect().await.unwrap().to_bytes();
        assert_eq!(
            serde_json::from_slice::<Value>(&body).unwrap()["error"]["code"],
            -32601
        );
        assert_eq!(backend.0.load(Ordering::SeqCst), 0);
    }

    #[tokio::test]
    async fn notifications_execute_without_a_response() {
        let backend = Arc::new(FakeBackend::default());
        let app = router(ServerState::with_shared_backend(
            backend.clone(),
            TOKEN,
            LISTEN,
        ));
        let response = app
            .clone()
            .oneshot(request(
                r#"{"jsonrpc":"2.0","method":"ego.getBrowserVersion"}"#,
            ))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::NO_CONTENT);
        assert_eq!(backend.0.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn ids_round_trip_and_invalid_notifications_stay_silent() {
        let backend = Arc::new(FakeBackend::default());
        let app = router(ServerState::with_shared_backend(
            backend.clone(),
            TOKEN,
            LISTEN,
        ));
        for id in [json!(null), json!(-7), json!(1.25)] {
            let raw = json!({"jsonrpc":"2.0","id":id,"method":"ego.listProfiles"}).to_string();
            let response = app.clone().oneshot(request(&raw)).await.unwrap();
            let bytes = response.into_body().collect().await.unwrap().to_bytes();
            assert_eq!(serde_json::from_slice::<Value>(&bytes).unwrap()["id"], id);
        }
        let before = backend.0.load(Ordering::SeqCst);
        for method in ["ui.navigate", "ego.unknown", "ego.createTab"] {
            let response = app
                .clone()
                .oneshot(request(
                    &json!({"jsonrpc":"2.0","method":method}).to_string(),
                ))
                .await
                .unwrap();
            assert_eq!(response.status(), StatusCode::NO_CONTENT);
        }
        assert_eq!(backend.0.load(Ordering::SeqCst), before);
    }

    #[tokio::test]
    async fn oversized_requests_are_rejected() {
        let app = router(ServerState::new(FakeBackend::default(), TOKEN, LISTEN));
        let request = request(&format!("{} ", " ".repeat(MAX_REQUEST_BYTES)));
        assert_eq!(
            app.oneshot(request).await.unwrap().status(),
            StatusCode::PAYLOAD_TOO_LARGE
        );
    }

    #[test]
    fn listener_must_be_loopback() {
        assert!(ensure_loopback("127.0.0.1:9223".parse().unwrap()).is_ok());
        assert!(ensure_loopback("0.0.0.0:9223".parse().unwrap()).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn token_file_is_persisted_with_private_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("token");
        let token = load_or_create_token(&path).unwrap();
        assert_eq!(token.len(), 64);
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        assert_eq!(load_or_create_token(&path).unwrap(), token);
    }
}
