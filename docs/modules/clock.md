# The Clock Module: One Widget, Twelve Faces

This chapter explains the `clock` module step by step. The module has no Rust backend. It is one JavaScript widget that draws a clock in one of twelve styles, a gooey face built from SVG glyphs, and a dashboard with a calendar that opens when the card expands. You will see how the widget starts, how one shared one-second interval drives every clock on the desk, how the chosen style is stored, and where the module fails.

Every mechanism here is taken from the code. Line numbers refer to the files as they are in this repository.

## What you will understand by the end

- Why the clock needs no Rust code, and what the module contract still asks of it.
- How one `setInterval` updates every clock face in the document, and why it skips hidden tabs.
- How a style is chosen, stored in the desk configuration, and cycled.
- How the gooey face redraws only the digits that changed.
- Which parts of the expanded dashboard are live, and which are read once.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/clock/ui/widget.js](../../modules/clock/ui/widget.js) | Renders every style, runs the tick, cycles styles, builds the dashboard (299 lines). |
| [modules/clock/ui/gooey.js](../../modules/clock/ui/gooey.js) | The gooey face: glyph paths, four digit slots, date line (70 lines). |
| [modules/clock/ui/clock.css](../../modules/clock/ui/clock.css) | Styles for the classic faces, the switcher, and the dashboard (293 lines). |
| [modules/clock/ui/gooey.css](../../modules/clock/ui/gooey.css) | Styles for the gooey face, scoped to its container (52 lines). |
| [modules/clock/ui/metadata.json](../../modules/clock/ui/metadata.json) | Name, `specialType`, tint slot, and `container` flag. |
| [modules/clock/tests/runtime.js](../../modules/clock/tests/runtime.js) and [runtime.ps1](../../modules/clock/tests/runtime.ps1) | Live checks of the expand and collapse animation in the installed app. |
| [modules/clock/tests/ui.js](../../modules/clock/tests/ui.js) and [ui.ps1](../../modules/clock/tests/ui.ps1) | The same checks, run against a fixture built from the core's `unidtk-core.js`. |

The core loads the module into the page. [core/ui/index.html](../../core/ui/index.html) links the two stylesheets, and [core/ui/unidtk-core.js](../../core/ui/unidtk-core.js) imports `widget.js` when a clock card is built. The expand animation that moves a clock into its dashboard is not in this module: `animateClockMorph` and `finishClockMorph` are in `unidtk-core.js`.

