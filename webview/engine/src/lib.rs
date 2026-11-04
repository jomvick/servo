//! L'adaptateur Servo : un `Servo`, un contexte de rendu logiciel par onglet, et des frames
//! qui ne sortent que quand l'appelant le demande.
//!
//! Deux décisions de D4 et D5 structurent ce fichier : tout est `Rc` et donc sur un seul
//! thread (le moteur n'est pas `Send`), et la peinture se paie au `pump` de l'appelant, pas
//! dans la notification du moteur — sinon la cadence d'affichage serait décidée par Servo.

mod delegate;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::io::Cursor;
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use base64::Engine as _;
use base64::engine::general_purpose::STANDARD;
use dpi::PhysicalSize;
use embedder_traits::EventLoopWaker;
use euclid::default::Point2D;
use image::{DynamicImage, ImageFormat, RgbaImage};
use paint_api::rendering_context::{RenderingContext, SoftwareRenderingContext};
use serde_json::{Map, Value, json};
use servo::{
    DeviceIntRect, DevicePoint, InputEvent, JSValue, JavaScriptEvaluationError, Key, KeyState,
    KeyboardEvent, MouseButton, MouseButtonAction, MouseButtonEvent, MouseMoveEvent, Servo,
    ServoBuilder, UserContentManager, UserScript, WebView, WebViewBuilder, WheelDelta, WheelEvent,
    WheelMode,
};
use url::Url;
use webview_bridge::protocol::{Call, ErrorObject, Reply, Request};
use webview_core::{
    EgoError, ErrorCode, ProfileId, Registry, Snapshot, SnapshotOptions, SpaceId, TabId,
    parse_snapshot,
};

use crate::delegate::TabDelegate;

/// Délai d'une évaluation JavaScript. Large parce qu'un script d'agent peut légitimement
/// attendre quelque chose ; court parce qu'un frame pendu doit rester une erreur, pas un gel.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Taille d'un onglet neuf. Un `resize(0, 0)` ne l'écrase pas.
    pub viewport: (u32, u32),
    /// Racine de stockage Servo. `None` garde le comportement de la configuration Servo.
    pub profile_dir: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            viewport: (1280, 800),
            profile_dir: None,
        }
    }
}

#[derive(Clone, Debug)]
pub enum EngineEvent {
    /// Une image RGBA, dans l'ordre des octets de `read_to_image` : le consommateur gpui paie
    /// le `swap(0, 2)` vers BGRA (D5).
    Frame {
        tab: TabId,
        image: Arc<RgbaImage>,
    },
    UrlChanged {
        tab: TabId,
        url: String,
    },
    TitleChanged {
        tab: TabId,
        title: String,
    },
    LoadStatusChanged {
        tab: TabId,
        complete: bool,
    },
    Closed {
        tab: TabId,
    },
    Crashed {
        tab: TabId,
        reason: String,
    },
}

struct Tab {
    webview: WebView,
    context: Rc<dyn RenderingContext>,
    dirty: Rc<Cell<bool>>,
}

pub struct Engine {
    servo: Servo,
    registry: Registry,
    tabs: HashMap<TabId, Tab>,
    events: Rc<RefCell<VecDeque<EngineEvent>>>,
    user_content_manager: Rc<UserContentManager>,
    wakeup: Arc<AtomicBool>,
    viewport: (u32, u32),
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self, EgoError> {
        let wakeup = Arc::new(AtomicBool::new(false));
        let servo = ServoBuilder::default()
            .opts(servo::Opts {
                config_dir: config.profile_dir,
                ..Default::default()
            })
            .event_loop_waker(Box::new(Waker(wakeup.clone())))
            .build();
        Ok(Self {
            user_content_manager: Rc::new(UserContentManager::new(&servo)),
            servo,
            registry: Registry::default(),
            tabs: HashMap::new(),
            events: Rc::new(RefCell::new(VecDeque::new())),
            wakeup,
            viewport: config.viewport,
        })
    }

    pub fn registry(&self) -> &Registry {
        &self.registry
    }

    pub fn registry_mut(&mut self) -> &mut Registry {
        &mut self.registry
    }

    pub fn viewport(&self) -> (u32, u32) {
        self.viewport
    }

    /// Posé par le moteur à chaque travail qui attend le thread de l'appelant. Un shell qui
    /// pompe à la demande ne s'occupe que de ceci entre deux redraws.
    pub fn waked(&self) -> bool {
        self.wakeup.load(Ordering::Relaxed)
    }

