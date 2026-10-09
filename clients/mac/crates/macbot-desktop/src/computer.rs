//! Agent Computer: the live browser surface and its takeover input bridge.
//!
//! The screen transport deliberately stays outside this module.  The transport
//! gives the view a [`Frame`], and subscribes to [`ComputerAction`] for screen
//! acknowledgements and user input.  Keeping that seam small also makes the
//! view usable with fixtures before the websocket is connected.

use std::{cell::RefCell, rc::Rc, sync::Arc, time::Instant};

use gpui_kit::component::{
    Disableable, Selectable, Sizable,
    button::{Button, ButtonVariants},
};
use gpui_kit::gpui::{
    AnyElement, App, Bounds, Context, EventEmitter, FocusHandle, Focusable, Image, ImageFormat,
    InteractiveElement, IntoElement, KeyDownEvent, KeyUpEvent, Modifiers, MouseButton,
    MouseDownEvent, MouseMoveEvent, MouseUpEvent, ObjectFit, ParentElement, Render, RenderImage,
    ScrollWheelEvent, SharedString, Styled, StyledImage, Window, div, img, px,
};
use gpui_kit::prelude::FluentBuilder;
use serde_json::{Value, json};

use crate::tokens::Tokens;

/// A decoded screen frame supplied by the screen websocket transport.
#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: u64,
    pub width: u32,
    pub height: u32,
    pub jpeg: Arc<[u8]>,
}

impl Frame {
    pub fn new(seq: u64, width: u32, height: u32, jpeg: impl Into<Arc<[u8]>>) -> Self {
        Self {
            seq,
            width,
            height,
            jpeg: jpeg.into(),
        }
    }
}

/// Intent emitted by the computer surface.
#[derive(Clone, Debug, PartialEq)]
pub enum ComputerAction {
    /// A screen websocket input message (`{"type":"input", "event": …}`).
    Input(Value),
    /// Change the active browser tab while the user has control.
    SwitchTab(String),
    /// Change screencast quality (`auto`, `high`, or `low`).
    Quality(String),
    /// Ask the main websocket to pause the bot and grant the user control.
    Takeover,
    /// Ask the main websocket to release control back to the bot.
    Release,
    /// Leave the full computer surface and return to the conversation.
    Close,
    /// The frame with this sequence number has been painted and may be acked.
    Rendered(u64),
}

impl EventEmitter<ComputerAction> for Computer {}

/// A tab entry from `ScreenState.tabs`.
#[derive(Clone, Debug, PartialEq)]
pub struct ComputerTab {
    pub id: String,
    pub title: String,
    pub url: String,
    pub active: bool,
}

/// Retained state for the Agent Computer page.
pub struct Computer {
    state: Value,
    frame: Option<Frame>,
    image: Option<Arc<Image>>,
    render_image: Option<Arc<RenderImage>>,
    image_error: Option<String>,
    pending_render_ack: Option<u64>,
    acked_render_seq: Option<u64>,
    focus_handle: FocusHandle,
    viewport_width: f32,
    viewport_height: f32,
    canvas_bounds: Rc<RefCell<Option<Bounds<gpui_kit::gpui::Pixels>>>>,
    quality: String,
    request_reason: Option<String>,
    frame_window_started: Instant,
    frame_count: u32,
    frames_per_second: f32,
    last_frame_received: Option<Instant>,
    frame_received_at: Option<(u64, Instant)>,
    paint_latency_ms: Option<f32>,
}

impl Computer {
    /// Create a computer view entity.  The shell owns the entity and keeps it
    /// alive while navigating into and out of the full-session surface.
    pub fn new(_window: &mut Window, cx: &mut Context<Self>) -> Self {
        Self {
            state: Value::Object(Default::default()),
            frame: None,
            image: None,
            render_image: None,
            image_error: None,
            pending_render_ack: None,
            acked_render_seq: None,
            focus_handle: cx.focus_handle(),
            viewport_width: 1.0,
            viewport_height: 1.0,
            canvas_bounds: Rc::new(RefCell::new(None)),
            quality: "auto".to_string(),
            request_reason: None,
            frame_window_started: Instant::now(),
            frame_count: 0,
            frames_per_second: 0.0,
            last_frame_received: None,
            frame_received_at: None,
            paint_latency_ms: None,
        }
    }

