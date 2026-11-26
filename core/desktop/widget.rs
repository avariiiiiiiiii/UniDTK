//! The widget contract: what a module's native half (`modules/<id>/native`) implements to live
//! on the desk, and the two handles the shell gives it (`r[shell.widget-contract]`).
//!
//! The shell owns everything every widget shares, once: where the card lands (`desk.rs`), its
//! chrome in both materials, the one surface morph from tile to surface (`surface.rs`), the
//! nesting zone, arrange mode, the hit region, the wake schedule and the kernel link. A widget
//! owns only what is its own: what it draws inside the chrome, how big its surface is, what it
//! shows when nested, the tools it adds in arrange mode, and how it reacts to the kernel's
//! events, a timer, Escape and a press outside.
//!
//! A widget reads through [`View`] (the theme, the time, its tile and box, the morph's
//! progress) and writes through [`Act`] (kernel commands whose answers come back to it, its own
//! record, toasts, its surface). It never holds the shell's state: Rune's handlers are typed on
//! the app state, so [`View::on`] and its siblings bind a handler to the widget by its desk id
//! and find it again when the press arrives. A widget removed meanwhile is simply not found.
//!
//! What a module declares as data lives in its `metadata.json` (`r[modules.metadata]`): the
//! chrome (`"chrome": "bare"` draws no header), the strip radius, `container`, `embeddable`,
//! `singleton`, the tints and the default placement.

use std::any::Any;

use rune::el::{Drag, El};
use rune::{Color, TextEdit, Tx};
use serde_json::Value;

pub use crate::desk::{LocalTime, Placement};
pub use crate::motion::{Motion, Token};
pub use crate::theme::{Material, Theme};
pub use crate::Host;

/// A module's widget type, registered by the `unidtk-desktop` binary.
pub struct WidgetType {
    /// The module id, as in `metadata.json` and the layout's records.
    pub id: &'static str,
    /// Kernel events this type receives (`Widget::on_event`); the link subscribes to the union
    /// of every registered type's events once, at connect.
    pub events: &'static [&'static str],
    pub make: fn(&Init<'_>) -> Box<dyn Widget>,
    /// Optional menu pane, available even when this module has no tile (r[shell.widget-contract]).
    pub settings: Option<crate::settings::Factory>,
}

/// What a new widget is made from.
pub struct Init<'a> {
    /// Its desk id: the module id, or an instance's id.
    pub id: &'a str,
    /// Its record in the layout (`positions[id]`), `Null` when it has none.
    pub record: &'a Value,
}

/// How big a widget's surface is, in points.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum SurfaceSize {
    /// The whole box (a bare card such as the clock's dashboard).
    Box(f32, f32),
    /// The body under a header card's header: the box is 400 wide and the body's height and
    /// 80 more high, at most 500 (`toggleWidget`).
    Body(f32),
}

/// A widget's surface.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Surface {
    pub size: SurfaceSize,
    /// Where a bare card's nesting zone starts, in the box's coordinates: `[x, y, width]`. A
    /// header card's zone follows its body.
    pub nest: Option<[f32; 3]>,
}

pub trait Widget: Any {
    /// Its record changed (a face, a setting); the layout's geometry is the shell's.
    fn sync(&mut self, _record: &Value) {}