    pub fn create_space(&mut self, name: &str, profile: ProfileId) -> SpaceId {
        self.registry.create_space(name, profile)
    }

    pub fn create_tab(&mut self, space: SpaceId, url: Option<&str>) -> Result<TabId, EgoError> {
        let parsed = url.map(parse_url).transpose()?;
        let (width, height) = self.viewport;
        let context = rendering_context((width, height))?;
        let tab = self.registry.create_tab(space, url.map(str::to_owned))?;
        let dirty = Rc::new(Cell::new(false));
        let delegate = Rc::new(TabDelegate {
            tab,
            dirty: dirty.clone(),
            events: self.events.clone(),
        });
        let mut builder = WebViewBuilder::new(&self.servo, context.clone())
            .delegate(delegate)
            .user_content_manager(self.user_content_manager.clone());
        if let Some(parsed) = parsed {
            builder = builder.url(parsed);
        }
        let webview = builder.build();
        webview.resize(PhysicalSize::new(width, height));

        self.tabs.insert(
            tab,
            Tab {
                webview,
                context,
                dirty,
            },
        );
        Ok(tab)
    }

    pub fn navigate(&mut self, tab: TabId, url: &str) -> Result<(), EgoError> {
        let parsed = parse_url(url)?;
        let webview = self.webview(tab)?;
        webview.load(parsed.clone());
        self.registry.tab_mut(tab)?.url = Some(parsed.to_string());
        Ok(())
    }

    pub fn reload(&mut self, tab: TabId) -> Result<(), EgoError> {
        self.webview(tab)?.reload();
        Ok(())
    }

    pub fn go_back(&mut self, tab: TabId) -> Result<(), EgoError> {
        self.webview(tab)?.go_back(1);
        Ok(())
    }

    pub fn go_forward(&mut self, tab: TabId) -> Result<(), EgoError> {
        self.webview(tab)?.go_forward(1);
        Ok(())
    }

    pub fn input(&mut self, tab: TabId, event: InputEvent) -> Result<(), EgoError> {
        self.webview(tab)?.notify_input_event(event);
        Ok(())
    }

