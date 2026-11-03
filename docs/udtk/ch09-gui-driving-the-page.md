# Chapter 9. `gui`: driving the page

```bash
udtk gui [--json] [--force] <step>...
```

`gui` runs a batch of steps in the page. It is the replacement for three older commands, `dom`, `eval`, and `act`, which were removed. The batch has at most **50** steps, and it opens one CDP session (Chapter 5) for all of them.

## 9.1 The steps

| Step | What it does |
|---|---|
| `js:<expr>` | Evaluates JavaScript in the page and returns the value |
| `click:<target>` | Moves the mouse to the centre of the target, presses, and releases |
| `type:<target>:<text>` | Clicks the target to focus it, then inserts the text (`Input.insertText`) |
| `press:<key>` or `press:<mod>+<key>` | Presses one key. Modifiers are `ctrl`, `shift`, `alt`, `meta` |
| `tree` | Returns the semantic tree of the page |
| `wait:<ms>` | Pauses, at most 10 000 ms; a longer or non-numeric value fails the step instead of being clamped |

The `<target>` is resolved by the page's own harness (Chapter 5, 5.5): a numeric id from the tree, a stable reference `r<N>`, a test id written `@name`, an accessible name, or a selector.

Examples:

```bash
udtk gui js:6*7
udtk gui tree
udtk gui click:r12
udtk gui type:@search:hello press:Enter
udtk gui press:ctrl+a
udtk gui js:window.__UNIDTK_BRIDGE__.getTransportState()
```

## 9.2 The receipt

Each step produces a receipt, so the caller never has to trust that a step worked. The fields the code writes are `step`, `kind`, `ok`, `target`, `hit_point`, `covering`, `refused`, `text`, `key`, `value`, `error`, `log_delta`, `ui_errors`, and `duration_ms`.

- `log_delta` holds the journal records written since the previous step. It is read with the socket command `log_delta`, one round trip per step.
- `ui_errors` holds the page errors collected since the previous step.
- `ok` is derived from what happened, not from whether a message was sent. A click on an occluded target has `ok: false`, not `ok: true`.

`--json` prints `{"ok": ..., "steps": [...]}`. The text mode prints one line per step. Any failed step gives exit 1.

## 9.3 Occlusion is a refusal

Before a click, the harness reports whether the centre of the target is covered by another element. If it is, the step is **refused**, not sent:

```text
ok: false, refused: "occluded", covering: {tag, id, testid, ref}
```

The receipt names the covering element. `--force` lifts the refusal and clicks anyway. This rule came from a live finding: the old `act` read the flag and sent the click anyway, and returned `status: ok` while the press landed on another element (see [H3](../history/rfcs/udtk-migration.md#h3-slices-s0-to-s5-former-109-to-1022)).

## 9.4 Key refusals

A key that cannot be emulated is refused before anything is sent, so that no `keyUp` is left without its `keyDown`:

| `refused` value | Cause |
|---|---|
| `unknown-key` | The name is not a canonical DOM key name (for example `Esc`, `Left`) |
| `non-ascii` | A non-ASCII character was given to `press:` (use `type:` for text) |
| `unknown-modifier` | The modifier is not `ctrl`, `shift`, `alt`, or `meta` |
| `empty-key` | Nothing follows the modifiers |

Keys that used to be inert through the CLI, such as `Delete`, `Backspace`, `F2`, and `F5`, now act on the page. Do not run them against live data in a check.

## 9.5 Failure modes

| Message | Meaning |
|---|---|
| `too many steps in one batch (max 50)` | Split the batch |
| `CDP not reachable on <port>: ... (is the app running?)` | The app is not running, or CDP is not open |
| `CDP target is not the UniDTK page ...` | The first page is not the app's page; the CLI refuses to drive it |
| `target not found in the active UI: <target>` | No element matches; use `tree` to see the names |
| `unknown step kind: <kind>` | A step was not understood. In PowerShell 5.1 a multi-line `js:` argument is split at line breaks and double quotes, and each piece becomes a step. Write one line, with single quotes inside |