    pub fn quality(&self) -> &str {
        &self.quality
    }

    pub fn is_user_driver(&self) -> bool {
        self.driver() == "user"
    }

    /// Replace the server-provided `ScreenState`.
    ///
    /// This setter intentionally does not require a context so transport
    /// adapters can apply a state snapshot without borrowing GPUI.  Use
    /// [`Self::set_state_in`] when the mutation happens inside an entity update.
    pub fn set_state(&mut self, state: Value) {
        if let Some(quality) = state.get("quality").and_then(Value::as_str) {
            self.quality = quality.to_string();
        }
        self.state = state;
    }

    /// Replace state and schedule the owning entity for a redraw.
    pub fn set_state_in(&mut self, state: Value, cx: &mut Context<Self>) {
        self.set_state(state);
        cx.notify();
    }

    /// Set the current takeover request independently from `ScreenState`.
    /// The main websocket owns this message-level prompt, while the screen
    /// websocket only owns the browser driver state.
    pub fn set_request_reason(&mut self, reason: Option<String>, cx: &mut Context<Self>) {
        if self.request_reason != reason {
            self.request_reason = reason;
            cx.notify();
        }
    }

    /// Replace the latest frame.  Image decoding is delegated to GPUI's
    /// asynchronous image asset loader; the ack is emitted only by the
    /// callback scheduled after the first successful paint.
    pub fn set_frame(
        &mut self,
        seq: u64,
        width: u32,
        height: u32,
        jpeg: impl Into<Arc<[u8]>>,
        cx: &mut Context<Self>,
    ) {
        let received_at = Instant::now();
        let elapsed = received_at.duration_since(self.frame_window_started);
        if elapsed.as_secs_f32() >= 1.0 {
            self.frames_per_second = self.frame_count as f32 / elapsed.as_secs_f32();
            self.frame_window_started = received_at;
            self.frame_count = 0;
        }
        self.frame_count = self.frame_count.saturating_add(1);
        self.last_frame_received = Some(received_at);
        self.frame_received_at = Some((seq, received_at));
        let frame = Frame::new(seq, width, height, jpeg);
        self.frame = Some(frame.clone());
        self.image = Some(Arc::new(Image::from_bytes(
            ImageFormat::Jpeg,
            frame.jpeg.to_vec(),
        )));
        self.render_image = None;
        self.image_error = None;
        self.pending_render_ack = (self.acked_render_seq != Some(seq)).then_some(seq);
        cx.notify();
    }

    pub fn switch_tab(&mut self, tab_id: impl Into<String>, cx: &mut Context<Self>) {
        let tab_id = tab_id.into();
        if self.is_user_driver() {
            self.emit(ComputerAction::SwitchTab(tab_id), cx);
        }
    }

    pub fn set_quality(&mut self, quality: impl Into<String>, cx: &mut Context<Self>) {
        let quality = quality.into();
        if matches!(quality.as_str(), "auto" | "high" | "low") {
            self.quality = quality.clone();
            self.emit(ComputerAction::Quality(quality), cx);
            cx.notify();
        }
    }

    pub fn takeover(&mut self, cx: &mut Context<Self>) {
        self.emit(ComputerAction::Takeover, cx);
    }

    pub fn release(&mut self, cx: &mut Context<Self>) {
        self.emit(ComputerAction::Release, cx);
    }

    pub fn close(&mut self, cx: &mut Context<Self>) {
        self.emit(ComputerAction::Close, cx);
    }

    fn current_fps(&self) -> f32 {
        let Some(last_received) = self.last_frame_received else {
            return 0.0;
        };
        if last_received.elapsed().as_secs_f32() > 1.5 {
            return 0.0;
        }
        let elapsed = self.frame_window_started.elapsed().as_secs_f32();
        if elapsed > 0.0 {
            self.frame_count as f32 / elapsed
        } else {
            self.frames_per_second
        }
    }

