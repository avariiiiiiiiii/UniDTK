# Unified Core Modules Motion (unicoremodmotion.md)

> **Document status:** Official architecture standard of the UniDTK core.  
> **Scope:** The animation engine (`motion.js`), the unified surface manager (`surface-manager.js`), all widgets, popups, modals, overlays and context menus.

---

## 1. The Fundamental Rule of Two Object Types

The whole UniDTK graphics subsystem is split into **exactly two types of objects**. Hybrids and ad-hoc popup blocks are not allowed.

```
                     ┌────────────────────────────────────────┐
                     │             UniDTK Engine              │
                     └───────────────────┬────────────────────┘
                                         │
                 ┌───────────────────────┴───────────────────────┐
                 ▼                                               ▼
    ┌─────────────────────────┐                     ┌─────────────────────────┐
    │    1. WIDGETS           │                     │    2. SURFACES          │
    │     (WIDGETS)           │                     │     (SURFACES)          │
    ├─────────────────────────┤                     ├─────────────────────────┤
    │ • Always on the desktop │                     │ • Open on click         │
    │ • Store coordinates     │                     │ • Morph from hitbox     │
    │ • Drag & Drop / Resize  │                     │ • LIFO close stack      │
    │ (Player, Clock, Folders)│                     │ (Explorer, popups, TG)  │
    └─────────────────────────┘                     └─────────────────────────┘
```

### Type 1. Desktop Widget (`Widget`)
- **Definition:** A persistent card that is always present on the `widgetsCanvas` surface.
- **Examples:** The round audio player (`mp3-player`), the clock (`clock`), the Telegram dock (`telegram`), the glyph bar (`glyph-bar`). The names used in early versions of the docs (`yt-music`, `games`/`notes`/`launcher`/`network`/`tools`) are outdated. The current modules are `clock`, `explorer`, `glyph-bar`, `mp3-player`, `telegram`, and `vpn`.
- **Properties:** A widget keeps its coordinates in `config.positions`. It supports dragging (`setupDragReorder`) and recalculation when screens change (`relayoutWidgets`). It is never reset or removed from the DOM when other windows close.

### Type 2. Surface / Window (`Surface`)
- **Definition:** Any window, panel, modal, dialog, or menu that opens in response to a user action.
- **Examples:**
  - The Nothing Explorer file manager window (`#explorerOverlay`);
  - The audio player popups: the library (`.music-library-popup`) and the settings (`.music-settings-popup`);
  - The VPN server panel in the clock (`.vpn-server-menu`);
  - Telegram modals (forwarding messages, sending files, the photo lightbox);
  - Right-click context menus (in Explorer and in chats).
- **Properties:** Surfaces are controlled **only** through the single core contract `SurfaceManager`. They are mounted into the single coordinate-safe portal `#shell-portals`, use the same hardware border morph, and are registered on a LIFO stack.

---

## 2. Hard Motion Guardrails

### Rule 2.1: Full Ban on Hardcoded Milliseconds
- **Writing fixed animation timings in CSS and JS is strictly forbidden** (`140ms`, `220ms`, `340ms`, `400ms`, `setTimeout(..., 220)`).
- The animation duration $T$ is **computed at runtime** by the function `calculateDuration(targetW, targetH, originW, originH)` in `motion.js`. It is based on the physical size, or on the expansion delta of the border ($\Delta d = \sqrt{\Delta W^2 + \Delta H^2}$):

$$T(\Delta d) = \left[ T_{\min} + (T_{\max} - T_{\min}) \cdot \frac{\ln\left(1 + \frac{\Delta d}{d_0}\right)}{\ln\left(1 + \frac{d_{\max}}{d_0}\right)} \right] \times \text{currentScale}$$

Where:
- $T_{\min} = 240\text{ ms}$ (micro menus, context lists);
- $T_{\max} = 440\text{ ms}$ (full-screen Explorer);
- $d_0 = 100\text{ px}$, $d_{\max} = 1600\text{ px}$;
- $\text{currentScale}$ is the global speed multiplier set by `setMotionScale()`.

Because the term is a logarithm, the duration rises quickly for small changes and flattens out for large ones.