    pub fn click_at(&mut self, tab: TabId, x: f32, y: f32) -> Result<(), EgoError> {
        let size = self.entry(tab)?.context.size();
        if !x.is_finite() || !y.is_finite() || x < 0.0 || y < 0.0 {
            return Err(EgoError::new(
                ErrorCode::InvalidArgument,
                "pointer coordinates must be finite and non-negative",
            ));
        }
        if x >= size.width as f32 || y >= size.height as f32 {
            return Err(EgoError::new(
                ErrorCode::InvalidArgument,
                "pointer coordinates are outside the webview",
            ));
        }
        let point = DevicePoint::new(x, y);
        self.input(
            tab,
            InputEvent::MouseMove(MouseMoveEvent::new(point.into())),
        )?;
        self.input(
            tab,
            InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Down,
                MouseButton::Primary,
                point.into(),
            )),
        )?;
        self.input(
            tab,
            InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Up,
                MouseButton::Primary,
                point.into(),
            )),
        )
    }

    /// Enregistrer le preload agent sur le `UserContentManager` partagé. À appeler **avant**
    /// le premier chargement : le moteur n'applique une mise à jour du gestionnaire qu'au
    /// rechargement de la page (`user_content_manager.rs:16`).
    ///
    /// Ce que couvre un `UserScript` est mesuré et borné (D9) : le frame principal et les
    /// iframes cross-origin, pas les iframes même origine ni `srcdoc`. Le parcours
    /// `contentWindow` depuis le top, fait par le walker lui-même, complète la couverture.
    pub fn install_preload(&mut self, script: &str) {
        self.user_content_manager
            .add_script(Rc::new(UserScript::new(script.to_owned(), None)));
    }

    /// Évaluer dans le frame top de l'onglet et attendre le résultat en pompant. L'appelant
    /// est déjà le thread moteur : c'est ici qu'un `ego.evaluate` du pont devient synchrone.
    pub fn evaluate(&mut self, tab: TabId, script: &str) -> Result<Value, EgoError> {
        let webview = self.webview(tab)?;
        let slot: Rc<RefCell<Option<Result<JSValue, JavaScriptEvaluationError>>>> =
            Rc::new(RefCell::new(None));
        let pending = slot.clone();
        webview.evaluate_javascript(script, move |result| {
            *pending.borrow_mut() = Some(result);
        });

        let started = Instant::now();
        loop {
            if slot.borrow().is_some() {
                break;
            }
            if !self.tabs.contains_key(&tab) {
                return Err(EgoError::new(
                    ErrorCode::WebContentsUnavailable,
                    format!("tab {} closed while evaluating", tab.0),
                ));
            }
            if started.elapsed() >= EVALUATE_TIMEOUT {
                return Err(EgoError::new(
                    ErrorCode::OperationFailed,
                    format!("evaluation timed out after {EVALUATE_TIMEOUT:?}"),
                ));
            }
            self.pump();
            std::thread::sleep(Duration::from_millis(2));
        }

        let outcome = slot.borrow_mut().take().expect("checked above");
        let value = outcome.map_err(|error| {
            let code = match error {
                JavaScriptEvaluationError::DocumentNotFound
                | JavaScriptEvaluationError::WebViewNotReady => ErrorCode::WebContentsUnavailable,
                _ => ErrorCode::OperationFailed,
            };
            EgoError::new(code, format!("evaluation failed: {error:?}"))
        })?;
        Ok(json_of(value))
    }

    pub fn snapshot(
        &mut self,
        tab: TabId,
        options: &SnapshotOptions,
    ) -> Result<Snapshot, EgoError> {
        let raw = self.evaluate(tab, include_str!("../../bridge/assets/ego-preload.js"))?;
        parse_snapshot(&raw, options)
    }

    pub fn handle_rpc(&mut self, raw: &str) -> Option<String> {
        let request = match Request::parse(raw) {
            Ok(request) => request,
            Err(reply) => return Some(reply.to_json()),
        };
        let id = request.id.clone();
        let call = match request.call() {
            Ok(call) => call,
            Err(error) => {
                return id.map(|id| Reply::error(Some(id), error).to_json());
            },
        };
        let result = self.dispatch(call);
        id.map(|id| match result {
            Ok(result) => Reply::ok(id, result).to_json(),
            Err(error) => Reply::error(Some(id), ErrorObject::ego(&error)).to_json(),
        })
    }

    fn dispatch(&mut self, call: Call) -> Result<Value, EgoError> {
        match call {
            Call::GetBrowserVersion => Ok(json!({
                "name": "Servo",
                "version": env!("CARGO_PKG_VERSION"),
            })),
            Call::ListProfiles => {
                let mut profiles: Vec<String> = self
                    .registry
                    .spaces()
                    .map(|space| space.profile.0.clone())
                    .collect();
                profiles.sort();
                profiles.dedup();
                Ok(json!(profiles))
            },
            Call::ListTaskSpaces => {
                json_of_serializable(self.registry.spaces().collect::<Vec<_>>())
            },
            Call::CreateTaskSpace { name } => {
                let id = self.create_space(&name, ProfileId("default".into()));
                json_of_serializable(self.registry.space(id)?)
            },
            Call::ClaimTaskSpace { space } | Call::TakeOverTaskSpace { space } => {
                self.registry.claim(space)?;
                Ok(json!({ "space": space }))
            },
            Call::HandOffTaskSpace { space } => {
                self.registry.hand_off(space)?;
                Ok(json!({ "space": space }))
            },
            Call::CompleteTaskSpace { space } => {
                self.registry.complete(space)?;
                Ok(json!({ "space": space }))
            },
            Call::CloseTaskSpace { space } => {
                let tabs = self.registry.close(space)?;
                for tab in tabs {
                    self.tabs.remove(&tab);
                    self.push(EngineEvent::Closed { tab });
                }
                Ok(json!({ "space": space }))
            },
            Call::ListTabs { space } => {
                let space = self.registry.space(space)?;
                let tabs = space
                    .tabs
                    .iter()
                    .map(|tab| self.registry.tab(*tab))
                    .collect::<Result<Vec<_>, _>>()?;
                json_of_serializable(tabs)
            },
            Call::CreateTab { space, url } => {
                let tab = self.create_tab(space, url.as_deref())?;
                json_of_serializable(self.registry.tab(tab)?)
            },
            Call::CloseTab { tab } => {
                self.close_tab(tab)?;
                Ok(json!({ "tab": tab }))
            },
            Call::Navigate { tab, url } => {
                self.require_agent_control_for_tab(tab)?;
                self.navigate(tab, &url)?;
                Ok(json!({ "tab": tab, "url": url }))
            },
            Call::GoBack { tab } => {
                self.require_agent_control_for_tab(tab)?;
                self.go_back(tab)?;
                Ok(json!({ "tab": tab }))
            },
            Call::GoForward { tab } => {
                self.require_agent_control_for_tab(tab)?;
                self.go_forward(tab)?;
                Ok(json!({ "tab": tab }))
            },
            Call::Reload { tab } => {
                self.require_agent_control_for_tab(tab)?;
                self.reload(tab)?;
                Ok(json!({ "tab": tab }))
            },
            Call::ClickAt { tab, x, y } => {
                self.require_agent_control_for_tab(tab)?;
                self.click_at(tab, x as f32, y as f32)?;
                Ok(json!({ "tab": tab }))
            },
            Call::Snapshot { tab, options } => {
                let snapshot = self.snapshot(tab, &options)?;
                json_of_serializable(snapshot)
            },
            Call::Evaluate { tab, expression } => {
                self.require_agent_control_for_tab(tab)?;
                self.evaluate(tab, &expression)
            },
            Call::Click { tab, reference } => {
                self.click(tab, &reference)?;
                Ok(json!({ "tab": tab, "reference": reference }))
            },
            Call::Fill {
                tab,
                reference,
                value,
            } => {
                self.fill(tab, &reference, &value)?;
                Ok(json!({ "tab": tab, "reference": reference }))
            },
            Call::TypeText { tab, text } => {
                self.require_agent_control_for_tab(tab)?;
                for character in text.chars() {
                    let key = Key::Character(character.to_string());
                    self.input(
                        tab,
                        InputEvent::Keyboard(KeyboardEvent::from_state_and_key(
                            KeyState::Down,
                            key.clone(),
                        )),
                    )?;
                    self.input(
                        tab,
                        InputEvent::Keyboard(KeyboardEvent::from_state_and_key(KeyState::Up, key)),
                    )?;
                }
                Ok(json!({ "tab": tab }))
            },
            Call::Scroll {
                tab,
                delta_x,
                delta_y,
            } => {
                self.require_agent_control_for_tab(tab)?;
                self.input(
                    tab,
                    InputEvent::Wheel(WheelEvent::new(
                        WheelDelta {
                            x: delta_x,
                            y: delta_y,
                            z: 0.0,
                            mode: WheelMode::DeltaPixel,
                        },
                        DevicePoint::default().into(),
                    )),
                )?;
                Ok(json!({ "tab": tab }))
            },
            Call::Capture { tab } => {
                self.pump();
                let entry = self.entry(tab)?;
                let size = entry.context.size();
                let image = entry
                    .context
                    .read_to_image(frame_rect((size.width, size.height)))
                    .ok_or_else(|| {
                        EgoError::new(ErrorCode::OperationFailed, "capture returned no image")
                    })?;
                let mut png = Cursor::new(Vec::new());
                DynamicImage::ImageRgba8(image)
                    .write_to(&mut png, ImageFormat::Png)
                    .map_err(|error| {
                        EgoError::new(
                            ErrorCode::OperationFailed,
                            format!("PNG capture failed: {error}"),
                        )
                    })?;
                let metadata = self.registry.tab(tab)?;
                Ok(json!({
                    "data": STANDARD.encode(png.into_inner()),
                    "title": metadata.title,
                    "url": metadata.url,
                }))
            },
            Call::SendCdpMessage { .. } => Err(EgoError::new(
                ErrorCode::CdpChannelUnavailable,
                "the DevTools protocol channel is not configured",
            )),
        }
    }

    pub fn click(&mut self, tab: TabId, reference: &str) -> Result<(), EgoError> {
        self.require_agent_control_for_tab(tab)?;
        let script = reference_script(reference, "element.click();")?;
        action_succeeded(self.evaluate(tab, &script)?, reference)
    }

    pub fn fill(&mut self, tab: TabId, reference: &str, value: &str) -> Result<(), EgoError> {
        self.require_agent_control_for_tab(tab)?;
        let value = serde_json::to_string(value).expect("strings always serialize");
        let script = reference_script(
            reference,
            &format!(
                "const value={value};let proto=Object.getPrototypeOf(element);let setter;while(proto&&!setter){{setter=Object.getOwnPropertyDescriptor(proto,'value')?.set;proto=Object.getPrototypeOf(proto);}}if(setter)setter.call(element,value);else element.value=value;element.dispatchEvent(new Event('input',{{bubbles:true}}));element.dispatchEvent(new Event('change',{{bubbles:true}}));"
            ),
        )?;
        action_succeeded(self.evaluate(tab, &script)?, reference)
    }

    fn require_agent_control_for_tab(&self, tab: TabId) -> Result<(), EgoError> {
        let space = self.registry.tab(tab)?.space;
        self.registry.require_agent_control(space)?;
        Ok(())
    }

    /// Redimensionner un onglet. `0` sur un axe est ignoré plutôt que propagé : un contexte
    /// logiciel de surface nulle alloue et rend sans rien montrer (point 5 du Review Focus).
    pub fn resize(&mut self, tab: TabId, width: u32, height: u32) -> Result<(), EgoError> {
        if width == 0 || height == 0 {
            return Ok(());
        }
        let size = PhysicalSize::new(width, height);
        let entry = self.entry(tab)?;
        entry.context.resize(size);
        entry.webview.resize(size);
        Ok(())
    }

    pub fn close_tab(&mut self, tab: TabId) -> Result<(), EgoError> {
        self.registry.close_tab(tab)?;
        self.tabs.remove(&tab);
        self.push(EngineEvent::Closed { tab });
        Ok(())
    }

    pub fn set_active_tab(&mut self, tab: TabId) -> Result<(), EgoError> {
        self.registry.set_active_tab(tab)
    }

    /// Faire avancer le moteur, puis rendre les images que le moteur annonce comme sales.
    /// C'est l'horloge de l'affichage : qui appelle `pump` décide de la cadence.
    pub fn pump(&mut self) {
        self.servo.spin_event_loop();
        if self.waked() {
            self.wakeup.store(false, Ordering::Relaxed);
        }

        let queue = self.events.clone();
        let mut painted: Vec<(TabId, Option<RgbaImage>)> = Vec::new();
        for (tab, entry) in self.tabs.iter_mut() {
            if !entry.dirty.replace(false) {
                continue;
            }
            entry.webview.paint();
            let size = entry.context.size();
            let image = entry
                .context
                .read_to_image(frame_rect((size.width, size.height)));
            painted.push((*tab, image));
        }
        for (tab, image) in painted {
            if let Some(image) = image {
                queue.borrow_mut().push_back(EngineEvent::Frame {
                    tab,
                    image: Arc::new(image),
                });
            }
        }

        self.sync_registry();
    }

    /// Les événements sortent quand l'appelant les veut, mais le modèle, lui, doit être à jour
    /// à la fin de chaque `pump` : c'est là que l'URL, le titre et l'état de chargement sont
    /// recopiés dans le `Registry`.
    fn sync_registry(&mut self) {
        for event in self.events.borrow().iter() {
            match event {
                EngineEvent::UrlChanged { tab, url } => {
                    if let Ok(entry) = self.registry.tab_mut(*tab) {
                        entry.url = Some(url.clone());
                        entry.loading = true;
                    }
                },
                EngineEvent::TitleChanged { tab, title } => {
                    if let Ok(entry) = self.registry.tab_mut(*tab) {
                        entry.title = Some(title.clone());
                    }
                },
                EngineEvent::LoadStatusChanged { tab, complete } => {
                    if let Ok(entry) = self.registry.tab_mut(*tab) {
                        entry.loading = !complete;
                    }
                },
                EngineEvent::Frame { .. }
                | EngineEvent::Closed { .. }
                | EngineEvent::Crashed { .. } => {},
            }
        }
    }

    pub fn take_events(&mut self) -> Vec<EngineEvent> {
        self.events.borrow_mut().drain(..).collect()
    }

    fn webview(&self, tab: TabId) -> Result<WebView, EgoError> {
        Ok(self.entry(tab)?.webview.clone())
    }

    fn entry(&self, tab: TabId) -> Result<&Tab, EgoError> {
        self.registry.tab(tab)?;
        self.tabs.get(&tab).ok_or_else(|| {
            EgoError::new(
                ErrorCode::WebContentsUnavailable,
                format!("no engine tab {}", tab.0),
            )
        })
    }

    fn push(&self, event: EngineEvent) {
        self.events.borrow_mut().push_back(event);
    }
}

