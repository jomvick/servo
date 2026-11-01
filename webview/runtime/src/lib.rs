//! Thread-safe embedding boundary. Servo and its GL objects never leave the engine thread.
use std::collections::HashMap;
use std::io::Cursor;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use base64::Engine as _;
use image::RgbaImage;
use serde_json::{Value, json};
use webview_bridge::{Call, ErrorObject, Request};
use webview_core::{EgoError, ErrorCode, ProfileId, TabId};
use webview_engine::{Engine, EngineConfig, EngineEvent};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(30);
static STARTED: OnceLock<()> = OnceLock::new();
type Frames = Arc<Mutex<HashMap<TabId, Arc<RgbaImage>>>>;

#[derive(Clone, Debug)]
pub struct RuntimeConfig {
    pub data_dir: PathBuf,
    pub initial_url: String,
}
impl Default for RuntimeConfig {
    fn default() -> Self {
        let base = std::env::var_os("XDG_DATA_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
            .unwrap_or_else(|| PathBuf::from("."));
        Self {
            data_dir: base.join("ego-lite"),
            initial_url: "about:blank".into(),
        }
    }
}

struct Command {
    method: String,
    params: Value,
    deadline: Instant,
    reply: mpsc::SyncSender<Result<Value, ErrorObject>>,
}
enum Message {
    Request(Command),
    Shutdown,
}

#[derive(Clone)]
pub struct BrowserClient {
    sender: mpsc::SyncSender<Message>,
    frames: Frames,
    engine_thread: thread::Thread,
}
impl BrowserClient {
    pub fn request(&self, method: &str, params: Value) -> Result<Value, String> {
        self.request_rpc(method, params).map_err(|e| e.message)
    }
    pub fn request_rpc(&self, method: &str, params: Value) -> Result<Value, ErrorObject> {
        let (reply, receiver) = mpsc::sync_channel(1);
        let command = Command {
            method: method.into(),
            params,
            deadline: Instant::now() + REQUEST_TIMEOUT,
            reply,
        };
        self.sender
            .try_send(Message::Request(command))
            .map_err(|_| disconnected("engine unavailable or command queue full"))?;
        self.engine_thread.unpark();
        receiver
            .recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| disconnected("engine request timed out or disconnected"))?
    }
    pub fn latest_frame(&self, tab: u64) -> Option<Arc<RgbaImage>> {
        self.frames.lock().ok()?.get(&TabId(tab)).cloned()
    }
}

pub struct Browser {
    client: BrowserClient,
    thread: Option<JoinHandle<()>>,
}
impl Browser {
    pub fn start(config: RuntimeConfig) -> Result<Self, String> {
        std::fs::create_dir_all(&config.data_dir).map_err(|e| e.to_string())?;
        STARTED
            .set(())
            .map_err(|_| "only one Servo runtime may be started per process".to_owned())?;
        let (sender, receiver) = mpsc::sync_channel(64);
        let (ready, initialized) = mpsc::sync_channel(1);
        let frames = Frames::default();
        let output = frames.clone();
        let thread = thread::Builder::new()
            .name("servo-engine".into())
            .spawn(move || {
                let result = Engine::new(EngineConfig {
                    data_dir: Some(config.data_dir),
                    ..EngineConfig::default()
                });
                let mut engine = match result {
                    Ok(engine) => engine,
                    Err(error) => {
                        let _ = ready.send(Err(error.to_string()));
                        return;
                    },
                };
                engine.install_preload(webview_bridge::automation::PRELOAD);
                let profile = ProfileId("default".into());
                let space = engine.create_space("Browser", profile.clone());
                if let Err(error) = engine.create_tab(space, Some(&config.initial_url)) {
                    let _ = ready.send(Err(error.to_string()));
                    return;
                }
                let _ = ready.send(Ok(()));
                loop {
                    engine.pump();
                    collect_frames(&mut engine, &output);
                    match receiver.try_recv() {
                        Ok(Message::Shutdown) | Err(mpsc::TryRecvError::Disconnected) => break,
                        Err(mpsc::TryRecvError::Empty) => {
                            if !engine.waked() {
                                thread::park();
                            }
                        },
                        Ok(Message::Request(command)) => {
                            if Instant::now() >= command.deadline {
                                continue;
                            }
                            let result = dispatch(
                                &mut engine,
                                &output,
                                &profile,
                                &command.method,
                                command.params,
                            );
                            collect_frames(&mut engine, &output);
                            let _ = command.reply.send(result);
                        },
                    }
                }
            })
            .map_err(|e| e.to_string())?;
        match initialized.recv_timeout(Duration::from_secs(60)) {
            Ok(Ok(())) => Ok(Self {
                client: BrowserClient {
                    sender,
                    frames,
                    engine_thread: thread.thread().clone(),
                },
                thread: Some(thread),
            }),
            Ok(Err(error)) => {
                let _ = thread.join();
                Err(error)
            },
            Err(error) => {
                let _ = sender.try_send(Message::Shutdown);
                thread.thread().unpark();
                Err(format!("engine startup failed: {error}"))
            },
        }
    }
    pub fn client(&self) -> BrowserClient {
        self.client.clone()
    }
    pub fn shutdown(mut self) -> Result<(), String> {
        self.stop()
    }
    fn stop(&mut self) -> Result<(), String> {
        if let Some(thread) = self.thread.take() {
            let _ = self.client.sender.send(Message::Shutdown);
            self.client.engine_thread.unpark();
            thread
                .join()
                .map_err(|_| "engine thread panicked".to_owned())?;
        }
        Ok(())
    }
}
impl Drop for Browser {
    fn drop(&mut self) {
        let _ = self.stop();
    }
}

