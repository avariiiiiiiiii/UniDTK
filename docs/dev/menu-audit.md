---
title: Customization menu audit (UX → implementation)
type: task
project: unidtk
updated_at: "2026-10-28"
status: open
---

# Customization menu audit

> Problems only, no solutions. Sources: a live run of the installed build `f16cedd` (screenshots `udtk shot`, `udtk gui tree`, `udtk gui js`) and a code read by 4 subagents (Haiku 5.5) with selective manual verification. The tag **[live]** means reproduced at runtime, **[code]** means checked by reading the code by the orchestrator agent, and no tag means a subagent finding that was not re-verified manually.

## 1. Data loss and broken behavior

> **Session 130:** items 1–5, 8–13 and 14 fixed (shortcut targets; old directories not deleted). 6 is a false finding (dead code, removed). 7: `saveConfig` now reports a failed save. The remaining sections are open (UX, CSS).

1. **[live]** Clicking a pill for font, material, micrographics layout, or «Shuffle/Restore» resets the «Motion transition profile». The `.theme-motion-pill` handler is attached to all of these buttons, their `dataset.profile` is empty → `setLiveToken("motionProfile", undefined)` → the profile falls back to «Balanced». Reproduced: «Smooth» → click on the already active «Sea Glass» → «Balanced». `core/ui/menu.js:1399-1406`, classes on `1100, 1115, 1197-1213, 1221-1222`.
2. **[code]** Almost any action in the menu (widget visibility, X/Y, position reset, creating/renaming/deleting a folder, adding/removing a shortcut) calls `loadWidgets()` → `widgetsCanvas.innerHTML = ""` and re-reads the config: the player, Telegram and the clock are recreated. `menu.js:297, 361, 533, 550, 562, 603, 622`; `unidtk-core.js:366`.
3. **[code]** Two independent write queues for one `config` blob: the theme goes through `saveChain` with a 600 ms debounce, while the menu calls `saveWidgetConfig(config)` directly, without `syncThemeIntoConfig`. The commit order is not guaranteed, so an old theme may end up in redb. `menu.js:296, 332, 360`; `theme.js:1107-1136`; `unidtk-core.js:123-129`.
4. **[code]** There is no flush on close (`beforeunload`/`pagehide` are absent): a theme change made in the last 600 ms before exit is lost. `theme.js:1124-1136`.
5. **[code]** A `vpn_settings` error silently draws the form with empty defaults; «Save subscription» / «Save server» then overwrite the real settings with empty ones. The error appears only in the console. `menu.js:642-661, 774-790, 808-835`.
6. **[code]** After a folder card is collapsed, its icon disappears: the body is cleared, and restoration reads `config.folders` (filled by no one) and `window.__renderFolderIcon` (defined nowhere). `unidtk-core.js:664-668, 1126-1130`.
7. If the config source is not `redb` (a `get_widget_config` failure, broken JSON), `saveWidgetConfig` silently writes nothing for the whole session, and the menu shows success. `widgets-api.js:202-221`.
8. A race between two fast toggles of backend modules: the second overwrites the first; when `ok === false`, the checkbox stays in the wrong state. `menu.js:304-313`.
9. Buttons are not locked while `invoke` is running: a double click creates two folders / two shortcuts / two VPN requests, and the id `folder_${Date.now()}` may collide. `menu.js:518-538, 571-609, 774-835`.
10. `config.hidden` is mutated before saving, without try/catch: a failed hide «rides along» into redb with the next successful save. `menu.js:292-296`.
11. «Reset positions» writes glyph-bar and telegram `w:340, h:64`, which diverges from the defaults (`unidtk-core.js:138-139`, `widgets-api.js:168-171`); it clears `hidden` and does not touch `instances`. `menu.js:349-359`.
12. The default layout is defined twice and differently: `core/kernel/main.rs:2226-2240` (clock x:510) and `widgets-api.js:163` (x:28).
13. `relayoutWidgets()` calls `loadWidgets()` (a DOM reset) when the canvas is empty, which violates architecture rule §4. `unidtk-core.js:2201-2205`.

## 2. Data and storage