/// Le `JSValue` du moteur en JSON. Les quatre variantes de poignées de DOM (`Element`,
/// `ShadowRoot`, `Frame`, `Window`) n'ont pas d'équivalent JSON : elles deviennent leur
/// identifiant, seul truc exploitables par un agent qui ne tient pas le navigateur.
fn json_of(value: JSValue) -> Value {
    match value {
        JSValue::Undefined | JSValue::Null => Value::Null,
        JSValue::Boolean(flag) => Value::Bool(flag),
        JSValue::Number(number) => {
            serde_json::Number::from_f64(number).map_or(Value::Null, Value::Number)
        },
        JSValue::String(text) => Value::String(text),
        JSValue::Element(id)
        | JSValue::ShadowRoot(id)
        | JSValue::Frame(id)
        | JSValue::Window(id) => Value::String(id),
        JSValue::Array(items) => Value::Array(items.into_iter().map(json_of).collect()),
        JSValue::Object(entries) => {
            let mut map = Map::with_capacity(entries.len());
            for (key, value) in entries {
                map.insert(key, json_of(value));
            }
            Value::Object(map)
        },
    }
}

fn json_of_serializable(value: impl serde::Serialize) -> Result<Value, EgoError> {
    serde_json::to_value(value).map_err(|error| {
        EgoError::new(
            ErrorCode::OperationFailed,
            format!("could not serialize browser result: {error}"),
        )
    })
}