    /// What it draws inside the chrome, in the box's coordinates (`View::frame`). A bare card
    /// draws its tile and its surface alike (the morph clips them); a header card draws the
    /// body under its header, and only while its surface is shown.
    fn content(&self, v: &View<'_>) -> Option<El>;

    /// Its surface, or `None` when it does not open.
    fn surface(&self, _v: &View<'_>) -> Option<Surface> {
        None
    }

    /// The compact form a host's nesting zone draws, and its height; `None` when it has none
    /// (it cannot be nested).
    fn compact(&self, _v: &View<'_>) -> Option<(El, f32)> {
        None
    }

    /// Controls it adds over its tile in arrange mode, in the tile's coordinates (the clock's
    /// face switcher). Their keys must start with `hit:`.
    fn arrange_tools(&self, _v: &View<'_>) -> Vec<El> {
        Vec::new()
    }

    /// Seconds from now until it has something new to show; `None` sleeps. The shell wakes at
    /// the earliest of every widget's.
    fn wake(&self, _now: &LocalTime) -> Option<f64> {
        None
    }

    /// The wake it asked for came; `true` draws again.
    fn tick(&mut self, _now: &LocalTime, _act: &mut Act<'_>) -> bool {
        false
    }

    /// A kernel event of its type's `events`. `snapshot` marks the catch-up frame a subscribe
    /// or a reconnect replays, which must not repeat a side effect. An event does not draw
    /// the desk again by itself: call [`Act::redraw`] when what the widget shows changed
    /// (`r[shell.redraw-on-change]`).
    fn on_event(&mut self, _name: &str, _payload: &Value, _snapshot: bool, _act: &mut Act<'_>) {}

    /// It came on screen as a surface (its own, or its host's nesting zone) or left it.
    fn shown(&mut self, _visible: bool, _act: &mut Act<'_>) {}

    /// Escape while it is on screen: `true` when it closed something of its own (a list, a
    /// popup), so the surface stays open.
    fn escape(&mut self, _act: &mut Act<'_>) -> bool {
        false
    }

    /// The wheel turned `notches` over its tile (a notch is 120 units; positive is away from the
    /// user, as the page's `deltaY < 0`), while no surface or menu is open. `touchpad` marks
    /// travel that follows the system's scroll direction (natural: fingers down is positive),
    /// which a control that wants the fingers' own direction turns back. `true` when it took it.
    fn wheel(&mut self, _notches: i32, _touchpad: bool, _act: &mut Act<'_>) -> bool {
        false
    }

    /// A press on the bare desk: `true` when it closed something of its own.
    fn outside(&mut self, _act: &mut Act<'_>) -> bool {
        false
    }
}

/// What a widget reads while it draws.
#[derive(Clone, Copy)]
pub struct View<'a> {
    /// Its desk id.
    pub id: &'a str,
    pub theme: &'a Theme,
    pub motion: &'a Motion,
    pub now: LocalTime,
    pub canvas: (f32, f32),
    /// Its tint (`--own-tint`) and the ink readable on it.
    pub tint: Color,
    pub ink: Color,
    /// The box it opens into, on the canvas; everything it draws is in this box's coordinates.
    pub frame: Placement,
    /// Its tile in the box's coordinates.
    pub tile: Placement,
    /// How far its surface is open, 0 (the tile) to 1 (the whole box), as the morph shows it
    /// now: what a part that flies between its tile and its surface follows.
    pub progress: f32,
    /// Its surface is open (or opening).
    pub open: bool,
    pub arranging: bool,
    pub(crate) host: &'a Host,
}

impl<'a> View<'a> {
    /// The same view over another box: what a widget measures its surface in.
    pub fn with_frame(&self, frame: Placement) -> View<'a> {
        View { frame, ..*self }
    }

    /// A rectangle between `tile` (closed) and `surface` (open) at the morph's progress: a
    /// part that flies from the tile into the surface (`r[motion.shared-flight]`).
    pub fn fly(&self, tile: Placement, surface: Placement) -> Placement {
        let t = self.progress.clamp(0.0, 1.0);
        let lerp = |a: f32, b: f32| a + (b - a) * t;
        Placement { x: lerp(tile.x, surface.x), y: lerp(tile.y, surface.y), w: lerp(tile.w, surface.w), h: lerp(tile.h, surface.h) }
    }

    /// An SVG tinted and rasterized by the image cache, or an empty box of its size until it is.
    pub fn svg(&self, markup: &str, size: f32, colour: Color) -> El {
        crate::shell::svg_icon(self.host, markup, size, colour)
    }

    /// Binds a press to this widget: `f` runs on it, if it is still on the desk.
    pub fn on<W: Widget>(&self, f: impl Fn(&mut W, &mut Act<'_>) + 'static) -> impl Fn(&mut Host, &mut Tx) + 'static {
        let id = self.id.to_string();
        move |host: &mut Host, tx: &mut Tx| {
            host.with_widget::<W>(&id, tx, |widget, act| f(widget, act));
        }
    }

    /// Binds a drag to this widget.
    pub fn on_drag<W: Widget>(&self, f: impl Fn(&mut W, &mut Drag, &mut Act<'_>) + 'static) -> impl Fn(&mut Host, &mut Drag, &mut Tx) + 'static {
        let id = self.id.to_string();
        move |host: &mut Host, drag: &mut Drag, tx: &mut Tx| {
            host.with_widget::<W>(&id, tx, |widget, act| f(widget, drag, act));
        }
    }

    /// A text field's state inside this widget, for `text_input(..).on_edit(..)`. A widget
    /// gone meanwhile edits a scratch field nobody reads.
    pub fn edit<W: Widget>(&self, field: fn(&mut W) -> &mut TextEdit) -> impl Fn(&mut Host) -> &mut TextEdit + 'static {
        let id = self.id.to_string();
        move |host: &mut Host| host.widget_field::<W>(&id, field)
    }

