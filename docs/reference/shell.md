# The native shell and the widget contract

The native desk (`core/desktop`, the `unidtk-shell` library, drawn by Rune) replaces the page step by step ([the Rune host plan](../dev/plan-rune-host.md)). This chapter states what the shell owns, what a module's widget owns, and the contract between them. The page's own rules stay in [desk.md](desk.md), [motion.md](motion.md) and [surfaces.md](surfaces.md); the shell keeps them, and this chapter says how.

## Who owns what

r[shell.widget-contract]

A module that draws on the native desk has a third half, `modules/<id>/native/`, the crate `unidtk-native-<id>` ([modules.vertical-slice](modules.md#r-modules.vertical-slice)). It implements `unidtk_shell::widget::Widget` and exports one `WidgetType` (its module id, the kernel events it receives, and how one is made). The `unidtk-desktop` binary (`core/desktop/main.rs`) registers every native half; `toolchain/build.rs` fails the build when a half is missing from that list or from the application's dependencies.

The shell owns, once, for every widget:

- where the card lands: the record, its anchor, the theme's preset size, the module's `defaultPlacement` ([desk.positions](desk.md#r-desk.positions));
- the card's chrome in both materials, with its grain, sheen, wash and shadows ([theme.tokens-only](desk.md#r-theme.tokens-only)), the header of a header card, the strip radius;
- the one surface morph from tile to surface ([shell.one-morph](#r-shell.one-morph));
- the nesting zone ([desk.nesting](desk.md#r-desk.nesting)), arrange mode, the hit region;
- the wake schedule ([shell.wake-schedule](#r-shell.wake-schedule)) and the kernel link.

A widget owns only what is its own: what it draws inside the chrome (`content`), the size of its surface (`surface`), its compact form for a host's zone (`compact`), the controls it adds over its tile in arrange mode (`arrange_tools`), and how it answers the kernel's events, a timer, Escape and a press on the bare desk.

What a module declares as data lives in its `metadata.json` ([modules.metadata](modules.md#r-modules.metadata)): `"chrome": "bare"` (the widget draws the whole card, no header; the default is a header card), `"flatChrome": "none"` (under the flat material the card paints nothing of its own and does not clip: a transparent strip whose glow is not cut), `"radius": "strip"` (a full pill under jelly), `instanceSize` (an added instance's size), `container`, `embeddable`, `singleton`, the tints and the default placement. `build.rs` refuses any other value of `chrome`, `flatChrome` or `radius`, and an `instanceSize` without a numeric `w` and `h`.

A widget reads through a `View` (the theme, the motion tokens, the local time, its tile and box, the morph's progress) and writes through an `Act` (kernel commands whose answers come back to it, its own record through `desk_apply`, toasts, the debug line, its surface). It never holds the shell's state.

## Identity

r[shell.widget-identity]

A widget lives under its desk id (the module id, or an instance's id) with a generation. The shell makes, keeps and drops widgets so they are the ones the layout shows, and tells each its record when the layout changes. A handler a widget binds (`View::on`, `View::on_drag`, `View::edit`) finds the widget again by its id when the press arrives; a widget gone meanwhile is not found and nothing happens. An answer to a command (`Act::call`) reaches the widget only if it is the same generation; a widget made later under the same id never receives the old one's answer.

Every element a widget keys carries its desk id, so two instances of one module never share a key.

## The morph

r[shell.one-morph]

Every object-to-surface open on the native desk is one morph, driven frame by frame by the shell from one progress value, with the page's curves ([motion.clip-morph](motion.md#r-motion.clip-morph)):

- the card is laid out at its surface's box from the first frame; what moves is the clip, from the tile's rectangle and radius to the whole box and the surface radius;
- the open runs on `expoOut` over `calculateDuration` of the growth;
- the close runs on `surfaceExit` through three keyframes: from where the clip is, to 85 % of the way at 0.8 with the tile's radius, to the tile; it lasts `0.65 x T x sqrt(r)`, `r` the share of the way still to go;
- a reversal starts the other leg from the clip as it is ([motion.reversal](motion.md#r-motion.reversal));
- the jelly wash fades in over 0.3 of the open on `expoOut` and out over the second half of the close on `surfaceExit`;
- the content is not faded: the clip alone reveals and hides it, as the page opens widgets with their body transitions off;
- the header's icon flies from the tile into the box, and a widget flies its own parts with `View::fly`, which places a rectangle between the tile and the surface at the morph's progress ([motion.shared-flight](surfaces.md#r-motion.shared-flight));
- whatever starts, turns or closes a morph (the shell's open and close, the menu's sheet) calls `surface::keep_frames` in the same handler: Rune runs a frame's `poll`, which steps the morph, before the input of that frame, and every commit here is `instant`, so without the call no frame follows the press and the morph waits for the next timer wake.

Every other element that appears and leaves (a toast, the arrange island, a resize handle, the "+ Виджет" popover, a widget's arrange tools) enters and leaves on the page's token and easing through `Motion::presence`, which gives Rune's `Transition` the CSS curve in place of its presence springs ([motion.canonical-tokens](motion.md#r-motion.canonical-tokens)). A paint change Rune moves on a spring (a border that turns 14 % in arrange mode, a switch's track) takes one without bounce over the same token.

A morph can always be taken back: a press on a card that is opening (its header, its body, a bare card anywhere) folds it from where it is, and a press on one that is closing opens it again from where it is ([motion.reversal](motion.md#r-motion.reversal)). The page blocked both (`pointer-events: none`); the native desk does not. A press on the bare desk outside the shell (the kernel's mouse hook sees it land on `Progman`, `WorkerW` or the icons) closes the top layer, as the page's outside-click listener hears presses on the desk only; a press in another window takes the keys back from the shell and closes nothing. One surface is open at a time ([motion.widget-integrity](motion.md#r-motion.widget-integrity)); a closing one draws above the cards at rest, the one opened last on top.

## Waking

r[shell.wake-schedule]

The desk wakes at the earliest of what is due: each widget's own wake (`Widget::wake`, the seconds until it has something new to show), the toast's end, the menu's slide-out, the debug line, the end of the settle after arrange mode (one motion step in which the desk takes no press, `body.arrange-settling`). A widget's wake is never sooner than a frame, so a widget that asks for "now" does not spin the desk. While a morph runs the desk draws every frame and then stops. An idle desk wakes only when a widget asks: the clock once a second or once a minute by its face, a VPN panel every 3 s while it is on screen.

## The window region

r[shell.window-region]

The shell is one window over the whole desk; a window region (`SetWindowRgn`) decides where it takes the pointer, so a press anywhere else reaches the desktop, in any process. A region also clips what the window shows. It is the union of every `widget:` element (a card's whole box) and `hit:` element, each with at least 8 pt round it for a focus ring or an outline, and every card's painted body (`card:`), each with as much as its own shadows reach (`ElementInfo::shadow_extent`). A shadow is never cut, and a card that morphs keeps one region from the start of the morph to its end: its box does not move, and its body, which has no shadow while it is open, stays inside it. A press inside the region that no element takes (a shadow's reach, the part of a growing surface's box the clip has not reached) is a press on the desk; every surface takes the presses inside itself. Each rect is where the last frame drew the element, or, when a commit came after that frame (a covered desk commits without drawing), where that commit placed it: the region always describes the committed tree, so the first press after a start lands.

Every child of the shell's root and every card is keyed. Rune knows an unkeyed element by its place among its siblings, and a card that opens over the others, or the grid arrange mode puts in front, changes those places.

## The backdrop

r[shell.backdrop]

A card of either material blurs what lies under it (`backdrop-filter: blur(--theme-blur)` on `.widget-card`). The shell is a transparent child of the desktop, so what lies under a card is the wallpaper, which the window cannot read. The shell reads the wallpaper itself (the file, `WallpaperStyle`, `TileWallpaper`, the desktop colour), lays it out on the primary display as Windows does, and blurs a quarter-size copy once, on a worker, when the wallpaper or the theme's blur changes. A card draws its part of that copy under its glass or its jelly; when another card lies under it, that card is not in the blur (the page blurred what it drew, cards included).