fn reference_script(reference: &str, action: &str) -> Result<String, EgoError> {
    let (frame_path, element_ref) = reference.split_once(':').ok_or_else(|| {
        EgoError::new(
            ErrorCode::InvalidArgument,
            "reference must be `frame:element`",
        )
    })?;
    let mut segments = frame_path.split('.');
    if segments.next() != Some("f0") {
        return Err(EgoError::new(
            ErrorCode::InvalidArgument,
            format!("invalid frame path `{frame_path}`"),
        ));
    }
    let frame_indices = segments
        .map(|segment| {
            segment.parse::<usize>().map_err(|_| {
                EgoError::new(
                    ErrorCode::InvalidArgument,
                    format!("invalid frame index `{segment}`"),
                )
            })
        })
        .collect::<Result<Vec<_>, _>>()?;
    if !element_ref.starts_with('e') || element_ref[1..].parse::<usize>().is_err() {
        return Err(EgoError::new(
            ErrorCode::InvalidArgument,
            format!("invalid element reference `{element_ref}`"),
        ));
    }

    let frame_indices = serde_json::to_string(&frame_indices).expect("indices serialize");
    let element_ref = serde_json::to_string(element_ref).expect("references serialize");
    Ok(format!(
        "(()=>{{let win=window;for(const index of {frame_indices}){{if(!win.frames[index])return false;win=win.frames[index];}}const element=Array.from(win.document.querySelectorAll('[data-ego-ref]')).find(node=>node.getAttribute('data-ego-ref')==={element_ref});if(!element)return false;{action}return true;}})()"
    ))
}

