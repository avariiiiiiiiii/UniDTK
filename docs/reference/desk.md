# The Desk, the Menu, and Themes

The desk is the window behind the icons, with widgets on it. This chapter states how widgets are placed, how they nest, and how a theme paints them. Animation has its own chapters, [Motion](motion.md) and [Surfaces](surfaces.md).

## Placing widgets

r[desk.positions]

A widget's position is `config.positions[id]`: `x` and `y`, plus a corner anchor `anchor ∈ {tl, tr, bl, br}` with its offset `ax`/`ay`. `placeOnCanvas` in `core/ui/unidtk-core.js` reads it; without an `anchor` the corner is `tl`. Arrange mode snaps to a 16 px grid and to corners, and writes the corner on "Done"; "Undo" or Esc restores everything it touched. The arrange island stands at the right edge, in the slot of the side menu; `config.arrangeSide` (`"left"` or `"right"`) is the user's choice of edge, a preference that "Undo" does not revert.

r[desk.relayout-not-reset]

The widget canvas is never reset (`widgetsCanvas.innerHTML = ""`) when the geometry changes, the window moves between monitors, or `display-changed` fires. `relayoutWidgets()` moves and resizes the existing widgets, so their state, network connections, and the user's input survive.

r[desk.folders]

A folder shortcut (`FolderItem`: `target`, `args`, `kind`) refers to the real target, not to a copy of a `.lnk` file. Start menu entries, icons, and launching go through `core/kernel/start_menu.rs`, which runs COM on an STA thread; editing on the desk is `folder-body.js`.

r[desk.no-blocking-dialogs]

An irreversible action runs at once and is offered back as a 5 second "Undo" toast (`toast.js`). `alert` and `confirm` are not used.

## Nesting

r[desk.nesting]

A widget can live inside another widget instead of on the desk (`core/ui/nesting.js`). Two `metadata.json` fields decide it: `container` (the widget hosts others; folders always do) and `embeddable: {module, render}` (the file and export that draw the compact form). `canNest(child, host)` is the only gate: it rejects a widget inside itself, a host that is not a container, a container as a child, and a child without `embeddable`.

Nesting is one level deep, so a cycle cannot exist. `config.nested` (`{hostId: [childId]}`) is the user's intent and is never trimmed. `effectiveNested(stored, widgets)` computes what applies now: a child whose host is absent shows on the desk until the host returns, and a child listed twice belongs to the first host.

## Themes

r[theme.tokens-only]

Appearance comes from tokens, never from hardcoded values: `--dur-*`, `--ease-*`, `--r-*`, `--blur-*`, `--theme-*`, `--z-surface-*`. The source of truth is `core/ui/motion.js` for timing and `core/ui/theme.js` for presets; the values in `:root` of `core/ui/styles.css` are only a fallback.

`core/ui/theme.js` ships two presets, Nothing Classic and Sea Glass, plus user themes. A preset changes only appearance: positions are user data, sizes are preset defaults, and the user's edits win. The theme is applied before the first paint from the `paint` cache, and a change fires `unidtk-theme-changed`.

r[theme.module-contracts]

`theme.js` and the menu name no module. A module declares its share of the theme in `metadata.json`: `tints [{id, label, default, role}]` (a surface that can be tinted; `default` is a palette slot; `role` groups targets that share one color when colors are shuffled) and `lookOptions` (module settings that belong to the theme). `registerThemeContracts(modules)` reads them.

r[theme.contrast]

A theme declares `onAccentRgb`, the color of text on the accent. `readableOn(surfaceHex, preferredRgb)` keeps it while the contrast is at least 4.5:1 (WCAG AA) and otherwise picks the better of white and near-black, so a user's accent can never make text unreadable.

r[theme.per-theme-colors]

The user's colors are stored per theme in `config.colorOverrides` (palette, `tintMap`, color fields).
