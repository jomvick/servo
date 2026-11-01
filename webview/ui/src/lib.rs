use std::sync::{
    Arc,
    mpsc::{self, Receiver, Sender, TryRecvError},
};
use std::thread;
use std::time::Duration;

use anyhow::Context as _;
use gpui::{
    App, Bounds, Context, FocusHandle, Focusable, ImageSource, IntoElement, MouseButton, Render,
    RenderImage, ScrollDelta, SharedString, Window, WindowBounds, WindowOptions, div, img,
    prelude::*, px, rgb, size,
};
use gpui_platform::application;
use image::{Frame, RgbaImage};
use serde::Deserialize;
use serde_json::{Value, json};
use webview_runtime::BrowserClient;

const INITIAL_SPACE: u64 = 1;
const INITIAL_TAB: u64 = 1;
const VIEWPORT_WIDTH: u32 = 1280;
const VIEWPORT_HEIGHT: u32 = 720;
const TOOLBAR_HEIGHT: f32 = 116.0;

/// Starts a standalone GPUI browser window backed by a Servo runtime client.
pub fn run(client: BrowserClient) -> anyhow::Result<()> {
    application().run(move |cx: &mut App| {
        let view_client = client.clone();
        let shell = cx.new(|cx| BrowserView::new(view_client, cx).expect("start browser view"));
        let bounds = Bounds::centered(None, size(px(1280.0), px(840.0)), cx);
        cx.open_window(
            WindowOptions {
                window_bounds: Some(WindowBounds::Windowed(bounds)),
                ..Default::default()
            },
            move |_, _| shell,
        )
        .expect("open browser window");
        cx.on_window_closed(|cx, _| {
            if cx.windows().is_empty() {
                cx.quit();
            }
        })
        .detach();
        cx.activate(true);
    });

    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct Space {
    id: u64,
    name: String,
    ownership: String,
    active: Option<u64>,
}

#[derive(Clone, Debug, Deserialize)]
struct Tab {
    id: u64,
    space: u64,
    url: Option<String>,
    title: Option<String>,
    loading: bool,
}

#[derive(Clone)]
struct Snapshot {
    spaces: Vec<Space>,
    tabs: Vec<Tab>,
    active_tab: u64,
    error: Option<String>,
}

enum UiCommand {
    SelectSpace {
        space: u64,
        tab: u64,
    },
    SelectTab(u64),
    CloseTab(u64),
    CreateTab {
        space: u64,
        url: String,
    },
    Navigate {
        tab: u64,
        url: String,
    },
    Reload(u64),
    Back(u64),
    Forward(u64),
    Click {
        tab: u64,
        x: f32,
        y: f32,
    },
    Scroll {
        tab: u64,
        x: f32,
        y: f32,
        dx: f32,
        dy: f32,
    },
    Type {
        tab: u64,
        text: String,
    },
    Key {
        tab: u64,
        key: String,
    },
}

fn browser_worker(
    client: BrowserClient,
    commands: Receiver<UiCommand>,
    snapshots: mpsc::SyncSender<Snapshot>,
) {
    let mut selected_space = INITIAL_SPACE;
    let mut selected_tab = INITIAL_TAB;
    let mut last_error = None;

    let _ = client.request(
        "ui.resize",
        json!({ "tab": selected_tab, "width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT }),
    );

    loop {
        match commands.recv_timeout(Duration::from_millis(100)) {
            Ok(command) => match command {
                UiCommand::SelectSpace { space, tab } => {
                    selected_space = space;
                    selected_tab = tab;
                    last_error = resize_tab(&client, tab);
                },
                UiCommand::SelectTab(tab) => {
                    selected_tab = tab;
                    last_error = resize_tab(&client, tab);
                },
                UiCommand::CloseTab(tab) => {
                    last_error = request_error(&client, "ui.closeTab", json!({ "tab": tab }));
                    if last_error.is_none() && selected_tab == tab {
                        selected_tab = 0;
                    }
                },
                UiCommand::CreateTab { space, url } => {
                    match client.request("ui.createTab", json!({ "space": space, "url": url })) {
                        Ok(value) => match serde_json::from_value::<Tab>(value) {
                            Ok(tab) => {
                                selected_space = space;
                                selected_tab = tab.id;
                                last_error = resize_tab(&client, tab.id);
                            },
                            Err(error) => {
                                last_error = Some(format!("invalid new tab response: {error}"))
                            },
                        },
                        Err(error) => last_error = Some(error),
                    }
                },
                UiCommand::Navigate { tab, url } => {
                    last_error =
                        request_error(&client, "ui.navigate", json!({ "tab": tab, "url": url }));
                },
                UiCommand::Reload(tab) => {
                    last_error = request_error(&client, "ui.reload", json!({ "tab": tab }));
                },
                UiCommand::Back(tab) => {
                    last_error = request_error(&client, "ui.back", json!({ "tab": tab }));
                },
                UiCommand::Forward(tab) => {
                    last_error = request_error(&client, "ui.forward", json!({ "tab": tab }));
                },
                UiCommand::Click { tab, x, y } => {
                    last_error =
                        request_error(&client, "ui.click", json!({ "tab": tab, "x": x, "y": y }));
                },
                UiCommand::Scroll { tab, x, y, dx, dy } => {
                    last_error = request_error(
                        &client,
                        "ui.scroll",
                        json!({ "tab": tab, "x": x, "y": y, "deltaX": dx, "deltaY": dy }),
                    );
                },
                UiCommand::Type { tab, text } => {
                    last_error =
                        request_error(&client, "ui.type", json!({ "tab": tab, "text": text }));
                },
                UiCommand::Key { tab, key } => {
                    last_error =
                        request_error(&client, "ui.key", json!({ "tab": tab, "key": key }));
                },
            },
            Err(mpsc::RecvTimeoutError::Timeout) => {},
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }

        let spaces = client
            .request("ego.listTaskSpaces", json!({}))
            .and_then(parse_array::<Space>);
        let spaces = match spaces {
            Ok(spaces) => spaces,
            Err(error) => {
                last_error = Some(error);
                Vec::new()
            },
        };
        if !spaces.iter().any(|space| space.id == selected_space) {
            selected_space = spaces.first().map_or(INITIAL_SPACE, |space| space.id);
        }
        if let Some(active) = spaces
            .iter()
            .find(|space| space.id == selected_space)
            .and_then(|space| space.active)
        {
            if selected_tab == INITIAL_TAB || !spaces.iter().any(|space| space.id == selected_space)
            {
                selected_tab = active;
            }
        }

        let tabs = client
            .request("ego.listTabs", json!({ "space": selected_space }))
            .and_then(parse_array::<Tab>);
        let tabs = match tabs {
            Ok(tabs) => tabs,
            Err(error) => {
                last_error = Some(error);
                Vec::new()
            },
        };
        if selected_tab == 0 {
            if let Some(active) = spaces
                .iter()
                .find(|space| space.id == selected_space)
                .and_then(|space| space.active)
                .filter(|id| tabs.iter().any(|tab| tab.id == *id))
            {
                selected_tab = active;
                last_error = resize_tab(&client, active);
            }
        }
        if !tabs.iter().any(|tab| tab.id == selected_tab) {
            if let Some(tab) = tabs.first() {
                selected_tab = tab.id;
            }
        }
        match snapshots.try_send(Snapshot {
            spaces,
            tabs,
            active_tab: selected_tab,
            error: last_error.clone(),
        }) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => {},
            Err(mpsc::TrySendError::Disconnected(_)) => break,
        }
    }
}

fn request_error(client: &BrowserClient, method: &str, params: Value) -> Option<String> {
    client.request(method, params).err()
}

fn resize_tab(client: &BrowserClient, tab: u64) -> Option<String> {
    if let Some(error) = request_error(client, "ui.activateTab", json!({ "tab": tab })) {
        return Some(error);
    }
    request_error(
        client,
        "ui.resize",
        json!({ "tab": tab, "width": VIEWPORT_WIDTH, "height": VIEWPORT_HEIGHT }),
    )
}

fn parse_array<T: for<'de> Deserialize<'de>>(value: Value) -> Result<Vec<T>, String> {
    serde_json::from_value(value).map_err(|error| error.to_string())
}

pub struct BrowserView {
    client: BrowserClient,
    commands: Sender<UiCommand>,
    spaces: Vec<Space>,
    tabs: Vec<Tab>,
    active_space: u64,
    active_tab: u64,
    frame: Option<Arc<RenderImage>>,
    last_rgba: Option<Arc<RgbaImage>>,
    url_text: String,
    editing_url: bool,
    error: Option<String>,
    focus_handle: FocusHandle,
}

impl BrowserView {
    /// Creates a reusable browser panel backed by a runtime client.
    pub fn new(client: BrowserClient, cx: &mut Context<Self>) -> anyhow::Result<Self> {
        let (command_tx, command_rx) = mpsc::channel();
        let (snapshot_tx, snapshot_rx) = mpsc::sync_channel(1);
        thread::Builder::new()
            .name("webview-ui-rpc".into())
            .spawn({
                let client = client.clone();
                move || browser_worker(client, command_rx, snapshot_tx)
            })
            .context("starting browser UI RPC worker")?;

        let view = Self {
            client: client.clone(),
            commands: command_tx,
            spaces: Vec::new(),
            tabs: Vec::new(),
            active_space: INITIAL_SPACE,
            active_tab: INITIAL_TAB,
            frame: None,
            last_rgba: None,
            url_text: "about:blank".into(),
            editing_url: false,
            error: None,
            focus_handle: cx.focus_handle(),
        };

        cx.spawn(async move |this, cx| {
            loop {
                cx.background_executor()
                    .timer(Duration::from_millis(16))
                    .await;
                let mut latest = None;
                loop {
                    match snapshot_rx.try_recv() {
                        Ok(snapshot) => latest = Some(snapshot),
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }
                if this
                    .update(cx, |this, cx| {
                        let mut changed = false;
                        if let Some(snapshot) = latest {
                            this.apply_snapshot(snapshot);
                            changed = true;
                        }
                        if this.refresh_frame() {
                            changed = true;
                        }
                        if changed {
                            cx.notify();
                        }
                    })
                    .is_err()
                {
                    return;
                }
            }
        })
        .detach();

        Ok(view)
    }

    fn apply_snapshot(&mut self, snapshot: Snapshot) {
        self.spaces = snapshot.spaces;
        self.tabs = snapshot.tabs;
        self.active_tab = snapshot.active_tab;
        self.error = snapshot.error;
        if !self
            .spaces
            .iter()
            .any(|space| space.id == self.active_space)
        {
            self.active_space = self.spaces.first().map_or(INITIAL_SPACE, |space| space.id);
        }
        if !self.editing_url {
            self.url_text = self
                .tabs
                .iter()
                .find(|tab| tab.id == self.active_tab)
                .and_then(|tab| tab.url.clone())
                .unwrap_or_default();
        }
    }

    fn refresh_frame(&mut self) -> bool {
        let Some(rgba) = self.client.latest_frame(self.active_tab) else {
            if self.last_rgba.is_some() {
                self.frame = None;
                self.last_rgba = None;
                return true;
            }
            return false;
        };

        if let Some(previous) = &self.last_rgba {
            if Arc::ptr_eq(previous, &rgba) {
                return false;
            }
        }
        self.frame = render_image(&rgba);
        self.last_rgba = Some(rgba);
        true
    }

    fn send(&self, command: UiCommand) {
        let _ = self.commands.send(command);
    }

    fn navigate(&mut self, cx: &mut Context<Self>) {
        let url = normalize_url(&self.url_text);
        self.url_text = url.clone();
        self.editing_url = false;
        self.send(UiCommand::Navigate {
            tab: self.active_tab,
            url,
        });
        cx.notify();
    }
}

impl Focusable for BrowserView {
    fn focus_handle(&self, _: &gpui::App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for BrowserView {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let active_tab = self.active_tab;
        let active_space = self.active_space;

        let spaces = self.spaces.iter().map(|space| {
            let id = space.id;
            let is_active = id == active_space;
            let ownership_color = match space.ownership.as_str() {
                "Agent" => rgb(0x9c81e8),
                "User" => rgb(0x5dc9a4),
                "Done" => rgb(0x9b9da5),
                _ => rgb(0xe3aa62),
            };
            div()
                .flex()
                .items_center()
                .gap_2()
                .h(px(38.0))
                .px_3()
                .when(is_active, |this| this.bg(rgb(0x34363d)))
                .hover(|this| this.bg(rgb(0x303238)))
                .cursor_pointer()
                .on_mouse_down(
                    MouseButton::Left,
                    cx.listener(move |this, _, _, cx| {
                        this.active_space = id;
                        this.active_tab = this
                            .spaces
                            .iter()
                            .find(|space| space.id == id)
                            .and_then(|space| space.active)
                            .unwrap_or(INITIAL_TAB);
                        this.send(UiCommand::SelectSpace {
                            space: id,
                            tab: this.active_tab,
                        });
                        cx.notify();
                    }),
                )
                .child(div().text_color(rgb(0xe4e5e9)).child(space.name.clone()))
                .child(
                    div()
                        .text_xs()
                        .text_color(ownership_color)
                        .child(space.ownership.clone()),
                )
        });

        let tabs = self.tabs.iter().map(|tab| {
            let id = tab.id;
            let is_active = id == active_tab;
            let title = tab
                .title
                .as_deref()
                .filter(|title| !title.is_empty())
                .or(tab.url.as_deref())
                .unwrap_or("New tab");
            let title = if tab.loading {
                format!("◌ {title}")
            } else {
                title.to_owned()
            };
            div()
                .h(px(36.0))
                .max_w(px(260.0))
                .min_w(px(90.0))
                .px_3()
                .flex()
                .items_center()
                .overflow_hidden()
                .when(is_active, |this| {
                    this.bg(rgb(0x27292f))
                        .border_1()
                        .border_color(rgb(0x9c81e8))
                })
                .when(!is_active, |this| this.bg(rgb(0x202227)))
                .hover(|this| this.bg(rgb(0x303238)))
                .cursor_pointer()
                .child(
                    div()
                        .id(SharedString::from(format!("tab-title-{id}")))
                        .flex_1()
                        .overflow_hidden()
                        .text_sm()
                        .text_color(rgb(0xd7d8dc))
                        .cursor_pointer()
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.active_tab = id;
                                this.editing_url = false;
                                if let Some(tab) = this.tabs.iter().find(|tab| tab.id == id) {
                                    this.active_space = tab.space;
                                }
                                this.send(UiCommand::SelectTab(id));
                                cx.notify();
                            }),
                        )
                        .child(title),
                )
                .child(
                    div()
                        .id(SharedString::from(format!("close-tab-{id}")))
                        .h(px(22.0))
                        .w(px(22.0))
                        .flex()
                        .items_center()
                        .justify_center()
                        .rounded_sm()
                        .text_xs()
                        .text_color(rgb(0xaeb0b8))
                        .hover(|this| this.bg(rgb(0x474951)).text_color(rgb(0xffffff)))
                        .on_mouse_down(
                            MouseButton::Left,
                            cx.listener(move |this, _, _, cx| {
                                this.send(UiCommand::CloseTab(id));
                                cx.notify();
                            }),
                        )
                        .child("×"),
                )
        });

        let frame = match &self.frame {
            Some(frame) => img(ImageSource::Render(frame.clone()))
                .size_full()
                .object_fit(gpui::ObjectFit::Contain)
                .into_any_element(),
            None => div()
                .size_full()
                .flex()
                .items_center()
                .justify_center()
                .text_color(rgb(0x858891))
                .child("Waiting for the first Servo frame…")
                .into_any_element(),
        };

        div()
            .size_full()
            .flex()
            .flex_col()
            .bg(rgb(0x17181c))
            .text_color(rgb(0xd7d8dc))
            .child(
                div()
                    .h(px(38.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .bg(rgb(0x202227))
                    .children(spaces)
                    .child(
                        div()
                            .ml_auto()
                            .px_3()
                            .text_sm()
                            .text_color(rgb(0xb6b8c0))
                            .child("Spaces · Agent / User / Done"),
                    ),
            )
            .child(
                div()
                    .h(px(36.0))
                    .flex()
                    .items_center()
                    .gap_1()
                    .px_2()
                    .bg(rgb(0x202227))
                    .children(tabs)
                    .child(
                        div()
                            .h(px(30.0))
                            .px_3()
                            .flex()
                            .items_center()
                            .text_color(rgb(0xc9cad0))
                            .hover(|this| this.bg(rgb(0x34363d)))
                            .cursor_pointer()
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, _, cx| {
                                    this.send(UiCommand::CreateTab {
                                        space: this.active_space,
                                        url: "about:blank".to_owned(),
                                    });
                                    cx.notify();
                                }),
                            )
                            .child("+"),
                    ),
            )
            .child(
                div()
                    .h(px(42.0))
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_3()
                    .bg(rgb(0x24262b))
                    .child(nav_button(
                        "←",
                        cx.listener(move |this, _, _, cx| {
                            this.send(UiCommand::Back(active_tab));
                            cx.notify();
                        }),
                    ))
                    .child(nav_button(
                        "→",
                        cx.listener(move |this, _, _, cx| {
                            this.send(UiCommand::Forward(active_tab));
                            cx.notify();
                        }),
                    ))
                    .child(nav_button(
                        "⟳",
                        cx.listener(move |this, _, _, cx| {
                            this.send(UiCommand::Reload(active_tab));
                            cx.notify();
                        }),
                    ))
                    .child(
                        div()
                            .id("url-entry")
                            .flex_1()
                            .h(px(30.0))
                            .flex()
                            .items_center()
                            .px_3()
                            .rounded_md()
                            .border_1()
                            .border_color(if self.editing_url {
                                rgb(0x9c81e8)
                            } else {
                                rgb(0x41434b)
                            })
                            .bg(rgb(0x17181c))
                            .track_focus(&self.focus_handle)
                            .on_mouse_down(
                                MouseButton::Left,
                                cx.listener(|this, _, window, cx| {
                                    this.editing_url = true;
                                    window.focus(&this.focus_handle, cx);
                                    cx.notify();
                                }),
                            )
                            .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, cx| {
                                if !this.editing_url {
                                    return;
                                }
                                let key = event.keystroke.key.as_str();
                                if key == "enter" || key == "return" {
                                    this.navigate(cx);
                                } else if key == "escape" {
                                    this.editing_url = false;
                                    cx.notify();
                                } else if key == "backspace" {
                                    this.url_text.pop();
                                    cx.notify();
                                } else if (event.keystroke.modifiers.control
                                    || event.keystroke.modifiers.platform)
                                    && key.eq_ignore_ascii_case("a")
                                {
                                    this.url_text.clear();
                                    cx.notify();
                                } else if !event.keystroke.modifiers.modified()
                                    && let Some(character) = &event.keystroke.key_char
                                {
                                    this.url_text.push_str(character);
                                    cx.notify();
                                }
                            }))
                            .child(if self.url_text.is_empty() {
                                "Search or enter address".to_owned()
                            } else {
                                self.url_text.clone()
                            }),
                    )
                    .child(
                        div()
                            .px_2()
                            .text_xs()
                            .text_color(rgb(0x858891))
                            .child(self.error.clone().unwrap_or_default()),
                    ),
            )
            .child(
                div()
                    .id("web-content")
                    .flex_1()
                    .w_full()
                    .overflow_hidden()
                    .bg(rgb(0x101115))
                    .track_focus(&self.focus_handle)
                    .on_mouse_down(
                        MouseButton::Left,
                        cx.listener(move |this, event: &gpui::MouseDownEvent, window, cx| {
                            this.editing_url = false;
                            window.focus(&this.focus_handle, cx);
                            let x = f32::from(event.position.x);
                            let y = (f32::from(event.position.y) - TOOLBAR_HEIGHT).max(0.0);
                            this.send(UiCommand::Click {
                                tab: this.active_tab,
                                x,
                                y,
                            });
                        }),
                    )
                    .on_scroll_wheel(cx.listener(|this, event: &gpui::ScrollWheelEvent, _, _| {
                        let (dx, dy) = match event.delta {
                            ScrollDelta::Pixels(delta) => (f32::from(delta.x), f32::from(delta.y)),
                            ScrollDelta::Lines(delta) => (delta.x * 40.0, delta.y * 40.0),
                        };
                        let x = f32::from(event.position.x);
                        let y = (f32::from(event.position.y) - TOOLBAR_HEIGHT).max(0.0);
                        this.send(UiCommand::Scroll {
                            tab: this.active_tab,
                            x,
                            y,
                            dx,
                            dy,
                        });
                    }))
                    .on_key_down(cx.listener(|this, event: &gpui::KeyDownEvent, _, _| {
                        if this.editing_url {
                            return;
                        }
                        if let Some(text) = event.keystroke.key_char.as_ref() {
                            this.send(UiCommand::Type {
                                tab: this.active_tab,
                                text: text.clone(),
                            });
                        } else {
                            this.send(UiCommand::Key {
                                tab: this.active_tab,
                                key: event.keystroke.key.clone(),
                            });
                        }
                    }))
                    .child(frame),
            )
    }
}