### Rule 2.2: Hardware Border Morph from the Hitbox (Origin Clip-Morphing)
- **Scaling content (`scale`) is forbidden:** window content (text, Ndot/Inter fonts, icons, covers) must **never be scaled with `transform: scale()`**. Scaling fonts blurs DirectWrite subpixel rendering in WebView2.
- **Morphing the border:** The window is mounted immediately at its final pixel size (one layout pass, no reflow during the animation). The container's border is expanded on the GPU through `clip-path: inset(top right bottom left round radius)`:
  - $t = 0$: the mask is compressed to the exact coordinates and corner radius of the trigger button or hitbox. For example, it matches the `zone-top` sector of the player with a radius of $R=75\text{px}$, following the vinyl circle exactly.
  - $t = 1$: the mask expands exponentially to `inset(0 0 0 0 round 24px)`.
- **Content fade-in:** The content fades in with `opacity: 0 -> 1` and moves $\Delta y = 6\text{–}8\text{ px}$ when the border movement is 65% complete.

### Rule 2.3: Seamless Animation Reversal
- `setTimeout` must not be used to finish animations.
- If the user presses `Escape` or clicks outside the window while it is opening (for example, 60 ms into a 260 ms animation), the animation **does not freeze or jerk**. The code calls `animation.reverse()`, and the window closes smoothly back into the hitbox from that same intermediate point, in the same 60 ms.

### Rule 2.4: A Single LIFO Close Stack (Last In, First Out)
- Every open surface is registered in the core array `SurfaceManager.activeStack`.
- The `Escape` key and clicks outside a window are handled by one global core listener.
- Pressing `Escape` closes **exactly one surface, the topmost one**:
  $$\text{Context menu} \xrightarrow{\text{Esc}} \text{Modal} \xrightarrow{\text{Esc}} \text{Explorer} \xrightarrow{\text{Esc}} \text{Collapse widgets}$$
- Background windows and widgets must not be closed while a top-level modal is open.

### Rule 2.5: Protecting Widget Coordinates and Integrity
- No operation on surfaces or animations may, under any circumstances, reset `config.positions` or perform a destructive DOM reset (`innerHTML = ""`) on `widgetsCanvas`.

---

## 3. Step-by-Step Engineering Plan

### Stage 1. Motion Core and Dynamic Timing Math (`motion.js`)
1. Implemented in `core/ui/motion.js`:
   - The function `calculateDuration(width, height, originRect)`, based on the logarithmic formula above.
   - The full set of tokens (EASINGS in `core/ui/motion.js`): `--ease-standard` `(0.4,0,0.2,1)`, `--ease-spring` `(0.34,1.56,0.64,1)`, `--ease-expo`/`--ease-expo-out` `(0.30,0.95,0.35,1.00)`, `--ease-expo-out-rev`/`--ease-surface-exit` `(0.38,0.05,0.45,1.00)`, `--ease-snap` `(0.18,1.04,0.22,1)`, `--ease-content` `(0.25,1,0.5,1)`, `--ease-smooth` `(0.25,0.1,0.25,1)`, `--ease-item-in` `(0.16,1,0.3,1)`.
   - Support for seamless `reverse()` in the Web Animations API.

### Stage 2. The Core Surface Dispatcher (`surface-manager.js`)
1. Created in `core/ui/surface-manager.js`:
   - The LIFO stack of open windows (`activeStack`);
   - A single `Escape` handler in the capture phase, which guarantees that exactly one top window closes;
   - A single `pointerdown` handler for closing on a click outside;
   - The function `openSurface(element, options)`: calculates the hitbox coordinates, starts the hardware `clip-path` border morph, and fades in the content with a delay;
   - The function `closeSurface(element)`: smoothly reverses back into the hitbox;
   - The canonical layer scale `--z-surface-*`.

### Stage 3. Moving the Player Popups (Library and Settings)
1. Connect the player popups (`.music-library-popup` and `.music-settings-popup`) to `surface-manager.js`:
   - Clicking `zone-top` expands the border from the top sector of the circle ($R=75\text{px} \to 24\text{px}$);
   - Clicking `zone-bottom` expands it from the bottom touch zone;
   - The playlist content fades in smoothly, without distorting covers or text;
   - Pressing `Escape` in the middle of the opening animation reverses it instantly back into the hitbox.

### Stage 4. Connecting Explorer, Telegram Modals, and Context Menus
1. Move the full-screen Explorer overlay (`#explorerOverlay`) to registration in `surface-manager.js`:
   - Replace the old `scale(0.92) + translateY(12px)` with hardware border expansion using dynamic timing;
   - Explorer and Telegram context menus are registered as top-layer surfaces and are closed first by `Escape`.
2. Remove the duplicate click handlers in `vpn-panel.js` and fix the memory leaks.

