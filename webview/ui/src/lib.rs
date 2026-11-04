use std::env;
use std::path::PathBuf;
use std::process::Command as ProcessCommand;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender, TryRecvError};
use std::thread;
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use gpui::{
    App, AppContext as _, Bounds, Context, ImageSource, IntoElement, ObjectFit, Render,
    RenderImage, WindowBounds, WindowOptions, div, img, prelude::*, px, size,
};
use image::{Frame, RgbaImage};
use serde_json::{Value, json};
use tungstenite::client::IntoClientRequest;
use tungstenite::http::header::AUTHORIZATION;
use tungstenite::{Message, WebSocket, connect as ws_connect};

const VIEWPORT: (u32, u32) = (1280, 800);
type RpcSocket = WebSocket<tungstenite::stream::MaybeTlsStream<std::net::TcpStream>>;

pub struct BrowserConfig {
    pub endpoint: String,
    pub token: String,
    pub tab: u64,
}

enum Command {
    Back,
    Forward,
    Reload,
    Click { x: f32, y: f32 },
    Shutdown,
}

enum Event {
    Page {
        image: Arc<RgbaImage>,
        title: String,
        url: String,
    },
    Failed(String),
}

pub struct BrowserView {
    commands: Sender<Command>,
    events: Receiver<Event>,
    image: Option<Arc<RenderImage>>,
    retired_images: Vec<Arc<RenderImage>>,
    title: String,
    url: String,
    error: Option<String>,
}

impl BrowserView {
    pub fn with_config(url: String, config: BrowserConfig, cx: &mut Context<Self>) -> Self {
        let (command_tx, command_rx) = mpsc::channel();
        let (event_tx, event_rx) = mpsc::channel();
        let initial_url = url.clone();

        thread::Builder::new()
            .name("webview-rpc-client".into())
            .spawn(move || run_client(command_rx, event_tx, config, initial_url))
            .expect("failed to start webview RPC client");

        let view = Self {
            commands: command_tx,
            events: event_rx,
            image: None,
            retired_images: Vec::new(),
            title: "Starting Servo…".into(),
            url,
            error: None,
        };

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                if this.update(cx, |view, cx| view.receive_events(cx)).is_err() {
                    break;
                }
            }
        })
        .detach();
        view
    }

    fn receive_events(&mut self, cx: &mut Context<Self>) {
        let mut changed = false;
        loop {
            match self.events.try_recv() {
                Ok(Event::Page { image, title, url }) => {
                    self.replace_image(gpui_image(image));
                    self.title = title;
                    self.url = url;
                    self.error = None;
                    changed = true;
                },
                Ok(Event::Failed(error)) => {
                    self.error = Some(error);
                    changed = true;
                },
                Err(TryRecvError::Empty | TryRecvError::Disconnected) => break,
            }
        }
        if changed {
            cx.notify();
        }
    }

    fn replace_image(&mut self, image: Arc<RenderImage>) {
        if let Some(previous) = self.image.replace(image) {
            self.retired_images.push(previous);
        }
    }
}

impl Drop for BrowserView {
    fn drop(&mut self) {
        let _ = self.commands.send(Command::Shutdown);
    }
}

impl Render for BrowserView {
    fn render(&mut self, window: &mut gpui::Window, cx: &mut Context<Self>) -> impl IntoElement {
        for image in self.retired_images.drain(..) {
            ImageSource::Render(image).evict(Some(window), cx);
        }
        let back = self.commands.clone();
        let forward = self.commands.clone();
        let reload = self.commands.clone();
        let click = self.commands.clone();
        let page_ready = self.image.is_some();
        let page = self
            .image
            .clone()
            .map(|image| {
                img(image)
                    .size_full()
                    .object_fit(ObjectFit::Contain)
                    .into_any_element()
            })
            .unwrap_or_else(|| {
                div()
                    .flex_1()
                    .flex()
                    .items_center()
                    .justify_center()
                    .child(if let Some(error) = &self.error {
                        error.clone()
                    } else {
                        "Loading page…".to_owned()
                    })
                    .into_any_element()
            });

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(gpui::rgb(0x171717))
            .text_color(gpui::rgb(0xe5e5e5))
            .child(
                div()
                    .h(px(42.))
                    .flex()
                    .items_center()
                    .gap(px(8.))
                    .px(px(12.))
                    .border_b_1()
                    .border_color(gpui::rgb(0x303030))
                    .child(toolbar_button("‹", move || {
                        let _ = back.send(Command::Back);
                    }))
                    .child(toolbar_button("›", move || {
                        let _ = forward.send(Command::Forward);
                    }))
                    .child(toolbar_button("↻", move || {
                        let _ = reload.send(Command::Reload);
                    }))
                    .child(
                        div()
                            .flex_1()
                            .text_sm()
                            .text_color(gpui::rgb(0xb8b8b8))
                            .child(self.url.clone()),
                    )
                    .child(div().text_sm().child(self.title.clone())),
            )
            .child(
                div()
                    .flex_1()
                    .size_full()
                    .id("browser-page")
                    .on_click(move |event, window, _| {
                        if !page_ready {
                            return;
                        }
                        let bounds = window.bounds();
                        let scale = window.scale_factor();
                        let page_width = f32::from(bounds.size.width) * scale;
                        let page_height = (f32::from(bounds.size.height) - 42.0) * scale;
                        let image_width = VIEWPORT.0 as f32;
                        let image_height = VIEWPORT.1 as f32;
                        let image_scale =
                            (page_width / image_width).min(page_height / image_height);
                        if image_scale <= 0.0 {
                            return;
                        }
                        let image_x = (page_width - image_width * image_scale) / 2.0;
                        let image_y = (page_height - image_height * image_scale) / 2.0;
                        let position = event.position();
                        let x = f32::from(position.x) * scale - image_x;
                        let y = (f32::from(position.y) - 42.0) * scale - image_y;
                        if x >= 0.0
                            && y >= 0.0
                            && x < image_width * image_scale
                            && y < image_height * image_scale
                        {
                            let _ = click.send(Command::Click {
                                x: x / image_scale,
                                y: y / image_scale,
                            });
                        }
                    })
                    .child(page),
            )
    }
}

