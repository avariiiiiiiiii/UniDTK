# Chapter 6. The page

The window behind the desktop icons is a WebView2 page: ordinary HTML, CSS, and JavaScript, served from the application's own files. This chapter follows the page from its first byte to a mounted widget. It shows how the source trees become the served page, how startup finds the modules and builds one card per widget, how a card is placed and kept in place when the screen changes, and how the page calls the kernel through one bridge file. After it, you can add a widget to the page and know which file each step belongs to.

## 6.1 From source trees to the served page

The build script `toolchain/build.rs` copies three source trees into one folder, `toolchain/dist`, and the Tauri configuration serves that folder. The function `stage_dist` does the copying. The shared page code goes to the root, each module's `ui/` folder goes to `modules/<id>/`, and the fonts and vendor files go to `assets/`. The Tauri resources section then copies `dist/modules/` into the installed application, so the page finds module files in the same relative place there.

Note: the page refers to module files with paths such as `../modules/glyph-bar/glyph-bar.css`. A URL cannot climb above its site root, so `..` from the root resolves to `modules/...` inside the served folder.

## 6.2 The entry page

The entry page is `core/ui/index.html`. Its head does three things before any widget exists.

Filename: core/ui/index.html (abridged)

```html
<script>
  // Classic script, so it runs before the stylesheets apply.
  var paint = JSON.parse(localStorage.getItem("unidtk_theme_state") || "{}").paint;
  // ... set CSS custom properties on <html> from paint.vars ...
</script>
<link rel="stylesheet" href="../modules/glyph-bar/glyph-bar.css" />
<!-- one line per module that has CSS; nothing adds these automatically -->
<script type="importmap">
{ "imports": { "@unidtk/bridge": "./unidtk-bridge.js" } }
</script>
```

The first script paints the last known theme before the first frame, so the page does not flash default colors. It only reads the cache. The stylesheet lines are written by hand. The import map gives the page one name for the kernel, `@unidtk/bridge`, which points at the bridge of section 6.8.

At the end of the body, three module scripts load in order: `boot-log.js`, `harness.js`, and `renderer.js`. The harness keeps the last 50 errors in `window.__UNIDTK_ERRORS__`, so a test can read them while the app runs. The renderer is one line, `import "./unidtk-core.js";`, and importing that file runs the whole page.

## 6.3 The core file: unidtk-core.js

`unidtk-core.js` is the largest shared file. Its top-level code starts the subsystems and then loads the widgets.

Filename: core/ui/unidtk-core.js (abridged)

```js
import { invoke, listen } from "@unidtk/bridge";
import { getWidgets, getWidgetConfig, getDisabledModules } from "./widgets-api.js";
// ... motion.js, theme.js, nesting.js, menu.js, folder-body.js ...

initMotionSystem();
initThemeSystem();
initMenu();

const widgetsCanvas = document.getElementById("widgets");
let config = { order: [], align: "left" };

listen("display-changed", handleDisplayOrResizeChange).catch(() => {});
window.addEventListener("resize", handleDisplayOrResizeChange);
loadWidgets();
```

`widgetsCanvas` is the element that holds every card, and `config` is the saved layout. `loadWidgets()` is the only call that builds the canvas.

## 6.4 Startup: from module folders to cards

`loadWidgets` runs once, when the page starts. It loads the saved layout, clears the canvas, and builds one card per widget.

Filename: core/ui/unidtk-core.js (abridged)

```js
async function loadWidgets() {
  await loadConfig();
  const widgets = await fetchRenderableWidgets();
  if (!widgets) return;
  widgetsCanvas.innerHTML = "";   // startup only: the one blunt reset
  desiredWidgets(widgets).forEach((w) => {
    widgetsCanvas.appendChild(buildWidgetCard(w, { offsetY: 28 }));
  });
  requestAnimationFrame(() => notifyGeometryChanged("load"));
}
```