14. **[live]** Folder shortcuts point to `%LOCALAPPDATA%\Shell\folders\…\shortcuts\*.lnk`, a directory from the old project name. It lives next to `UniDTK` and contains `sing-box.exe` and `vpn-helper.exe` (programs in AppData, against the «program in Program Files» rule). Cleaning up `Shell` would break all shortcuts.
15. Four holders of theme state with no reconciliation at startup: localStorage, `config.theme` in redb, memory, and the `paint` snapshot in `index.html`. The comment «redb is authoritative» is wrong: an empty redb resurrects colors from the cache. `theme.js:1141-1190`, `index.html:14-19`.
16. The config blob has no version and no validation: `set_widget_config` accepts any JSON and replaces the whole key. `core/kernel/unidtk_db.rs:118-127`.
17. Migration of old presets silently fills in fields from Nothing Classic; the built-in Sea Glass of the old schema loses its palette, contrary to the comment. `theme.js:517-562, 36-38`.
18. «Shuffle» on Classic permanently overwrites the palette of the built-in preset. `theme.js:896-899`.
19. The live path of the sliders writes values without clamp/validation (`radiusSurface = val + 8` with no upper bound; the font is taken from `dataset` without checking). `theme.js:859-879`, `menu.js:1377, 1305`.
20. The slider bounds and the schema diverge: blur 36 vs 48, radius 28 vs 32.
21. Each slider `change` is a synchronous write of the whole JSON to localStorage plus the full blob to redb.

## 3. UX — the «Widgets and desktop» tab

22. **[live]** Each card has two toggles with no explanation: the upper one (show on the desktop) and «Module (backend)». The backend concept is exposed to the user. Clock has no second toggle, so the cards have different heights and the «Coordinates» row jumps.
23. **[live]** The position is raw X/Y in px (Clock Y = −3). There are no bounds, negative values are accepted, and an empty value becomes 0. There is no size at all.
24. **[live]** The «System module» labels on all cards are empty information; widget names are in English (Clock, Glyph Bar, Micrographics) in the Russian interface.
25. **[live]** The color emoji icon on «Move on desktop» is the only color icon in the menu.
26. «Move on desktop» silently closes the menu. Exiting edit mode is possible only by clicking the button on the desktop again; Escape does not work. `menu.js:338-343`, `unidtk-core.js:1903-1908, 2083-2095`.
27. Dragging has no screen bounds, grid, guides, overlap checks, or undo; `pointercancel` fixes the position. `unidtk-core.js:1781-1840`.
28. Positions are absolute CSS px with no binding to screen size; there is no clamping (except telegram, with magic 1200/1700). On another monitor, widgets end up off-screen. `unidtk-core.js:158-171, 418-421`.
29. Hiding by type hides all instances, and an instance cannot be restored separately. `unidtk-core.js:377-383`.

## 4. UX — the «Shortcut folders» tab