    fn emit(&mut self, action: ComputerAction, cx: &mut Context<Self>) {
        cx.emit(action);
    }

    fn driver(&self) -> &str {
        self.state
            .get("driver")
            .and_then(Value::as_str)
            .unwrap_or("idle")
    }

    fn tabs(&self) -> Vec<ComputerTab> {
        self.state
            .get("tabs")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(|tab| {
                Some(ComputerTab {
                    id: tab.get("tab_id")?.as_str()?.to_string(),
                    title: tab
                        .get("title")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    url: tab
                        .get("url")
                        .and_then(Value::as_str)
                        .unwrap_or("")
                        .to_string(),
                    active: tab.get("active").and_then(Value::as_bool).unwrap_or(false),
                })
            })
            .collect()
    }

    fn emit_mouse(&mut self, action: &str, event: &MouseDownEvent, cx: &mut Context<Self>) {
        let Some((x, y)) =
            self.map_window_point(event.position.x.as_f32(), event.position.y.as_f32())
        else {
            return;
        };
        self.emit(
            ComputerAction::Input(json!({
                "type": "input",
                "event": {
                    "type": "mouse", "action": action, "x": x, "y": y,
                    "button": mouse_button(event.button), "click_count": event.click_count
                }
            })),
            cx,
        );
    }

    fn emit_mouse_up(&mut self, action: &str, event: &MouseUpEvent, cx: &mut Context<Self>) {
        let Some((x, y)) =
            self.map_window_point(event.position.x.as_f32(), event.position.y.as_f32())
        else {
            return;
        };
        self.emit(
            ComputerAction::Input(json!({
                "type": "input",
                "event": {
                    "type": "mouse", "action": action, "x": x, "y": y,
                    "button": mouse_button(event.button), "click_count": event.click_count
                }
            })),
            cx,
        );
    }

    fn emit_mouse_move(&mut self, event: &MouseMoveEvent, cx: &mut Context<Self>) {
        let Some((x, y)) =
            self.map_window_point(event.position.x.as_f32(), event.position.y.as_f32())
        else {
            return;
        };
        self.emit(
            ComputerAction::Input(json!({
                "type": "input",
                "event": {
                    "type": "mouse", "action": "move", "x": x, "y": y,
                    "button": event.pressed_button.map(mouse_button).unwrap_or("left"),
                    "click_count": 0
                }
            })),
            cx,
        );
    }

    fn emit_wheel(&mut self, event: &ScrollWheelEvent, cx: &mut Context<Self>) {
        let Some((x, y)) =
            self.map_window_point(event.position.x.as_f32(), event.position.y.as_f32())
        else {
            return;
        };
        let delta = event.delta.pixel_delta(px(16.0));
        self.emit(
            ComputerAction::Input(json!({
                "type": "input",
                "event": { "type": "wheel", "x": x, "y": y, "dx": delta.x.as_f32(), "dy": delta.y.as_f32() }
            })),
            cx,
        );
    }

    fn emit_key(&mut self, action: &str, event: &KeyDownEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        self.emit_key_value(
            action,
            &key.key,
            &key.key,
            key.key_char.as_deref(),
            key.modifiers,
            cx,
        );
    }

    fn emit_key_up(&mut self, action: &str, event: &KeyUpEvent, cx: &mut Context<Self>) {
        let key = &event.keystroke;
        self.emit_key_value(
            action,
            &key.key,
            &key.key,
            key.key_char.as_deref(),
            key.modifiers,
            cx,
        );
    }

    fn emit_key_value(
        &mut self,
        action: &str,
        key: &str,
        code: &str,
        text: Option<&str>,
        modifiers: Modifiers,
        cx: &mut Context<Self>,
    ) {
        self.emit(
            ComputerAction::Input(json!({
                "type": "input",
                "event": {
                    "type": "key", "action": action, "key": key, "code": code,
                    "text": text, "modifiers": modifier_names(modifiers)
                }
            })),
            cx,
        );
    }