fn toolbar_button(label: &'static str, action: impl Fn() + 'static) -> impl IntoElement {
    div()
        .px(px(8.))
        .py(px(4.))
        .rounded(px(4.))
        .cursor_pointer()
        .hover(|style| style.bg(gpui::rgb(0x303030)))
        .id(label)
        .on_click(move |_, _, _| action())
        .child(label)
}

fn gpui_image(image: Arc<RgbaImage>) -> Arc<RenderImage> {
    let mut image = (*image).clone();
    for pixel in image.pixels_mut() {
        pixel.0.swap(0, 2);
    }
    Arc::new(RenderImage::new(vec![Frame::new(image)]))
}

fn connect(config: &BrowserConfig) -> Result<RpcSocket, String> {
    let deadline = Instant::now() + Duration::from_secs(15);
    loop {
        let mut request = config
            .endpoint
            .clone()
            .into_client_request()
            .map_err(|error| error.to_string())?;
        let authorization = format!("Bearer {}", config.token)
            .parse()
            .map_err(|error: tungstenite::http::header::InvalidHeaderValue| error.to_string())?;
        request.headers_mut().insert(AUTHORIZATION, authorization);
        match ws_connect(request) {
            Ok((socket, _)) => return Ok(socket),
            Err(error) => {
                if Instant::now() >= deadline {
                    return Err(format!("could not connect to Servo: {error}"));
                }
                thread::sleep(Duration::from_millis(100));
            },
        }
    }
}

fn rpc_call(
    socket: &mut RpcSocket,
    next_id: &mut u64,
    method: &str,
    params: Value,
) -> Result<Value, String> {
    let id = *next_id;
    *next_id += 1;
    socket
        .send(Message::Text(
            json!({
                "jsonrpc": "2.0",
                "id": id,
                "method": method,
                "params": params,
            })
            .to_string()
            .into(),
        ))
        .map_err(|error| error.to_string())?;
    loop {
        match socket.read().map_err(|error| error.to_string())? {
            Message::Text(response) => {
                let response: Value =
                    serde_json::from_str(response.as_ref()).map_err(|error| error.to_string())?;
                if response.get("id").and_then(Value::as_u64) != Some(id) {
                    continue;
                }
                if let Some(error) = response.get("error") {
                    return Err(error
                        .get("message")
                        .and_then(Value::as_str)
                        .unwrap_or("unknown RPC error")
                        .to_owned());
                }
                return Ok(response.get("result").cloned().unwrap_or(Value::Null));
            },
            Message::Ping(payload) => socket
                .send(Message::Pong(payload))
                .map_err(|error| error.to_string())?,
            Message::Close(_) => return Err("Servo RPC connection closed".into()),
            _ => {},
        }
    }
}