fn collect_frames(engine: &mut Engine, frames: &Frames) {
    let Ok(mut frames) = frames.lock() else {
        return;
    };
    for event in engine.take_events() {
        match event {
            EngineEvent::Frame { tab, image } => {
                frames.insert(tab, image);
            },
            EngineEvent::Closed { tab } | EngineEvent::Crashed { tab, .. } => {
                frames.remove(&tab);
            },
            _ => {},
        }
    }
}
fn disconnected(message: &str) -> ErrorObject {
    ErrorObject::ego(&EgoError::new(ErrorCode::TaskHostDisconnected, message))
}
fn agent_tab(engine: &Engine, tab: TabId) -> Result<(), EgoError> {
    engine
        .registry()
        .require_agent_control(engine.registry().tab(tab)?.space)?;
    Ok(())
}

fn dispatch(
    engine: &mut Engine,
    frames: &Frames,
    profile: &ProfileId,
    method: &str,
    params: Value,
) -> Result<Value, ErrorObject> {
    if method.starts_with("ui.") {
        return ui_dispatch(engine, method, params).map_err(|e| ErrorObject::ego(&e));
    }
    let call = Request {
        id: None,
        method: method.into(),
        params,
    }
    .call()?;
    dispatch_call(engine, frames, profile, call).map_err(|e| ErrorObject::ego(&e))
}
fn dispatch_call(
    engine: &mut Engine,
    frames: &Frames,
    profile: &ProfileId,
    call: Call,
) -> Result<Value, EgoError> {
    use Call::*;
    match call {
        GetBrowserVersion => Ok(
            json!({"name":"ego-lite", "version":env!("CARGO_PKG_VERSION"), "engine":"Servo", "protocolVersion":"1"}),
        ),
        ListProfiles => Ok(json!([{"id":profile.0,"name":"Default"}])),
        ListTaskSpaces => Ok(json!(engine.registry().spaces().collect::<Vec<_>>())),
        CreateTaskSpace { name } => {
            let id = engine.create_space(&name, profile.clone());
            Ok(json!(engine.registry().space(id)?))
        },
        ClaimTaskSpace { space } => {
            engine.registry_mut().claim(space)?;
            Ok(json!(engine.registry().space(space)?))
        },
        HandOffTaskSpace { space } => {
            engine.registry_mut().hand_off(space)?;
            Ok(json!(engine.registry().space(space)?))
        },
        TakeOverTaskSpace { space } => {
            engine.registry_mut().take_over(space)?;
            Ok(json!(engine.registry().space(space)?))
        },
        CompleteTaskSpace { space } => {
            engine.registry().require_agent_control(space)?;
            engine.registry_mut().complete(space)?;
            Ok(Value::Null)
        },
        CloseTaskSpace { space } => {
            // Completed spaces remain closable, user-owned spaces do not.
            if engine.registry().space(space)?.ownership == webview_core::Ownership::User {
                engine.registry().require_agent_control(space)?;
            }
            let tabs = engine.registry().space(space)?.tabs.clone();
            for tab in tabs {
                engine.close_tab(tab)?;
            }
            engine.registry_mut().close(space)?;
            Ok(Value::Null)
        },
        ListTabs { space } => {
            let ids = &engine.registry().space(space)?.tabs;
            Ok(json!(
                ids.iter()
                    .filter_map(|id| engine.registry().tab(*id).ok())
                    .collect::<Vec<_>>()
            ))
        },
        CreateTab { space, url } => {
            engine.registry().require_agent_control(space)?;
            let id = engine.create_tab(space, url.as_deref())?;
            Ok(json!(engine.registry().tab(id)?))
        },
        CloseTab { tab } => {
            agent_tab(engine, tab)?;
            engine.close_tab(tab)?;
            Ok(Value::Null)
        },
        Navigate { tab, url } => {
            agent_tab(engine, tab)?;
            engine.navigate(tab, &url)?;
            Ok(Value::Null)
        },
        Evaluate { tab, expression } => {
            agent_tab(engine, tab)?;
            engine.evaluate(tab, &expression)
        },
        Snapshot { tab, options } => {
            agent_tab(engine, tab)?;
            let raw = engine.evaluate(tab, webview_bridge::automation::SNAPSHOT_SCRIPT)?;
            Ok(json!(webview_core::parse_snapshot(&raw, &options)?))
        },
        Click { tab, reference } => {
            agent_tab(engine, tab)?;
            engine.evaluate(tab, &webview_bridge::automation::click_script(&reference))
        },
        Fill {
            tab,
            reference,
            value,
        } => {
            agent_tab(engine, tab)?;
            engine.evaluate(
                tab,
                &webview_bridge::automation::fill_script(&reference, &value),
            )
        },
        TypeText { tab, text } => {
            agent_tab(engine, tab)?;
            engine.text_input(tab, &text)?;
            Ok(Value::Null)
        },
        Scroll {
            tab,
            delta_x,
            delta_y,
        } => {
            agent_tab(engine, tab)?;
            engine.scroll(tab, 0., 0., delta_x as f32, delta_y as f32)?;
            Ok(Value::Null)
        },
        Capture { tab } => {
            agent_tab(engine, tab)?;
            engine.pump();
            collect_frames(engine, frames);
            let image = frames
                .lock()
                .ok()
                .and_then(|frames| frames.get(&tab).cloned())
                .ok_or_else(|| {
                    EgoError::new(ErrorCode::OperationFailed, "no frame available yet")
                })?;
            let mut bytes = Cursor::new(Vec::new());
            image
                .write_to(&mut bytes, image::ImageFormat::Png)
                .map_err(|e| EgoError::new(ErrorCode::OperationFailed, e.to_string()))?;
            Ok(
                json!({"mimeType":"image/png", "data":base64::prelude::BASE64_STANDARD.encode(bytes.into_inner()), "width":image.width(),"height":image.height()}),
            )
        },
        SendCdpMessage { tab, .. } => {
            agent_tab(engine, tab)?;
            Err(EgoError::new(
                ErrorCode::CdpChannelUnavailable,
                "CDP is not implemented; use the ego API",
            ))
        },
    }
}
fn ui_dispatch(engine: &mut Engine, method: &str, params: Value) -> Result<Value, EgoError> {
    let bad = || EgoError::new(ErrorCode::InvalidArgument, "invalid UI parameters");
    if method == "ui.createTab" {
        let space = webview_core::SpaceId(
            params
                .get("space")
                .and_then(Value::as_u64)
                .ok_or_else(bad)?,
        );
        let url = match params.get("url") {
            None | Some(Value::Null) => None,
            Some(Value::String(url)) => Some(url.as_str()),
            _ => return Err(bad()),
        };
        let tab = engine.create_user_tab(space, url)?;
        return Ok(json!(engine.registry().tab(tab)?));
    }
    let tab = TabId(params.get("tab").and_then(Value::as_u64).ok_or_else(bad)?);
    let number = |name: &str| {
        params
            .get(name)
            .and_then(Value::as_f64)
            .filter(|v| v.is_finite() && v.abs() <= f32::MAX as f64)
            .map(|v| v as f32)
            .ok_or_else(bad)
    };
    let text = |name: &str| params.get(name).and_then(Value::as_str).ok_or_else(bad);
    match method {
        "ui.navigate" => engine.navigate(tab, text("url")?)?,
        "ui.reload" => engine.reload(tab)?,
        "ui.closeTab" => engine.close_tab(tab)?,
        "ui.activateTab" => engine.set_active_tab(tab)?,
        "ui.back" => engine.go_back(tab)?,
        "ui.forward" => engine.go_forward(tab)?,
        "ui.resize" => {
            let width = params
                .get("width")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(bad)?;
            let height = params
                .get("height")
                .and_then(Value::as_u64)
                .and_then(|n| u32::try_from(n).ok())
                .ok_or_else(bad)?;
            if width > 8192 || height > 8192 {
                return Err(bad());
            }
            engine.resize(tab, width, height)?;
        },
        "ui.click" => engine.pointer_click(tab, number("x")?, number("y")?)?,
        "ui.scroll" => engine.scroll(
            tab,
            number("x")?,
            number("y")?,
            number("deltaX")?,
            number("deltaY")?,
        )?,
        "ui.type" => engine.text_input(tab, text("text")?)?,
        "ui.key" => engine.key_input(tab, text("key")?)?,
        _ => {
            return Err(EgoError::new(
                ErrorCode::InvalidArgument,
                "unknown UI command",
            ));
        },
    }
    Ok(Value::Null)
}