The module is a vertical slice, so everything it draws lives under `modules/clock/` ([modules.vertical-slice](../reference/modules.md#r-modules.vertical-slice)). It has no backend, so its only state is the desk configuration ([modules.states](../reference/modules.md#r-modules.states)).

---

## Step 1: No backend, only a metadata file and a widget

The module has `ui/` and `tests/` folders and no `rust/` folder. Its metadata names the widget and declares its theme share:

```json
// Filename: modules/clock/ui/metadata.json (abridged)
{
  "name": "Часы",
  "specialType": "clock",
  "simulate": { "module": "widget.js", "function": "renderClock", "sandbox": "..." },
  "tints": [ { "id": "clock", "label": "Часы", "default": "lilac", "role": "calm" } ],
  "container": true
}
```

- `specialType: "clock"` is the key the core uses. `buildWidgetCard` in `unidtk-core.js` (around line 587) imports `modules/clock/widget.js` for it.
- `tints` declares one colour slot, `clock`, with `lilac` as its default ([theme.module-contracts](../reference/desk.md#r-theme.module-contracts)).
- `container: true` lets other widgets be nested inside a clock card ([desk.nesting](../reference/desk.md#r-desk.nesting)). `widgets-api.js` uses this to move the VPN panel into the clock's nested list.
- `simulate` names the file and function a preview would call. The reader of this field is outside the module, and this chapter does not describe it.

---

## Step 2: Startup: one interval for all faces

Each clock card is rendered on its own. The shared timer is started once, after the core has placed the cards. The widget-loading routine in `unidtk-core.js` (around line 817) calls `_startWidgetLoops()`:

```js
// Filename: core/ui/unidtk-core.js (lines 856-861)
async function _startWidgetLoops() {
  const clock = await import("../modules/clock/widget.js");
  clock.startClockInterval();
  // ... audio-arcs loop follows ...
}
```

`startClockInterval` in `widget.js` owns the timer:

```js
// Filename: modules/clock/ui/widget.js (lines 167-174)
function startClockInterval() {
  if (window.clockInterval) clearInterval(window.clockInterval);
  window.clockInterval = setInterval(() => {
    if (document.hidden) return;
    updateClockTime();
  }, 1000);
  updateClockTime();
}
```

- The timer is kept on `window.clockInterval`. A second call clears the first, so two timers never run.
- The callback returns at once while the document is hidden, so the clocks do no work when the WebView is not visible.
- It updates once immediately, so the faces do not wait a second for their first value.

---

## Step 3: Rendering a card

`renderClock` marks the card, reads the stored style, and builds the switcher button and the face container:

```js
// Filename: modules/clock/ui/widget.js (lines 7-11, abridged)
function renderClock(card, widgetId) {
  card.classList.add("clock-widget");
  card.classList.add("no-header");
  const pos = config.positions[widgetId] || (config.positions[widgetId] = {});
  const version = pos.version || "digital";
  // ... container and switcher created here, then ...
  updateClockFace(container, version);
}
```

- `config.positions[widgetId]` is the desk record for this card ([desk.positions](../reference/desk.md#r-desk.positions)). If it is missing, the code creates it.
- A card with no stored `version` shows `digital`.
- `updateClockFace` clears the container, toggles a class for four of the styles (`gothic`, `grid-second`, `nothing-watch-pro`, `gooey`), writes the markup for the chosen style, and calls `updateClockTime()` so the new face shows the current time at once.

`updateClockFace` is a chain of `if (version === ...)` branches, each writing a fixed template. Thirteen names are recognised, and the switcher cycles through all thirteen. The gooey branch does not write markup itself: it calls `renderGooeyClock` in `gooey.js` (Step 6).

---

## Step 4: Switching style and what is stored

The switcher is a small button in the top-right corner. It is hidden until the desk is in edit mode: `clock.css` sets its opacity to 0 and `pointer-events` to `none`, and `body.edit-mode .version-switcher` shows it.

Clicking it advances through a fixed list of twelve names, written in `widget.js` (line 28):

```js
// Filename: modules/clock/ui/widget.js (lines 29-36, abridged)
const curIdx = versions.indexOf(container.dataset.version);
const nextIdx = (curIdx + 1) % versions.length;
const nextVer = versions[nextIdx];
container.dataset.version = nextVer;
config.positions[widgetId].version = nextVer;
saveConfig();
updateClockFace(container, nextVer);
notifyGeometryChanged("dock-content");
```

- An unknown current name gives index -1, so the next name is `digital`.
- The new name is written to `config.positions[widgetId].version`, and `saveConfig()` persists it. That is the module's only stored state. It keeps no file or database entry of its own.

Warning: nothing validates `version` on load. A name that no branch recognises leaves the container empty, because `updateClockFace` clears it first. The switcher still recovers the card, since its index lookup falls back to `digital`.

---

## Step 5: Each tick: `updateClockTime`

Once per second `updateClockTime` formats one `Date` and writes the result into every matching element in the document. It holds no references to individual cards; it queries the document each time:

```js
// Filename: modules/clock/ui/widget.js (lines 176-181, abridged)
function updateClockTime() {
  const now = new Date();
  const h = now.getHours();
  const m = now.getMinutes();
  const s = now.getSeconds();
  const timeStr = `${h.toString().padStart(2, '0')}:${m.toString().padStart(2, '0')}`;
  // ... one querySelectorAll block per face class follows ...
}
```

Digital faces get `timeStr`. Stacked and vertical faces split the parts into their own nodes. Rings rotate by `m * 6` and `s * 6` degrees, and analogue hands use `hDeg = (h % 12) * 30 + m * 0.5`, `mDeg = m * 6` and `sDeg = s * 6`.

Because the function writes to every match, the dashboard copy of a clock (Step 7) is updated by the same tick.

Note: `nothing-watch-pro` is the one digital face that shows seconds. Its branch writes `HH:MM:SS`. The class toggle in `updateClockFace` exists so that this style does not leave a stale class behind when the card switches to another face.

---

## Step 6: The gooey face: fixed slots, changed ink only

The gooey face is the only style that draws digits as SVG paths. `gooey.js` opens with the reason: the glyphs are editable geometry, with no font, bitmap or filter, and every digit has the same advance, so a change of minute cannot move the rest of the face.

`renderGooeyClock` builds the frame once, with four slots placed 70 units apart:

```js
// Filename: modules/clock/ui/gooey.js (lines 33-40, abridged)
const svg = container.querySelector('.gooey-clock-digits');
for (let index = 0; index < 4; index += 1) {
  const slot = document.createElementNS(SVG_NS, 'g');
  slot.setAttribute('transform', `translate(${index * 70} 0)`);
  slot.classList.add('gooey-clock-slot');
  svg.appendChild(slot);
}
```

`updateGooeyClock` runs on every tick, and does the least work it can:

```js
// Filename: modules/clock/ui/gooey.js (lines 57-66)
if (face.dataset.time === digits && !dateChanged) return;
const slots = face.querySelectorAll('.gooey-clock-slot');
for (let index = 0; index < slots.length; index += 1) {
  const slot = slots[index];
  if (slot.dataset.digit === digits[index]) continue;
  slot.innerHTML = `<g class="gooey-clock-ink">${digitMarkup(digits[index])}</g>`;
  slot.dataset.digit = digits[index];
}
face.dataset.time = digits;
```

- The face remembers the last `HHMM` in `data-time`. If nothing changed, the function returns.
- Each slot remembers its digit in `data-digit`. Only a slot whose digit changed gets a new ink group.
- The date line is rewritten only when the day changes, tracked by `data-date`.
- The new ink plays a settle animation that uses `var(--dur-transient)` and `var(--ease-standard)` from `gooey.css`, the shared motion tokens ([motion.no-hardcoded-ms](../reference/motion.md#r-motion.no-hardcoded-ms)).

Digit shapes come from fixed tables in the file: zero and one are filled paths, and two to nine are stroked paths with round caps. Nothing is loaded at run time.

---

## Step 7: The expanded dashboard

When a clock card expands, the core calls `renderClockDashboard(body, widgetId)` (`unidtk-core.js`, line 1218). The function builds two columns:

- On the left, a clock container with class `dash-clock`, rendered by the same `updateClockFace` with the stored style.
- On the right, a month calendar built from `new Date()` when the dashboard opens, with today marked by the `today` class, and a network line.

The calendar is not refreshed by a timer. The network line reads `navigator.onLine` once and writes "В сети" or "Нет сети". `widget.js` has no `online` or `offline` listener, so the line does not change while the dashboard stays open. The dashboard's clock copy, by contrast, is updated by the tick in Step 5.

---

## Step 8: How it fails

| Failure | What the code does | What the user sees |
| :--- | :--- | :--- |
| `widget.js` fails to load | The core logs the error and calls `showDebugMessage` with the clock id (`unidtk-core.js`, lines 593-596). | A debug message; the card stays empty. |
| The stored `version` is unknown | `updateClockFace` clears the container and writes nothing. | An empty card. The switcher recovers it. |
| The interval fails to start | `_startWidgetLoops()` is called with `.catch(() => {})`, so the error is swallowed. | The faces keep their first value. Nothing is logged. |
| The tab is hidden | The tick returns at once. | Nothing; the faces catch up within a second of the tab coming back. |
| The network changes while the dashboard is open | `navigator.onLine` was read once when the dashboard rendered. | The network line is stale until the dashboard is rebuilt. |

The module has no network, no disk and no backend of its own, so this table covers its own failures. Failures in the core that hosts it are outside this chapter.

---

## Step 9: Tests

The test folders check the expand and collapse animation, not the time display. [runtime.ps1](../../modules/clock/tests/runtime.ps1) passes `runtime.js` to `udtk gui js:` through `target/release/udtk.exe`, so it runs inside the installed app. The script opens and closes a real clock card for seven styles, and checks that the morph clone hands over to the source with matching geometry. It restores the saved style when it finishes.

[ui.ps1](../../modules/clock/tests/ui.ps1) runs the same checks against a fixture. It reads the section of `unidtk-core.js` from `rectSnapshot(` to `toggleWidget(`, prepends a line that takes `motionTimeline` and `EASINGS` from `deps`, and runs the result in the page with the module and motion modules passed in. Both scripts print `passed` and `total`, and fail unless the two numbers are equal.

---

## Known gaps

- **Six of the thirteen cycled faces have no morph test.** The runtime and UI checks cover `rings`, `digital`, `analogue`, `grid-second`, `gooey`, `nothing-watch-pro` and `gothic`. `digital-stacked`, `digital-vertical`, `analogue-dot`, `analogue-bezel`, `analogue-cmf` and `minimal-lines` are not in either list.
- **Hardcoded colours and a keyword easing.** `clock.css` writes `#333` (lines 133 and 154), `rgba(170, 170, 170, 0.8)` (line 121) and `#ff6e00` (lines 189 and 193). `.version-switcher` uses the keyword `ease` at line 98. These break [theme.tokens-only](../reference/desk.md#r-theme.tokens-only).
- **Placeholder content.** The `analogue-dot` face shows the literal text "СБ 23 / 156 ккал / 6 240 шагов" (widget.js, lines 98-102), and `nothing-watch-pro` shows "CMF BY NOTHING" (line 126). No data source feeds either.
- **The interval failure is silent.** `_startWidgetLoops().catch(() => {})` hides a failed import of `widget.js`, so the faces freeze with no message (Step 8).
- **The network line does not update.** It is read once and has no listener (Step 7).
- **Locale is fixed to Russian.** Dates use `ru-RU` in `widget.js` and `gooey.js`. The calendar header uses `'default'` for the month name, so the two parts of the dashboard use different locales.
- **A comment is mis-encoded.** The comment above the dashboard function (widget.js, line 248) holds box-drawing characters saved as UTF-8 and read as Windows-1251, so it is unreadable as written.