    fn map_window_point(&self, x: f32, y: f32) -> Option<(f32, f32)> {
        let frame = self.frame.as_ref()?;
        let (x, y, viewport_width, viewport_height) = self
            .canvas_bounds
            .borrow()
            .as_ref()
            .map(|bounds| {
                (
                    x - bounds.origin.x.as_f32(),
                    y - bounds.origin.y.as_f32(),
                    bounds.size.width.as_f32(),
                    bounds.size.height.as_f32(),
                )
            })
            .unwrap_or((x, y, self.viewport_width, self.viewport_height));
        let (x, y) = map_contain_point(
            x,
            y,
            viewport_width,
            viewport_height,
            frame.width as f32,
            frame.height as f32,
        );
        Some((x, y))
    }

    fn render_frame(&mut self, window: &mut Window, cx: &mut Context<Self>) -> AnyElement {
        let t = Tokens::get(cx);
        let Some(frame) = self.frame.as_ref() else {
            return div()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.secondary)
                .child(tr("computer.empty"))
                .into_any_element();
        };
        let seq = frame.seq;

        let ready = if self.render_image.is_some() {
            true
        } else if let Some(image) = self.image.clone()
            && let Some(render_image) = image.get_render_image(window, cx)
        {
            self.render_image = Some(render_image);
            true
        } else {
            false
        };

        if ready {
            self.schedule_render_ack(seq, window, cx);
        }

        let content = if let Some(render_image) = self.render_image.clone() {
            img(render_image)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else if let Some(error) = self.image_error.as_deref() {
            div()
                .flex()
                .items_center()
                .justify_center()
                .flex_col()
                .gap_2()
                .text_color(t.secondary)
                .child(tr("computer.decode_error"))
                .child(error.to_string())
                .into_any_element()
        } else if let Some(image) = self.image.clone() {
            img(image)
                .size_full()
                .object_fit(ObjectFit::Contain)
                .into_any_element()
        } else {
            div()
                .flex()
                .items_center()
                .justify_center()
                .text_color(t.secondary)
                .child(tr("computer.loading"))
                .into_any_element()
        };

        let canvas_bounds = self.canvas_bounds.clone();
        div()
            .on_children_prepainted(move |bounds, _, _| {
                if let Some(bounds) = bounds.first() {
                    *canvas_bounds.borrow_mut() = Some(*bounds);
                }
            })
            .id("computer-frame")
            .flex()
            .items_center()
            .justify_center()
            .size_full()
            .bg(t.window)
            .overflow_hidden()
            .on_mouse_down(
                MouseButton::Left,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse("down", event, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Right,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse("down", event, cx);
                    }
                }),
            )
            .on_mouse_down(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseDownEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse("down", event, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Left,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse_up("up", event, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Right,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse_up("up", event, cx);
                    }
                }),
            )
            .on_mouse_up(
                MouseButton::Middle,
                cx.listener(|this, event: &MouseUpEvent, _, cx| {
                    if this.is_user_driver() {
                        this.emit_mouse_up("up", event, cx);
                    }
                }),
            )
            .on_mouse_move(cx.listener(|this, event: &MouseMoveEvent, _, cx| {
                if this.is_user_driver() {
                    this.emit_mouse_move(event, cx);
                }
            }))
            .on_scroll_wheel(cx.listener(|this, event: &ScrollWheelEvent, _, cx| {
                if this.is_user_driver() {
                    this.emit_wheel(event, cx);
                }
            }))
            .track_focus(&self.focus_handle)
            .on_key_down(cx.listener(|this, event: &KeyDownEvent, _, cx| {
                if this.is_user_driver() {
                    this.emit_key("down", event, cx);
                }
            }))
            .on_key_up(cx.listener(|this, event: &KeyUpEvent, _, cx| {
                if this.is_user_driver() {
                    this.emit_key_up("up", event, cx);
                }
            }))
            .child(content)
            .into_any_element()
    }

    fn schedule_render_ack(&mut self, seq: u64, window: &mut Window, cx: &mut Context<Self>) {
        if self.pending_render_ack != Some(seq) || self.acked_render_seq == Some(seq) {
            return;
        }
        self.pending_render_ack = None;
        cx.on_next_frame(window, move |this, _window, cx| {
            if this.frame.as_ref().is_some_and(|frame| frame.seq == seq)
                && this.acked_render_seq != Some(seq)
            {
                this.acked_render_seq = Some(seq);
                if let Some((received_seq, received_at)) = this.frame_received_at
                    && received_seq == seq
                {
                    this.paint_latency_ms = Some(received_at.elapsed().as_secs_f32() * 1_000.0);
                }
                this.emit(ComputerAction::Rendered(seq), cx);
            }
        });
    }
}