    /// An element that swallows presses, so a press on a widget's own controls does not reach
    /// the card under them (`stopPropagation`).
    pub fn swallow(&self, element: El) -> El {
        element.on_press(|_: &mut Host| {})
    }
}

/// A widget's write path. Everything it asks for is applied by the shell after the handler
/// returns, so the widget never holds the shell's state.
pub struct Act<'t> {
    pub(crate) id: String,
    pub(crate) ops: Vec<Op>,
    pub tx: &'t mut Tx,
}

pub(crate) type Reply = Box<dyn FnOnce(&mut dyn Widget, Result<Value, String>, &mut Act<'_>) + Send>;

pub(crate) enum Op {
    Call { cmd: String, args: Value, reply: Option<Reply>, timeout: std::time::Duration },
    Save(Value),
    Toast { text: String, error: bool },
    Debug(String),
    Log(String),
    Open,
    Close,
    Redraw,
}

impl<'t> Act<'t> {
    pub(crate) fn new(id: &str, tx: &'t mut Tx) -> Act<'t> {
        Act { id: id.to_string(), ops: Vec::new(), tx }
    }

    /// Its desk id.
    pub fn id(&self) -> &str {
        &self.id
    }

    /// A kernel command; `then` runs on this widget with the answer, if it is still the same
    /// widget on the desk when the answer comes.
    pub fn call<W: Widget>(&mut self, cmd: &str, args: Value, then: impl FnOnce(&mut W, Result<Value, String>, &mut Act<'_>) + Send + 'static) {
        self.call_within(cmd, args, crate::ws_client::REPLY_TIMEOUT, then);
    }

    /// [`Act::call`] for a command that runs for seconds (`long` in its module's contract,
    /// `r[ipc.long-commands-off-thread]`): its answer is waited for `timeout`.
    pub fn call_within<W: Widget>(&mut self, cmd: &str, args: Value, timeout: std::time::Duration, then: impl FnOnce(&mut W, Result<Value, String>, &mut Act<'_>) + Send + 'static) {
        let reply: Reply = Box::new(move |widget: &mut dyn Widget, result, act| {
            let any: &mut dyn Any = widget;
            if let Some(widget) = any.downcast_mut::<W>() {
                then(widget, result, act);
            }
        });
        self.ops.push(Op::Call { cmd: cmd.to_string(), args, reply: Some(reply), timeout });
    }

    /// A kernel command whose answer nobody waits for; a failure is logged.
    pub fn send(&mut self, cmd: &str, args: Value) {
        self.ops.push(Op::Call { cmd: cmd.to_string(), args, reply: None, timeout: crate::ws_client::REPLY_TIMEOUT });
    }

    /// Merges `fields` into its own record through `desk_apply` (a face, a setting).
    pub fn save(&mut self, fields: Value) {
        self.ops.push(Op::Save(fields));
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Toast { text: text.into(), error: false });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Toast { text: text.into(), error: true });
    }

    /// `showDebugMessage`: the red line at the bottom of the desk.
    pub fn debug(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Debug(text.into()));
    }

    /// A line in the kernel's log under the frontend source.
    pub fn log(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Log(text.into()));
    }

    /// Opens its surface (another open one folds first).
    pub fn open(&mut self) {
        self.ops.push(Op::Open);
    }

    /// Folds its surface.
    pub fn close(&mut self) {
        self.ops.push(Op::Close);
    }

    /// Draw again: what it shows changed. A kernel event draws nothing without it.
    pub fn redraw(&mut self) {
        self.ops.push(Op::Redraw);
    }
}

/// Whether the machine is online now (`navigator.onLine`).
pub fn online() -> bool {
    crate::desktop_host::is_online()
}
