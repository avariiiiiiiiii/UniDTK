# Motion: Backlog and the Original Plan (History)

This record is frozen. It is what the motion work planned and found at the time; the current rules are in [Motion](../../reference/motion.md).

## 11. Recorded Backlog (History, With Corrections)

The old section was headed "Recorded Backlog and Polish Items (Backlog, 100% Complete)" and recorded eight polish items as resolved. The identifiers below exist in the tree. The numeric values were not re-measured for this document.

1. **Player popup speed.** `openMusicPopup` (`modules/mp3-player/ui/widget.js`, line 197) calls `openSurface` with `duration: "motion"`. The older text said `"layout"`. The content offset is `0.45` in `surface-manager.js` (line 308). It applies to the **close** only. On open the content uses the full transition duration.
2. **Closing popups by the header.** `closePopupElement` and the `.music-popup-header` style both exist in `modules/mp3-player/ui/`.
3. **VPN server list layout.** `_finalizeClose` restores `record.prevDisplay || ""` (line 812). The `.vpn-server-menu` rule exists in `core/ui/styles.css` and `modules/vpn/ui/vpn.css`.
4. **Telegram widget opening.** The rule of one geometry pass with a WAAPI `clipPath` morph is the same mechanism as section 7. The old text's `400ms` removal is not separately re-checked here.
5. **Telegram account dropdown.** `.tg-account-dropdown` and `tg-account-dropdown` exist in `modules/telegram/ui/`.
6. **Internal transitions.** Explorer's `scale(0.98)` effects and the Explorer marquee selection were described. They were not re-verified.
7. **Desktop click to close folders.** The handler is `setupDesktopOutsideClickHandler()` in `core/ui/unidtk-core.js`. The older text named `shell-core.js`, which does not exist in this tree.
8. **Widget mutual exclusion.** `collapseSingleWidget`, `expandSequence` and `_collapseTimer` exist in `core/ui/unidtk-core.js` and the Telegram widget. The old text recorded the timer as `320ms`; that value was not re-checked.

---

## 13. The Engineering Plan (Historical)

This plan was the order in which the work was done. It is kept for reference, and each stage is already reflected in the code above.

### Stage 1. Motion Core and Dynamic Timing Math (`motion.js`)

1. Implemented in `core/ui/motion.js`:
   - The function `calculateDuration(width, height, originRect)` (the code takes four numbers, or an origin object), based on the logarithmic formula above.
   - The full set of tokens (EASINGS in `core/ui/motion.js`): `--ease-standard` `(0.4,0,0.2,1)`, `--ease-spring` `(0.34,1.56,0.64,1)`, `--ease-expo`/`--ease-expo-out` `(0.30,0.95,0.35,1.00)`, `--ease-expo-out-rev`/`--ease-surface-exit` `(0.38,0.05,0.45,1.00)`, `--ease-snap` `(0.18,1.04,0.22,1)`, `--ease-content` `(0.25,1,0.5,1)`, `--ease-smooth` `(0.25,0.1,0.25,1)`, `--ease-item-in` `(0.16,1,0.3,1)`.
   - Support for seamless `reverse()` in the Web Animations API. The timeline class provides `reverse()`, but the surface manager does not use it (section 8.3).

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