impl Focusable for Computer {
    fn focus_handle(&self, _cx: &App) -> FocusHandle {
        self.focus_handle.clone()
    }
}

impl Render for Computer {
    fn render(&mut self, window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let driver = self.driver().to_string();
        let tabs = self.tabs();
        let title = self
            .state
            .get("title")
            .and_then(Value::as_str)
            .or_else(|| {
                tabs.iter().find(|tab| tab.active).and_then(|tab| {
                    (!tab.title.is_empty())
                        .then_some(tab.title.as_str())
                        .or_else(|| (!tab.url.is_empty()).then_some(tab.url.as_str()))
                })
            })
            .unwrap_or(tr("computer.title"))
            .to_string();
        let quality = self.quality.clone();
        let frame = self.render_frame(window, cx);
        let fps = self.current_fps();
        let paint_latency = self.paint_latency_ms;
        let status = match driver.as_str() {
            "bot" => tr("computer.bot_working"),
            "user" => tr("computer.user_control"),
            _ => tr("computer.idle"),
        };
        let action_label = if driver == "user" {
            tr("computer.return")
        } else {
            tr("computer.takeover")
        };
        let is_user = driver == "user";
        let width = self.frame.as_ref().map(|f| f.width).unwrap_or(0);
        let height = self.frame.as_ref().map(|f| f.height).unwrap_or(0);
        let t = Tokens::get(cx);
        let request_banner = self.request_reason.clone().map(|reason| {
            let request_is_user = is_user;
            div()
                .flex()
                .items_center()
                .gap_3()
                .px_4()
                .py_2()
                .bg(t.attention)
                .text_color(t.window)
                .child(div().flex_1().child(format!("⚑ {reason}")))
                .child(
                    Button::new("computer-request-control")
                        .primary()
                        .small()
                        .label(if request_is_user {
                            tr("computer.return")
                        } else {
                            tr("computer.takeover")
                        })
                        .on_click(cx.listener(move |this, _, _, cx| {
                            if request_is_user {
                                this.release(cx);
                            } else {
                                this.takeover(cx);
                            }
                        })),
                )
        });

        div()
            .id("computer")
            .flex()
            .flex_col()
            .size_full()
            .bg(t.window)
            .text_color(t.primary)
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(title)
                            .child(div().text_sm().text_color(t.secondary).child(status)),
                    )
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_1()
                            .child(
                                Button::new("computer-control")
                                    .primary()
                                    .small()
                                    .label(action_label)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        if is_user {
                                            this.release(cx);
                                        } else {
                                            this.takeover(cx);
                                        }
                                    })),
                            )
                            .child(
                                Button::new("computer-close")
                                    .ghost()
                                    .small()
                                    .label(tr("computer.close"))
                                    .on_click(cx.listener(|this, _, _, cx| this.close(cx))),
                            ),
                    ),
            )
            .child(
                div()
                    .flex()
                    .items_center()
                    .gap_2()
                    .px_4()
                    .py_2()
                    .border_b_1()
                    .border_color(t.border)
                    .children(tabs.into_iter().map(|tab| {
                        let tab_id = tab.id.clone();
                        let selected = tab.active;
                        Button::new(SharedString::from(format!("computer-tab-{}", tab.id)))
                            .ghost()
                            .small()
                            .label(if tab.title.is_empty() {
                                tab.url
                            } else {
                                tab.title
                            })
                            .selected(selected)
                            .disabled(!is_user)
                            .on_click(cx.listener(move |this, _, _, cx| {
                                this.switch_tab(tab_id.clone(), cx)
                            }))
                    })),
            )
            .child(div().flex_1().child(frame))
            .when_some(request_banner, |this, banner| this.child(banner))
            .child(
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .px_4()
                    .py_2()
                    .border_t_1()
                    .border_color(t.border)
                    .child(div().text_sm().text_color(t.secondary).child(format!(
                        "{} · {} × {} · {} {:.1}/s · {} {}",
                        tr("computer.quality"),
                        quality,
                        if width == 0 {
                            "—".to_string()
                        } else {
                            format!("{} × {}", width, height)
                        },
                        tr("computer.frames"),
                        fps,
                        tr("computer.paint_latency"),
                        paint_latency
                            .map(|latency| format!("{latency:.0}ms"))
                            .unwrap_or_else(|| "—".to_string())
                    )))
                    .child(
                        div()
                            .flex()
                            .gap_1()
                            .children(["auto", "high", "low"].into_iter().map(|value| {
                                let value = value.to_string();
                                let label = value.clone();
                                let selected = value == quality;
                                Button::new(SharedString::from(format!("computer-quality-{value}")))
                                    .ghost()
                                    .small()
                                    .label(label)
                                    .selected(selected)
                                    .on_click(cx.listener(move |this, _, _, cx| {
                                        this.set_quality(value.clone(), cx)
                                    }))
                            })),
                    ),
            )
            .into_any_element()
    }
}