fn action_succeeded(result: Value, reference: &str) -> Result<(), EgoError> {
    if result == Value::Bool(true) {
        Ok(())
    } else {
        Err(EgoError::new(
            ErrorCode::WebContentsUnavailable,
            format!("reference `{reference}` is no longer available"),
        ))
    }
}

fn parse_url(url: &str) -> Result<Url, EgoError> {
    Url::parse(url).map_err(|error| {
        EgoError::new(
            ErrorCode::InvalidArgument,
            format!("`{url}` is not a url: {error}"),
        )
    })
}

fn rendering_context(size: (u32, u32)) -> Result<Rc<dyn RenderingContext>, EgoError> {
    let context = Rc::new(
        SoftwareRenderingContext::new(PhysicalSize::new(size.0, size.1)).map_err(|error| {
            EgoError::new(
                ErrorCode::OperationFailed,
                format!("no software rendering context: {error:?}"),
            )
        })?,
    );
    context.make_current().map_err(|error| {
        EgoError::new(
            ErrorCode::OperationFailed,
            format!("software context not bindable: {error:?}",),
        )
    })?;
    Ok(context)
}

fn frame_rect(size: (u32, u32)) -> DeviceIntRect {
    DeviceIntRect::new(
        Point2D::origin().cast_unit(),
        Point2D::new(size.0 as i32, size.1 as i32).cast_unit(),
    )
}

#[derive(Clone)]
struct Waker(Arc<AtomicBool>);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.0.store(true, Ordering::Relaxed);
    }
}