The reset is harmless here because the canvas holds nothing yet. The rule [desk.relayout-not-reset](../reference/desk.md#r-desk.relayout-not-reset) forbids the same reset on geometry changes, and later changes use the functions of section 6.7. The canvas reset appears only in this startup path.

`getWidgets` in `core/ui/widgets-api.js` builds the widget list from the resource folder.

Filename: core/ui/widgets-api.js (abridged)

```js
export async function getWidgets() {
  const moduleEntries = await readResourceDir("modules");
  const moduleIds = moduleEntries.filter((e) => e.isDirectory && e.name !== "explorer").map((e) => e.name);
  const modules = await Promise.all(moduleIds.map(async (id) => {
    const meta = JSON.parse(await readResourceText(`modules/${id}/metadata.json`));
    return { id, name: meta.name || id, ...meta };
  }));
  // ... user folders from invoke("get_folders") are appended ...
  return [...modules, ...folderWidgets];
}
```

A module appears when its folder is in the resource tree and its `metadata.json` parses. The folder name becomes the widget id. `desiredWidgets` then hides a widget whose id is in `config.hidden`, or one that has a backend and the kernel reports as switched off. A new module therefore appears on the desk by default, with no entry in the saved layout. The `backend` field is the one the menu reads, described in [Chapter 4](ch04-modules.md).

## 6.5 The metadata fields

Each module describes itself in `metadata.json`. The `glyph-bar` file is the example. Its label values are Russian in the source, and the keys are what matter here.

Filename: modules/glyph-bar/ui/metadata.json (abridged)

```json
{
  "name": "Громкость",
  "specialType": "glyph-bar",
  "tints": [ { "id": "glyph-bar", "label": "Громкость", "default": "aqua", "role": "player" } ],
  "backend": true
}
```

| Field | Read by | Meaning |
| :--- | :--- | :--- |
| `name`, `description` | `getWidgets`, the card header | the text in the menu and on the card |
| `specialType` | `buildWidgetCard` | which built-in renderer mounts the widget (section 6.6) |
| `backend` | `desiredWidgets`, `menu.js` | the module has a backend, so the menu offers a switch |
| `container`, `embeddable` | `core/ui/nesting.js` | the widget hosts others, or lives inside another widget |
| `tints` | `core/ui/theme.js` | the colors the theme may paint on the module |
| `simulate` | the build lint, and `udtk doctor -simulate` | the file and function the doctor probes |
| `motion` | the build lint, and `udtk doctor -simulate` | `open`: what in the card to click to open its surface for the motion budget |

## 6.6 Building a card and mounting the widget

`buildWidgetCard` turns one widget description into a `div` in the canvas. It sets the position, writes the template, and mounts the module's renderer.

Filename: core/ui/unidtk-core.js (abridged)

```js
function buildWidgetCard(w, layout) {
  const card = document.createElement("div");
  card.className = "widget-card";
  card.dataset.id = w.id;
  const at = placeOnCanvas(config.positions[w.id]);
  card.style.left = at.x + "px";
  card.style.top = at.y + "px";
  card.innerHTML = `
    <div class="widget-header"><div class="widget-title">${w.name}</div></div>
    <div class="widget-body"></div>`;
  // ... the mount branch, below ...
  return card;
}
```

The mount is a fixed branch on `specialType`. The module's `widget.js` loads with a dynamic `import()`, so the core never names it statically.

Filename: core/ui/unidtk-core.js (abridged)

```js
} else if (w.specialType === "glyph-bar") {
  import("../modules/glyph-bar/widget.js").then((m) => m.renderGlyphBar(card, w.id)).catch(() => {});
}
```

Warning: the list of types is fixed in the core file. A `specialType` that is not in the list gets no renderer and falls back to the generic shortcut view. A new module with its own renderer must add a branch here. The function `_renderWidget` in the same file has a switch of the same kind. It is exported, but nothing calls it, so the branch in `buildWidgetCard` is the one that runs.

A renderer writes into the card it receives, as the `glyph-bar` renderer does.

Filename: modules/glyph-bar/ui/widget.js (abridged)

```js
card.classList.add("no-header");
card.insertAdjacentHTML("beforeend", `<div class="gb-container">${segs}</div>`);
```

The class `no-header` hides the core header, as `core/ui/styles.css` defines. The renderer appends with `insertAdjacentHTML` on purpose. Setting the card's `innerHTML` again would replace the remove button the core wired, and that button would stop working.

## 6.7 Placement, and the rule against resetting the desk

A card's position is data. The layout stores `x` and `y` for each widget, and optionally a corner anchor. `placeOnCanvas` turns that data into pixels for the current screen.

Filename: core/ui/unidtk-core.js (abridged)

```js
function placeOnCanvas(pos, card = null) {
  const anchor = pos?.anchor;
  if (!ANCHORS.includes(anchor)) return { x: Number(pos?.x) || 0, y: Number(pos?.y) || 0 };
  const canvasW = widgetsCanvas.clientWidth || window.innerWidth || 1920;
  const canvasH = widgetsCanvas.clientHeight || window.innerHeight || 1080;
  const w = Number(pos.w) || card?.offsetWidth || 0;
  const h = Number(pos.h) || card?.offsetHeight || 0;
  return {
    x: anchor[1] === "r" ? Math.max(0, canvasW - w - (Number(pos.ax) || 0)) : (Number(pos.ax) || 0),
    y: anchor[0] === "b" ? Math.max(0, canvasH - h - (Number(pos.ay) || 0)) : (Number(pos.ay) || 0),
  };
}
```

An anchor such as `br` means "this distance from the bottom-right corner", so the card keeps that distance when the screen changes. The function reads the canvas size and writes nothing, so a transient size during sleep never reaches the saved data. See [desk.positions](../reference/desk.md#r-desk.positions).

When the screen changes, `relayoutWidgets` moves the cards that already exist. It changes only `left` and `top`, so each card keeps its element, its timers, and the user's input.

Filename: core/ui/unidtk-core.js (abridged)

```js
function relayoutWidgets() {
  widgetsCanvas.querySelectorAll(".widget-card").forEach((card) => {
    if (card._geometryController) { card._geometryController.relayout(); return; }
    const at = placeOnCanvas(config.positions[card.dataset.id], card);
    card.style.left = at.x + "px";
    card.style.top = at.y + "px";
  });
  notifyGeometryChanged("relayout");
}
```

Four events reach it through `handleDisplayOrResizeChange`: the kernel's `display-changed`, the window `resize`, the visual viewport resize, and the screen change. That handler waits 150 milliseconds, so a burst of events causes one relayout. A change in the widget list goes through `syncWidgets` instead, which removes the cards no longer wanted, builds the missing ones, and then calls `relayoutWidgets`. The menu calls it when a module is shown or hidden.

Warning: `disposeWidgetCard` runs when a card is removed, and it cleans up a fixed list of things: the clock's morph, the geometry controller, and the music player's loops. It knows nothing about a timer that a new module starts. A module's timer must stop itself, by checking `card.isConnected` on each tick.

## 6.8 Calling the kernel: the bridge

A page never calls the kernel directly. It calls `invoke(command, args)` and `listen(event, handler)` from `core/ui/unidtk-bridge.js`. That file is the only page file allowed to import Tauri, and the build lint `lint_ui_imports` enforces it.

Filename: core/ui/unidtk-bridge.js (abridged)

```js
const TRANSPORT = new URLSearchParams(window.location.search).get("transport")
  || window.sessionStorage.getItem("unidtk.transport") || "tauri";

export async function invoke(command, args) {
  const wantsWs = TRANSPORT === "ws" || TRANSPORT === "shadow";
  if (!WS_COMMANDS.includes(command) || !wantsWs) {
    return tauriInvoke(command, args);
  }
  // ... shadow and ws branches ...
}
```

The transport is chosen once per page load, and the default is `tauri`. In `ws` and `shadow` modes, the commands listed in `WS_COMMANDS` travel over the local WebSocket instead. A command enters that list only after a shadow run, where both paths are called and their answers compared. The WebSocket door is explained in the `udtk` book, in [the WebSocket door](../udtk/ch04-the-websocket-door.md). `listen` follows the same rule for events. The bridge learns the socket's port and token from one `session_info` call, and it keeps the token in memory only.

Here is how `glyph-bar` uses the bridge. It asks once, and then follows pushed events.

Filename: modules/glyph-bar/ui/widget.js (abridged)

```js
import { invoke, listen } from '@unidtk/bridge';

async function refreshSystemVolume() {
  const v = await invoke("get_system_volume");
  systemVolumeState = { level: v.level, muted: !!v.muted };
}

listen("system-volume", (event) => {
  systemVolumeState = { level: event.payload.level, muted: !!event.payload.muted };
});
```

The payload is the value the kernel's `emit` sent, unchanged. The event name is the contract between the module and the page, the same contract that [Chapter 4](ch04-modules.md) describes.

## 6.9 Summary

The page is built from `core/ui` and from each module's `ui/` folder, and it is served from one folder. `index.html` paints the last theme, loads the stylesheets, and names the bridge. `unidtk-core.js` runs `loadWidgets` once: it reads each module's `metadata.json`, builds one card per widget, and mounts the module's renderer through a fixed branch on `specialType`. After startup, cards move through `relayoutWidgets`, which changes positions and keeps the elements. Every call to the kernel goes through `unidtk-bridge.js`. The next chapter builds a whole module, from the Rust crate to the card on the desk. Continue with [Chapter 7. Project: Building a Module](ch07-project-a-module.md).