fn map_contain_point(
    x: f32,
    y: f32,
    viewport_w: f32,
    viewport_h: f32,
    frame_w: f32,
    frame_h: f32,
) -> (f32, f32) {
    if frame_w <= 0.0 || frame_h <= 0.0 {
        return (0.0, 0.0);
    }
    let scale = (viewport_w / frame_w).min(viewport_h / frame_h);
    let drawn_w = frame_w * scale;
    let drawn_h = frame_h * scale;
    let offset_x = (viewport_w - drawn_w) / 2.0;
    let offset_y = (viewport_h - drawn_h) / 2.0;
    (
        ((x - offset_x) / scale).clamp(0.0, frame_w - 1.0),
        ((y - offset_y) / scale).clamp(0.0, frame_h - 1.0),
    )
}

fn mouse_button(button: MouseButton) -> &'static str {
    match button {
        MouseButton::Left => "left",
        MouseButton::Right => "right",
        MouseButton::Middle => "middle",
        MouseButton::Navigate(_) => "left",
    }
}

fn modifier_names(modifiers: Modifiers) -> Vec<&'static str> {
    let mut names = Vec::new();
    if modifiers.control {
        names.push("ctrl");
    }
    if modifiers.alt {
        names.push("alt");
    }
    if modifiers.shift {
        names.push("shift");
    }
    if modifiers.platform {
        names.push("cmd");
    }
    names
}

fn tr(key: &str) -> &'static str {
    match key {
        "computer.title" => "Agent Computer",
        "computer.bot_working" => "Bot 操作中",
        "computer.user_control" => "你正在操作（Bot 已暂停）",
        "computer.takeover" => "接管并处理",
        "computer.return" => "交还给 Bot",
        "computer.empty" => "暂无画面",
        "computer.loading" => "加载中…",
        "computer.decode_error" => "画面解码失败",
        "computer.idle" => "空闲",
        "computer.quality" => "画质",
        "computer.close" => "收起",
        "computer.frames" => "帧率",
        "computer.paint_latency" => "绘制延迟",
        _ => "—",
    }
}

#[cfg(test)]
mod tests {
    use super::map_contain_point;

    #[test]
    fn contain_mapping_preserves_aspect_ratio() {
        assert_eq!(
            map_contain_point(500.0, 250.0, 1000.0, 500.0, 2000.0, 1000.0),
            (1000.0, 500.0)
        );
        assert_eq!(
            map_contain_point(0.0, 0.0, 1000.0, 1000.0, 2000.0, 1000.0),
            (0.0, 0.0)
        );
        assert_eq!(
            map_contain_point(500.0, 0.0, 1000.0, 1000.0, 2000.0, 1000.0),
            (1000.0, 0.0)
        );
    }
}
