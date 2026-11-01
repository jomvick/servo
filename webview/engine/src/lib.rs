//! L'adaptateur Servo : un `Servo`, un contexte de rendu logiciel par onglet, et des frames
//! qui ne sortent que quand l'appelant le demande.
//!
//! Deux décisions de D4 et D5 structurent ce fichier : tout est `Rc` et donc sur un seul
//! thread (le moteur n'est pas `Send`), et la peinture se paie au `pump` de l'appelant, pas
//! dans la notification du moteur — sinon la cadence d'affichage serait décidée par Servo.

mod delegate;

use std::cell::{Cell, RefCell};
use std::collections::{HashMap, VecDeque};
use std::fs::{File, OpenOptions};
use std::path::PathBuf;
use std::rc::Rc;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::thread::{self, Thread};
use std::time::{Duration, Instant};

use dpi::PhysicalSize;
use embedder_traits::EventLoopWaker;
use euclid::default::Point2D;
use fs2::FileExt;
use image::RgbaImage;
use paint_api::rendering_context::{RenderingContext, SoftwareRenderingContext};
use serde_json::{Map, Value};
use servo::{
    Code, DeviceIntRect, DevicePoint, InputEvent, JSValue, JavaScriptEvaluationError, Key,
    KeyState, KeyboardEvent, MouseButton, MouseButtonAction, MouseButtonEvent, Opts, Servo,
    ServoBuilder, UserContentManager, UserScript, WebView, WebViewBuilder, WheelDelta, WheelEvent,
    WheelMode,
};
use url::Url;
use webview_core::{EgoError, ErrorCode, ProfileId, Registry, SpaceId, TabId};

use crate::delegate::TabDelegate;

/// Délai d'une évaluation JavaScript. Large parce qu'un script d'agent peut légitimement
/// attendre quelque chose ; court parce qu'un frame pendu doit rester une erreur, pas un gel.
const EVALUATE_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Clone, Debug)]
pub struct EngineConfig {
    /// Taille d'un onglet neuf. Un `resize(0, 0)` ne l'écrase pas.
    pub viewport: (u32, u32),
    /// Dossier de profil Servo. `None` conserve le comportement temporaire de Servo.
    pub data_dir: Option<PathBuf>,
}

