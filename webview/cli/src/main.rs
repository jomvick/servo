use std::net::SocketAddr;
use std::path::PathBuf;

use anyhow::{Context, bail};
use axum::serve;
use clap::{Parser, Subcommand};
use reqwest::header::{AUTHORIZATION, CONTENT_TYPE};
use serde_json::Value;

#[global_allocator]
static ALLOCATOR: mimalloc::MiMalloc = mimalloc::MiMalloc;
use webview_runtime::{Browser, BrowserClient, RuntimeConfig};

use ego_lite::{
    RpcBackend, ServerState, ensure_loopback, load_or_create_token, router, token_file_path,
};

#[derive(Parser)]
#[command(
    name = "ego-lite",
    version,
    about = "Standalone local ego.* browser runtime"
)]
struct Cli {
    #[arg(long, default_value = "127.0.0.1:9223", global = true)]
    listen: SocketAddr,
    #[arg(long, global = true)]
    data_dir: Option<PathBuf>,
    #[arg(long, global = true)]
    token_file: Option<PathBuf>,
    #[arg(long, global = true)]
    headless: bool,
    #[command(subcommand)]
    command: Option<Command>,
    #[arg(value_name = "INITIAL_URL", default_value = "about:blank")]
    initial_url: String,
}

#[derive(Subcommand)]
enum Command {
    Call { method: String, json_params: String },
}

struct Client(BrowserClient);

impl RpcBackend for Client {
    fn request_rpc(
        &self,
        method: &str,
        params: Value,
    ) -> Result<Value, webview_bridge::ErrorObject> {
        self.0.request_rpc(method, params)
    }
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    if rustls::crypto::CryptoProvider::get_default().is_none() {
        let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
    }
    let listen = ensure_loopback(cli.listen)?;
    let defaults = RuntimeConfig::default();
    let data_dir = cli.data_dir.unwrap_or(defaults.data_dir);
    let token_path = token_file_path(&data_dir, cli.token_file.as_deref());

    if let Some(Command::Call {
        method,
        json_params,
    }) = cli.command
    {
        let params: Value = serde_json::from_str(&json_params).context("invalid JSON params")?;
        webview_bridge::Request {
            id: None,
            method: method.clone(),
            params: params.clone(),
        }
        .call()
        .map_err(|error| anyhow::anyhow!("{}", error.message))?;
        let token = load_or_create_token(&token_path)?;
        let response = reqwest::Client::builder()
            .no_proxy()
            .timeout(std::time::Duration::from_secs(35))
            .build()?
            .post(format!("http://{listen}/rpc"))
            .header(AUTHORIZATION, format!("Bearer {token}"))
            .header(CONTENT_TYPE, "application/json")
            .json(&serde_json::json!({
                "jsonrpc": "2.0",
                "id": 1,
                "method": method,
                "params": params
            }))
            .send()
            .await
            .context("could not reach the local ego-lite RPC server")?;
        let status = response.status();
        let body = response.text().await?;
        if !status.is_success() {
            bail!("RPC server returned {status}: {body}");
        }
        println!("{body}");
        if serde_json::from_str::<Value>(&body)?.get("error").is_some() {
            bail!("RPC returned an error");
        }
        return Ok(());
    }

    let initial_url = if cli.initial_url == "about:blank" {
        defaults.initial_url
    } else {
        cli.initial_url
    };
    #[cfg(not(feature = "headed"))]
    if !cli.headless {
        bail!("this build has no GPUI shell; use --headless or enable the headed feature");
    }
    let listener = tokio::net::TcpListener::bind(listen)
        .await
        .with_context(|| format!("could not bind {listen}"))?;
    let listen = listener.local_addr()?;
    let token = load_or_create_token(&token_path)?;
    let browser = Browser::start(RuntimeConfig {
        data_dir,
        initial_url,
    })
    .map_err(anyhow::Error::msg)
    .context("could not start the Servo runtime")?;
    let state = ServerState::new(Client(browser.client()), token, listen);
    eprintln!(
        "ego-lite RPC listening on http://{listen}/rpc (token file: {})",
        token_path.display()
    );
    let (stop, stopped) = tokio::sync::oneshot::channel::<()>();
    let mut server = tokio::spawn(async move {
        serve(listener, router(state))
            .with_graceful_shutdown(async {
                let _ = stopped.await;
            })
            .await
    });
    let outcome = if cli.headless {
        tokio::select! {
            result = tokio::signal::ctrl_c() => result.context("failed to listen for Ctrl-C"),
            result = &mut server => {
                browser.shutdown().map_err(anyhow::Error::msg)?;
                result.context("RPC server task failed")?.context("RPC server failed")?;
                bail!("RPC server stopped unexpectedly");
            },
        }
    } else {
        #[cfg(feature = "headed")]
        {
            webview_ui::run(browser.client())
        }
        #[cfg(not(feature = "headed"))]
        {
            unreachable!("validated before engine startup")
        }
    };
    let _ = stop.send(());
    match tokio::time::timeout(std::time::Duration::from_secs(2), server).await {
        Ok(result) => result
            .context("RPC server task failed")?
            .context("RPC server failed")?,
        Err(_) => eprintln!("RPC shutdown deadline reached"),
    }
    browser.shutdown().map_err(anyhow::Error::msg)?;
    outcome
}