fn nav_button(
    label: &'static str,
    listener: impl Fn(&gpui::MouseDownEvent, &mut Window, &mut App) + 'static,
) -> impl IntoElement {
    div()
        .h(px(30.0))
        .min_w(px(30.0))
        .flex()
        .items_center()
        .justify_center()
        .rounded_md()
        .text_color(rgb(0xd7d8dc))
        .hover(|this| this.bg(rgb(0x34363d)))
        .cursor_pointer()
        .on_mouse_down(MouseButton::Left, listener)
        .child(label)
}

fn normalize_url(value: &str) -> String {
    let value = value.trim();
    if value.is_empty() || value.contains("://") || value.starts_with("about:") {
        value.to_owned()
    } else {
        format!("https://{value}")
    }
}

fn render_image(rgba: &RgbaImage) -> Option<Arc<RenderImage>> {
    let bgra = rgba_to_bgra(rgba);
    let buffer = RgbaImage::from_raw(rgba.width(), rgba.height(), bgra)?;
    Some(Arc::new(RenderImage::new(vec![Frame::new(buffer)])))
}

fn rgba_to_bgra(rgba: &RgbaImage) -> Vec<u8> {
    rgba.as_raw()
        .chunks_exact(4)
        .flat_map(|pixel| [pixel[2], pixel[1], pixel[0], pixel[3]])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_rgba_pixels_to_bgra_without_changing_alpha() {
        let rgba = RgbaImage::from_raw(2, 1, vec![10, 20, 30, 40, 50, 60, 70, 80]).unwrap();

        assert_eq!(rgba_to_bgra(&rgba), [30, 20, 10, 40, 70, 60, 50, 80]);
    }

    #[test]
    fn normalizes_plain_hostnames_for_navigation() {
        assert_eq!(
            normalize_url("example.com/path"),
            "https://example.com/path"
        );
        assert_eq!(normalize_url("about:blank"), "about:blank");
        assert_eq!(normalize_url("https://example.com"), "https://example.com");
    }
}