30. **[live]** The count label «(1 ярлыков)» is not pluralized correctly (the Russian form for one is «ярлык»). `menu.js:438`.
31. **[live]** The description «Data is stored in the redb database» is an implementation detail exposed to the user.
32. **[live]** The type «FILE» is in Latin uppercase; the placeholder «Имя ярлыка (YouTu» (shortcut name) is cut off; the path is truncated with an ellipsis.
33. **[live]** The folder name field looks like a heading, so it is not visible that it is editable. It is saved only on blur, with no label (`gui tree`: `name: null`).
34. A shortcut cannot be edited: it can only be deleted and created again. There is no reordering, no file drag & drop, and no file picking through a dialog.
35. No target validation: «Web (URL)» accepts `C:\…`, and «youtube.com» without a scheme goes to the file branch. Duplicates are not checked. `menu.js:571-609`, `widgets-api.js:289-296`.
36. Deleting a shortcut has no confirmation. Deleting a folder uses the native `confirm()`, and errors use the native `alert()`, which blocks and is not in the shell's style, while VPN uses toasts.
37. Deleting a folder does not clean up `positions`/`hidden`/`instances`. `menu.js:556-568`.
38. Renaming mutates the model before saving, without try/catch. An empty name leaves an empty field with no rollback. `menu.js:541-553`.

## 5. UX — the «VPN and Network» tab

39. **[live]** The subscription URL with its secret token is shown in plain text on the desktop.
40. **[live]** Jargon: «VLESS / Singbox», «Подмодуль» (submodule), «Short ID», «Public Key», «SNI (Server Name)».
41. VPN in the customization menu tab mixes purposes. The home server fields are not validated: the port `|| 443` swallows garbage, and UUID/CIDR are taken as-is. `menu.js:737, 808-823`.
42. The status «Отключено» (Disconnected) is shown on a `vpn_status` error and is not updated while the tab stays open. `menu.js:646, 663-681`.
43. Autoconnect: the save error appears only in the console, and the toggle stays on. `menu.js:793-799`.

## 6. UX — the «Appearance» tab

44. **[live]** One scroll of 1557 px with 489 px visible (3+ screens), ~50 controls, and no section navigation.
45. **[live]** Mixed languages: «(Glass Opacity)», «(Blur Strength)», «(Corner Radius)», «(Motion Duration Scale)», «(пресетный токен)» (preset token), «Виджет // Превью» (Widget // Preview), «Live», «UNIDTK // MENU».
46. **[live]** «Live preview — applied immediately», but the menu covers the whole desktop, so the effect cannot be seen on the widgets. The preview card is a fake white widget and is not recolored by the tint. «Уровень сигнала 64%» (Signal level 64%) is static, and «Aa» does not update.
47. **[live]** The accent chips (red, orange, cyan…) are not from the Sea Glass palette. The current accent `#4a5fc1` is not among the chips, so the selected one is not highlighted.
48. **[live]** Unselected pills are bare text with no border, so they do not read as buttons. «Перемешать цвета» (Shuffle colors) and «Вернуть прошлые» (Restore previous) look like plain text.
49. **[live]** The rhythm is broken: the labels «Микрографика» (Micrographics) and «Аватары Telegram» (Telegram avatars) stick to the controls above them.
50. **[live]** «Цвет 1…6» (Color 1…6) are meaningless names. The palette is squares and the widget tints are circles. «Панели: меню, прово…» (Panels: menu, prov…) is cut off.
51. **[live]** Section headings: Ndot in three tabs, Geist in «Appearance». Two fonts serve one role (`menu.css:230-239` vs `693-702`).
52. Irreversible actions have no confirmation and no undo: deleting a preset, «Reset sizes», and «Shuffle» (rollback is one step, in memory). There is no theme export, import, or full reset.
53. The whole tab is redrawn after almost every change, so focus, scroll position and an open color picker are lost. `menu.js:1276-1452`.
54. «Animation test» has no catch; repeated clicks pile up animations and timers. `menu.js:1488-1502`.

## 7. Common menu shell

55. **[live]** The hamburger in the header has no clear function. The title «UNIDTK // MENU» has a dev-style look.
56. The menu is a modal overlay covering almost the whole screen, so layout and color edits cannot be seen at the same time as the widgets.
57. Asynchronous tab renders have no request token: with fast switching, an old response overwrites a new one. `menu.js:127-175`.
58. A load error becomes a false empty state («Нет созданных папок» (No folders created), an empty grid) or endless «Загрузка…» (Loading…). `menu.js:177-184, 381-384`.
59. Enter does not submit any form (folder, shortcut, subscription, preset).
60. «После перезапуска» (After restart) has no restart button.

## 8. Accessibility and CSS

61. There is no `:focus-visible` anywhere; `outline: none` has no replacement (`menu.css:303, 850, 926`). The only replacement is a white 8% ring, which is invisible on the light theme.
62. Contrast is below 4.5:1 (calculated from the tokens): section descriptions 12px α0.45 (Sea Glass ≈2.7:1), inactive tabs α0.55 (≈3.6:1), placeholders α0.28 (≈2.5:1), table headers, `.widget-mgmt-type`, and the «Удалить» (Delete) button.
63. Small targets: tint dots 20×20, a 6px slider, «Удалить ярлык» (Delete shortcut) 28×27, and preset deletion with no size.
64. ARIA: tabs have no `tablist/tab/aria-selected`; the dialog has no `role=dialog`/`aria-modal`; focus is not moved inside and not returned; pills have no `aria-pressed`; sliders have no associated `label`.
65. Sea Glass: the jelly overrides cover only tabs, cards and the slider. White borders on `bg-elev` 255 are almost invisible at the header, buttons, inputs, presets and pills (`menu.css:65-1088`).
66. Hardcoded values outside the tokens: overlay background `rgba(5,5,8,.72)` instead of `--scrim-rgb`, blur 16px, radii 6–24px, shadows, `#e6e6e6`, danger border `rgba(215,25,32)` (32 instead of 33), ~20 dead fallbacks `#d71921`; inline styles in `menu.js` (`205, 236-271, 403-413, 680`).
67. `transition: all` (10 places), `ease` curves instead of tokens (17 places), and `translateY(6px)` in keyframes.
68. The menu's `z-index` equals the explorer's (2000); the token `--z-surface-menu` (4000) is not used.
69. Fixed columns (340px, 140px/110px) and a media query based on the viewport rather than the container.
70. Three classes encode one open state (`visible`, `surface-active`, `surface-open`).