impl Default for EngineConfig {
    fn default() -> Self {
        Self {
            viewport: (1280, 800),
            data_dir: None,
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
    pending_events: VecDeque<EngineEvent>,
    user_content_manager: Rc<UserContentManager>,
    wakeup: Arc<AtomicBool>,
    _profile_lock: Option<File>,
    viewport: (u32, u32),
}

impl Engine {
    pub fn new(config: EngineConfig) -> Result<Self, EgoError> {
        if rustls::crypto::CryptoProvider::get_default().is_none() {
            let _ = rustls::crypto::aws_lc_rs::default_provider().install_default();
        }
        let wakeup = Arc::new(AtomicBool::new(false));
        let profile_lock = if let Some(data_dir) = config.data_dir.as_ref() {
            std::fs::create_dir_all(data_dir).map_err(|error| {
                EgoError::new(
                    ErrorCode::OperationFailed,
                    format!(
                        "could not create profile directory {}: {error}",
                        data_dir.display()
                    ),
                )
            })?;
            let lock_path = data_dir.join(".webview-engine.lock");
            let lock = OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(&lock_path)
                .map_err(|error| {
                    EgoError::new(
                        ErrorCode::OperationFailed,
                        format!(
                            "could not open profile lock {}: {error}",
                            lock_path.display()
                        ),
                    )
                })?;
            FileExt::try_lock_exclusive(&lock).map_err(|error| {
                EgoError::new(
                    ErrorCode::OperationFailed,
                    format!(
                        "profile is already in use at {}: {error}",
                        data_dir.display()
                    ),
                )
            })?;
            Some(lock)
        } else {
            None
        };

        let mut builder = ServoBuilder::default();
        if let Some(data_dir) = config.data_dir {
            builder = builder.opts(Opts {
                config_dir: Some(data_dir),
                ..Opts::default()
            });
        }
        let servo = builder
            .event_loop_waker(Box::new(Waker(wakeup.clone(), thread::current())))
            .build();
        Ok(Self {
            user_content_manager: Rc::new(UserContentManager::new(&servo)),
            servo,
            registry: Registry::default(),
            tabs: HashMap::new(),
            events: Rc::new(RefCell::new(VecDeque::new())),
            pending_events: VecDeque::new(),
            wakeup,
            _profile_lock: profile_lock,
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
        self.create_tab_for(space, url, false)
    }

    pub fn create_user_tab(
        &mut self,
        space: SpaceId,
        url: Option<&str>,
    ) -> Result<TabId, EgoError> {
        self.create_tab_for(space, url, true)
    }

    fn create_tab_for(
        &mut self,
        space: SpaceId,
        url: Option<&str>,
        user: bool,
    ) -> Result<TabId, EgoError> {
        let parsed = url.map(parse_url).transpose()?;
        let tab = if user {
            self.registry
                .create_user_tab(space, url.map(str::to_owned))?
        } else {
            self.registry.create_tab(space, url.map(str::to_owned))?
        };

        let (width, height) = self.viewport;
        let context = match rendering_context((width, height)) {
            Ok(context) => context,
            Err(error) => {
                let _ = self.registry.close_tab(tab);
                return Err(error);
            },
        };
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

    pub fn pointer_click(&mut self, tab: TabId, x: f32, y: f32) -> Result<(), EgoError> {
        let point = DevicePoint::new(x, y).into();
        self.input(
            tab,
            InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Down,
                MouseButton::Primary,
                point,
            )),
        )?;
        self.input(
            tab,
            InputEvent::MouseButton(MouseButtonEvent::new(
                MouseButtonAction::Up,
                MouseButton::Primary,
                point,
            )),
        )
    }

    pub fn scroll(
        &mut self,
        tab: TabId,
        x: f32,
        y: f32,
        delta_x: f32,
        delta_y: f32,
    ) -> Result<(), EgoError> {
        self.input(
            tab,
            InputEvent::Wheel(WheelEvent::new(
                WheelDelta {
                    x: -delta_x as f64,
                    y: -delta_y as f64,
                    z: 0.0,
                    mode: WheelMode::DeltaPixel,
                },
                DevicePoint::new(x, y).into(),
            )),
        )
    }

    pub fn text_input(&mut self, tab: TabId, text: &str) -> Result<(), EgoError> {
        for character in text.chars() {
            self.dispatch_key(tab, Key::Character(character.to_string()))?;
        }
        Ok(())
    }

    pub fn key_input(&mut self, tab: TabId, key: &str) -> Result<(), EgoError> {
        let key = key
            .parse::<Key>()
            .unwrap_or_else(|_| Key::Character(key.to_owned()));
        self.dispatch_key(tab, key)
    }

    fn dispatch_key(&mut self, tab: TabId, key: Key) -> Result<(), EgoError> {
        for state in [KeyState::Down, KeyState::Up] {
            self.input(
                tab,
                InputEvent::Keyboard(KeyboardEvent::new_without_event(
                    state,
                    key.clone(),
                    Code::Unidentified,
                    servo::Location::Standard,
                    servo::Modifiers::empty(),
                    false,
                    false,
                )),
            )?;
        }
        Ok(())
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
        self.wakeup.swap(false, Ordering::AcqRel);
        self.servo.spin_event_loop();

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
        let events: Vec<_> = self.events.borrow_mut().drain(..).collect();
        for event in &events {
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
                EngineEvent::Frame { .. } => {},
                EngineEvent::Closed { tab } | EngineEvent::Crashed { tab, .. } => {
                    if self.registry.tab(*tab).is_ok() {
                        let _ = self.registry.close_tab(*tab);
                    }
                    self.tabs.remove(tab);
                },
            }
        }
        for event in events {
            if let EngineEvent::Frame { tab, .. } = &event {
                self.pending_events.retain(
                    |old| !matches!(old, EngineEvent::Frame { tab: old_tab, .. } if old_tab == tab),
                );
            }
            self.pending_events.push_back(event);
        }
    }

    pub fn take_events(&mut self) -> Vec<EngineEvent> {
        self.sync_registry();
        self.pending_events.drain(..).collect()
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
struct Waker(Arc<AtomicBool>, Thread);

impl EventLoopWaker for Waker {
    fn clone_box(&self) -> Box<dyn EventLoopWaker> {
        Box::new(self.clone())
    }

    fn wake(&self) {
        self.0.store(true, Ordering::Release);
        self.1.unpark();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_apply_once_frames_coalesce_and_profile_stays_locked() {
        let profile =
            std::env::temp_dir().join(format!("servo-engine-unit-{}", std::process::id()));
        let config = EngineConfig {
            data_dir: Some(profile.clone()),
            ..EngineConfig::default()
        };
        let mut engine = Engine::new(config.clone()).unwrap();
        assert!(
            Engine::new(config).is_err(),
            "a second engine must not share a live profile"
        );
        let space = engine.create_space("events", ProfileId("default".into()));
        let tab = engine.registry.create_tab(space, None).unwrap();
        engine.push(EngineEvent::TitleChanged {
            tab,
            title: "first".into(),
        });
        engine.sync_registry();
        engine.registry.tab_mut(tab).unwrap().title = Some("newer".into());
        engine.sync_registry();
        assert_eq!(
            engine.registry.tab(tab).unwrap().title.as_deref(),
            Some("newer")
        );
        for value in [10, 20] {
            engine.push(EngineEvent::Frame {
                tab,
                image: Arc::new(RgbaImage::from_pixel(1, 1, image::Rgba([value, 0, 0, 255]))),
            });
            engine.sync_registry();
        }
        let events = engine.take_events();
        let frames: Vec<_> = events
            .iter()
            .filter_map(|event| match event {
                EngineEvent::Frame { image, .. } => Some(image),
                _ => None,
            })
            .collect();
        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].get_pixel(0, 0).0[0], 20);
        engine.push(EngineEvent::Crashed {
            tab,
            reason: "fixture".into(),
        });
        engine.sync_registry();
        assert!(engine.registry.tab(tab).is_err());
        drop(engine);
        let lock = OpenOptions::new()
            .read(true)
            .write(true)
            .open(profile.join(".webview-engine.lock"))
            .unwrap();
        FileExt::try_lock_exclusive(&lock).unwrap();
        drop(lock);
        std::fs::remove_dir_all(profile).unwrap();
    }
}