fn run_client(
    commands: Receiver<Command>,
    events: Sender<Event>,
    config: BrowserConfig,
    initial_url: String,
) {
    let result = (|| {
        let mut socket = connect(&config)?;
        let mut next_id = 1;
        let mut next_capture = Instant::now();
        loop {
            match commands.recv_timeout(Duration::from_millis(10)) {
                Ok(Command::Back) => {
                    rpc_call(
                        &mut socket,
                        &mut next_id,
                        "ego.goBack",
                        json!({ "tab": config.tab }),
                    )?;
                },
                Ok(Command::Forward) => {
                    rpc_call(
                        &mut socket,
                        &mut next_id,
                        "ego.goForward",
                        json!({ "tab": config.tab }),
                    )?;
                },
                Ok(Command::Reload) => {
                    rpc_call(
                        &mut socket,
                        &mut next_id,
                        "ego.reload",
                        json!({ "tab": config.tab }),
                    )?;
                },
                Ok(Command::Click { x, y }) => {
                    rpc_call(
                        &mut socket,
                        &mut next_id,
                        "ego.clickAt",
                        json!({ "tab": config.tab, "x": x, "y": y }),
                    )?;
                },
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Err(mpsc::RecvTimeoutError::Timeout) => {},
            }
            if Instant::now() >= next_capture {
                let capture = match rpc_call(
                    &mut socket,
                    &mut next_id,
                    "ego.capture",
                    json!({ "tab": config.tab }),
                ) {
                    Ok(capture) => capture,
                    Err(error) => {
                        let _ = events.send(Event::Failed(error));
                        next_capture = Instant::now() + Duration::from_millis(250);
                        continue;
                    },
                };
                let data = capture
                    .get("data")
                    .and_then(Value::as_str)
                    .ok_or("capture response did not contain image data")?;
                let bytes = STANDARD.decode(data).map_err(|error| error.to_string())?;
                let image = image::load_from_memory(&bytes)
                    .map_err(|error| error.to_string())?
                    .to_rgba8();
                let title = capture
                    .get("title")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_owned();
                let url = capture
                    .get("url")
                    .and_then(Value::as_str)
                    .unwrap_or(&initial_url)
                    .to_owned();
                if events
                    .send(Event::Page {
                        image: Arc::new(image),
                        title,
                        url,
                    })
                    .is_err()
                {
                    break;
                }
                next_capture = Instant::now() + Duration::from_millis(100);
            }
        }
        Ok::<(), String>(())
    })();

    if let Err(error) = result {
        let _ = events.send(Event::Failed(error));
    }
}

pub fn run(url: String) -> Result<(), String> {
    let token = match env::var("SERVO_WEBVIEW_TOKEN") {
        Ok(token) if !token.is_empty() => token,
        Ok(_) => return Err("SERVO_WEBVIEW_TOKEN must not be empty".into()),
        Err(env::VarError::NotPresent) => random_token()?,
        Err(env::VarError::NotUnicode(_)) => {
            return Err("SERVO_WEBVIEW_TOKEN must be valid Unicode".into());
        },
    };
    let port = env::var("SERVO_WEBVIEW_PORT")
        .unwrap_or_else(|_| "9239".into())
        .parse::<u16>()
        .map_err(|error| format!("invalid SERVO_WEBVIEW_PORT: {error}"))?;
    let profile = dirs::data_local_dir().map(|path| path.join("servo-webview"));
    let engine = engine_executable();
    let mut child = ProcessCommand::new(&engine)
        .arg(&url)
        .env("SERVO_WEBVIEW_TOKEN", &token)
        .env("SERVO_WEBVIEW_PORT", port.to_string())
        .envs(profile.iter().map(|path| ("SERVO_WEBVIEW_PROFILE", path)))
        .spawn()
        .map_err(|error| {
            format!(
                "failed to launch {}: {error}; build it with `cargo build -p webview-engine --bin servo-webview-engine`",
                engine.display()
            )
        })?;

    let app = gpui_platform::application();
    app.run(move |cx: &mut App| {
        let bounds = Bounds::centered(None, size(px(1280.), px(800.)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_, cx| {
                cx.new(|cx| {
                    BrowserView::with_config(
                        url.clone(),
                        BrowserConfig {
                            endpoint: format!("ws://127.0.0.1:{port}"),
                            token: token.clone(),
                            tab: 1,
                        },
                        cx,
                    )
                })
            },
        )
        .expect("failed to open browser window");
    });
    let _ = child.kill();
    let _ = child.wait();
    Ok(())
}

fn engine_executable() -> PathBuf {
    if let Some(path) = env::var_os("SERVO_WEBVIEW_ENGINE") {
        return PathBuf::from(path);
    }
    let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../..");
    let target = env::var_os("CARGO_TARGET_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| root.join("target"));
    let profile = if cfg!(debug_assertions) {
        "debug"
    } else {
        "release"
    };
    let executable = if cfg!(windows) {
        "servo-webview-engine.exe"
    } else {
        "servo-webview-engine"
    };
    target.join(profile).join(executable)
}

fn random_token() -> Result<String, String> {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut bytes = [0; 32];
    getrandom::fill(&mut bytes)
        .map_err(|error| format!("failed to generate RPC token: {error}"))?;
    let mut token = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        token.push(HEX[(byte >> 4) as usize] as char);
        token.push(HEX[(byte & 0x0f) as usize] as char);
    }
    Ok(token)
}
