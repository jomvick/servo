use std::env;
use std::process::ExitCode;
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use webview_bridge::server::{self, Dispatcher};
use webview_core::ProfileId;
use webview_engine::{Engine, EngineConfig};

enum Command {
    Rpc {
        request: String,
        reply: mpsc::Sender<Option<String>>,
    },
}

fn main() -> ExitCode {
    match run() {
        Ok(()) => ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("webview engine failed: {error}");
            ExitCode::FAILURE
        },
    }
}

fn run() -> Result<(), String> {
    let url = env::args()
        .nth(1)
        .unwrap_or_else(|| "https://example.com".to_owned());
    url::Url::parse(&url).map_err(|error| format!("invalid URL: {error}"))?;

    let profile_dir = env::var_os("SERVO_WEBVIEW_PROFILE")
        .map(std::path::PathBuf::from)
        .or_else(|| dirs::data_local_dir().map(|path| path.join("servo-webview")));
    let mut engine = Engine::new(EngineConfig {
        profile_dir,
        ..EngineConfig::default()
    })
    .map_err(|error| error.to_string())?;
    let space = engine.create_space("Browser", ProfileId("default".into()));
    let _tab = engine
        .create_tab(space, Some(&url))
        .map_err(|error| error.to_string())?;

    let token = env::var("SERVO_WEBVIEW_TOKEN")
        .map_err(|_| "SERVO_WEBVIEW_TOKEN must be set by the frontend".to_owned())?;
    if token.is_empty() {
        return Err("SERVO_WEBVIEW_TOKEN must not be empty".into());
    }
    let port = env::var("SERVO_WEBVIEW_PORT")
        .unwrap_or_else(|_| "9239".into())
        .parse::<u16>()
        .map_err(|error| format!("invalid SERVO_WEBVIEW_PORT: {error}"))?;
    let listener =
        server::bind(port).map_err(|error| format!("failed to bind RPC listener: {error}"))?;

    let (commands, requests) = mpsc::channel();
    let dispatch: Dispatcher = std::sync::Arc::new(move |request| {
        let (reply, response) = mpsc::channel();
        if commands
            .send(Command::Rpc {
                request: request.to_owned(),
                reply,
            })
            .is_err()
        {
            return Some(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"engine stopped"}}"#
                    .to_owned(),
            );
        }
        response.recv().unwrap_or_else(|_| {
            Some(
                r#"{"jsonrpc":"2.0","id":null,"error":{"code":-32603,"message":"engine stopped"}}"#
                    .to_owned(),
            )
        })
    });
    thread::Builder::new()
        .name("webview-rpc".into())
        .spawn(move || {
            if let Err(error) = server::serve_listener(
                listener,
                token,
                dispatch,
                std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false)),
            ) {
                eprintln!("webview RPC server failed: {error}");
            }
        })
        .map_err(|error| format!("failed to start RPC server thread: {error}"))?;

    loop {
        match requests.recv_timeout(Duration::from_millis(8)) {
            Ok(Command::Rpc { request, reply }) => {
                let _ = reply.send(engine.handle_rpc(&request));
            },
            Err(mpsc::RecvTimeoutError::Disconnected) => return Ok(()),
            Err(mpsc::RecvTimeoutError::Timeout) => {},
        }
        engine.pump();
        engine.take_events();
    }
}