### Stage 5. End-to-End Verification and Measurements
1. Start the application and run an end-to-end measurement through the CLI bridge (`udtk`): `udtk doctor` (including `-simulate`), `udtk logs --level ERROR`, `udtk shot`.
2. Start UniDTK through Task Scheduler `UniDTK` (`schtasks /run /tn UniDTK`) or `udtk run`, and measure:
   - `udtk doctor --quick`: checks the integrity of DLLs, IPC, and tasks;
   - `udtk logs --level ERROR`: checks that the error count is zero;
   - `udtk shot`: a required visual audit in the collapsed, half-open, and fully open states.
3. Record the session results in `docs/architecture.md`.

---

## 4. Recorded Backlog and Polish Items (Backlog, 100% Complete)

All 8 items were implemented, tested, and verified at runtime in WebView2:

1. **Player popup speed calibration** (resolved):
   - `openMusicPopup` (`modules/mp3-player/ui/widget.js`) now explicitly passes the token `duration: "layout"` (340ms per the `DURATIONS` canon). This keeps the expand time in sync with the physical folders on the desktop.
   - The `content` offset in `surface-manager.js` was softened from `0.65` to `0.45` of the total duration. This prevents text and covers from appearing suddenly in the last milliseconds.
2. **Closing player popups by clicking the header** (resolved):
   - The whole `.music-popup-header` now has a delegated click handler that calls `closePopupElement(popup)`.
   - Clicks on interactive controls inside the header are filtered out (`e.target.closest("button, input, select, a, [data-prevent-close]")`), except for the close button `.music-popup-close`.
   - `cursor: pointer` and `user-select: none` were added to the styles.
3. **VPN server list layout in the clock widget** (resolved):
   - Root cause: `_finalizeClose` in `surface-manager.js` saved `element.style.display = record.prevDisplay || "none"`. When the inline style was initially empty, the element got `"none"`, and on the next open the fallback `display = "flex"` ran.
   - `_finalizeClose` now saves an empty string `""`. The `.vpn-server-menu` CSS fixes `display: flex; flex-direction: column;`, and `vpn-panel.js` passes `display: "flex"` explicitly.
4. **Telegram widget opening performance** (resolved):
   - The 400ms parallel CSS geometry transition, which caused layout thrashing, was removed.
   - A one-pass layout was implemented: the card is set to its target size immediately, and the borders expand with the hardware WAAPI GPU morph `clipPath: inset(...)`.
   - `overflow: hidden !important;` was added to the card, so text and interface elements stay hidden behind the widget border until the expansion finishes.
   - `onfinish` and `oncancel` fully clear `clipPath = ""`, to prevent blurring of DirectWrite ClearType subpixel antialiasing in WebView2.
5. **Telegram account list animations (`.tg-account-dropdown`)** (resolved):
   - The dropdown was moved to `surfaceManager.openSurface(dropdown, ...)` with the tokens `origin: accountBtn`, `originRadius: "8px"`, `targetRadius: "12px"`, `display: "flex"`.
   - The custom `setTimeout(180)` calls were removed, along with the `scale(0.96)` transforms that blurred the image, and the click listeners leaking on `document`.
6. **Animations for internal transitions and selection** (resolved):
   - In Explorer, the blurring scale effects `scale(0.98)` / `scale(0.96)` were removed from `.file-area.transitioning` and `@keyframes fileAppear`. The artificial 2-rAF delay during navigation was removed too.
   - In `telegramSetActiveChat`, a hardware micro-fade of the `.tg-messages` list was added, without layout jank.
   - In Explorer, a hardware frame for group selection (Marquee Selection) was implemented. It precomputes the AABB when the movement starts (cached for 120 FPS), has a 4px hysteresis, compensates for scrolling, and clears the selection on a click on the background.
7. **Closing widget folders by clicking empty desktop space** (resolved):
   - A global listener, `setupDesktopOutsideClickHandler()`, was added in `shell-core.js` with a mouse-movement threshold of `< 6px`. This dead zone prevents accidental closing while dragging.
   - A LIFO priority check was added: if a `surfaceManager` context menu or modal is open, the desktop does not handle the click.
8. **Widget Mutual Exclusion rule** (resolved):
   - An invisible phantom block of $400\times 400\text{px}$ was fixed. `collapseSingleWidget` and `collapseActiveDesktopWidget` now force `width` and `height` back to compact sizes (`pos.w || 340`, `pos.h || 64`).
   - The unreliable `setTimeout(400)` timers were replaced with cancellable `card._collapseTimer` handles (320ms).
   - A monotonic counter, `expandSequence`, was added to guard against async races in icon rendering.
   - `relayoutWidgets()` now protects the active expanded widget from having its coordinates overwritten during a screen resize.
