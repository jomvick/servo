use std::io;
use std::net::{TcpListener, TcpStream};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread;
use std::time::Duration;

use tungstenite::handshake::{HandshakeError, server::Request};
use tungstenite::http::{Response, StatusCode};
use tungstenite::{Message, accept_hdr};

pub type Dispatcher = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

pub fn bind(port: u16) -> io::Result<TcpListener> {
    let listener = TcpListener::bind(("127.0.0.1", port))?;
    listener.set_nonblocking(true)?;
    Ok(listener)
}

pub fn serve(port: u16, token: String, dispatch: Dispatcher) -> io::Result<()> {
    serve_until(port, token, dispatch, Arc::new(AtomicBool::new(false)))
}

pub fn serve_until(
    port: u16,
    token: String,
    dispatch: Dispatcher,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    serve_listener(bind(port)?, token, dispatch, shutdown)
}

pub fn serve_listener(
    listener: TcpListener,
    token: String,
    dispatch: Dispatcher,
    shutdown: Arc<AtomicBool>,
) -> io::Result<()> {
    let address = listener.local_addr()?;
    eprintln!("webview RPC listening at ws://{address} (token auth required)");

    while !shutdown.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => {
                let token = token.clone();
                let dispatch = dispatch.clone();
                let shutdown = shutdown.clone();
                thread::Builder::new()
                    .name("webview-rpc-client".into())
                    .spawn(move || {
                        if let Err(error) = serve_client(stream, &token, &dispatch, shutdown) {
                            eprintln!("webview RPC client disconnected: {error}");
                        }
                    })?;
            },
            Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                thread::sleep(Duration::from_millis(25));
            },
            Err(error) => return Err(error),
        }
    }
    Ok(())
}

fn serve_client(
    stream: TcpStream,
    token: &str,
    dispatch: &Dispatcher,
    shutdown: Arc<AtomicBool>,
) -> tungstenite::Result<()> {
    stream.set_read_timeout(Some(Duration::from_millis(100)))?;
    let mut socket = accept_hdr(stream, |request: &Request, response: Response<()>| {
        if authorized(request, token) {
            Ok(response)
        } else {
            Err(Response::builder()
                .status(StatusCode::UNAUTHORIZED)
                .body(Some("token authentication required".to_owned()))
                .expect("static unauthorized response is valid"))
        }
    })
    .map_err(|error| match error {
        HandshakeError::Failure(error) => error,
        HandshakeError::Interrupted(_) => tungstenite::Error::Io(io::Error::new(
            io::ErrorKind::TimedOut,
            "websocket handshake timed out",
        )),
    })?;

    loop {
        if shutdown.load(Ordering::Relaxed) {
            let _ = socket.close(None);
            return Ok(());
        }
        match socket.read() {
            Ok(Message::Text(request)) => {
                if let Some(reply) = dispatch(&request) {
                    socket.send(Message::Text(reply.into()))?;
                }
            },
            Ok(Message::Close(_)) => {
                socket.close(None)?;
                return Ok(());
            },
            Ok(Message::Ping(payload)) => socket.send(Message::Pong(payload))?,
            Ok(_) => {},
            Err(tungstenite::Error::ConnectionClosed | tungstenite::Error::AlreadyClosed) => {
                return Ok(());
            },
            Err(tungstenite::Error::Io(error))
                if matches!(
                    error.kind(),
                    io::ErrorKind::WouldBlock | io::ErrorKind::TimedOut
                ) => {},
            Err(error) => return Err(error),
        }
    }
}

fn authorized(request: &Request, token: &str) -> bool {
    let Some(value) = request.headers().get("authorization") else {
        return false;
    };
    let Ok(value) = value.to_str() else {
        return false;
    };
    let expected = ["Bearer", token].join(" ");
    constant_time_eq(value.as_bytes(), expected.as_bytes())
}

fn constant_time_eq(actual: &[u8], expected: &[u8]) -> bool {
    if actual.len() != expected.len() {
        return false;
    }
    actual
        .iter()
        .zip(expected)
        .fold(0_u8, |difference, (left, right)| {
            difference | (left ^ right)
        })
        == 0
}

#[cfg(test)]
mod tests {
    use super::*;
    use tungstenite::http::Request as HttpRequest;

    #[test]
    fn bearer_authentication_requires_exact_header_value() {
        let valid = HttpRequest::builder()
            .header("authorization", ["Bearer", "secret"].join(" "))
            .body(())
            .unwrap();
        let wrong_scheme = HttpRequest::builder()
            .header("authorization", "Basic secret")
            .body(())
            .unwrap();
        assert!(authorized(&valid, "secret"));
        assert!(!authorized(&wrong_scheme, "secret"));
        assert!(!authorized(&valid, "other"));
    }
}
