# Chapter 5. Controlling the WebView over CDP

`udtk gui`, `udtk shot`, `udtk cdp-list`, and `udtk stop` talk to the WebView2 engine through the Chrome DevTools Protocol (CDP). CDP is a JSON protocol over WebSocket that the browser engine exposes on a local port. This chapter explains how the CLI finds the right window and drives it safely.

## 5.1 Always on, and the port is discovered

WebView2 is started with a remote-debugging port. [core/kernel/main.rs](../../core/kernel/main.rs) decides the port in `pick_cdp_port()`:

1. Try `127.0.0.1:9222`. If it can be bound, use it.
2. Otherwise bind port 0 and let the operating system choose a free port.

The port is stored once (`CDP_PORT`) and published over the socket as `cdpPort` in the `ping` reply. The CLI never guesses. A stale or foreign process may hold 9222, and a blind guess would drive the wrong window. In one recorded probe, 9222 was held by a foreign listener and the app took port 54060, and the old `eval` command worked through it (see [H3](../history/rfcs/udtk-migration.md#h3-slices-s0-to-s5-former-109-to-1022)).

The browser arguments are assembled by `merge_browser_args()`. It keeps whatever `WEBVIEW2_ADDITIONAL_BROWSER_ARGUMENTS` already holds, **except** an inherited `--remote-debugging-port=` or `--remote-allow-origins=`, and then appends the kernel's own arguments and `--remote-debugging-port=<port>`. The kernel no longer overwrites what the caller set. `--remote-allow-origins` is not set anywhere in the code: the old wildcard was removed (session records in [H3](../history/rfcs/udtk-migration.md#h3-slices-s0-to-s5-former-109-to-1022)).

## 5.2 Finding the page, and refusing foreign windows

The CLI asks `cdpPort` of the app, through `ping`, and caches the answer for the process (`cdp_port()`). If the app is older and does not publish a port, the CLI falls back to 9222, because such a kernel always binds it. If the app does not answer at all, the error is `cannot determine the CDP port: app is not answering on 127.0.0.1:26540`.

Then `cdp_ws_url()` in [opt/cli/main.rs](../../opt/cli/main.rs):

1. Reads the target list over plain HTTP (`GET /json`, with `Host: 127.0.0.1`).
2. Chooses the page whose URL is `http://tauri.localhost/` (the wallpaper document). A transient window, such as the location-permission host, is also a page, and must never be chosen by list order.
3. **Refuses** any target whose URL does not start with `http://tauri.localhost`, with `CDP target is not the UniDTK page (url: ...); refusing to drive a foreign window`.
4. Rewrites a `ws://localhost/...` address to `ws://127.0.0.1:<port>/...`, which WebView2 sometimes reports without a port.

## 5.3 One session per batch

The `Cdp` structure in [opt/cli/main.rs](../../opt/cli/main.rs) holds **one** WebSocket and a counter of request ids. `udtk gui` opens it once and runs all its steps over that connection. Before this design, every call opened a new WebSocket, so one click cost five handshakes.

`Cdp::call` sends `{"id", "method", "params"}` and reads frames until it finds the reply with the same id. Frames without a matching id are progress events, and they are ignored on purpose. Each call has a 10 second deadline (`CDP response timeout for <method>`). A CDP error becomes `CDP error: <message>`.

`Cdp::evaluate` calls `Runtime.evaluate` with `returnByValue` and `awaitPromise`. A JavaScript exception becomes `JS Exception: <text>`.

## 5.4 Input conventions

Mouse input is `Input.dispatchMouseEvent` in three steps: moved, pressed, released. The coordinates are **CSS pixels** straight from `getBoundingClientRect()`. CDP takes them **1:1**. Multiplying by `devicePixelRatio` would miss the target by a factor of 1.25 to 1.5, and the comment in `mouse_click` says so.

Keyboard input is `Input.dispatchKeyEvent`, and the `press:` step emulates a **US keyboard**, independent of the system layout. The `code` is physical (`KeyA`, `Digit1`), and `vk` is the virtual key code. Only DOM `key` names are accepted (`Escape`, `ArrowLeft`, `Enter`). Aliases such as `Esc` or `Left` are refused, so each capability has one name. Text is entered with the `type:` step (`Input.insertText`), not with `press:`.

## 5.5 The page harness

The page defines `window.__UNIDTK_HARNESS__`, implemented in [core/ui/harness.js](../../core/ui/harness.js). The CLI uses three of its functions:

- `getSemanticTree()`: a list of the interactive elements, each with a numeric `id`, a `ref`, an accessible name, a `point` (the centre in CSS pixels), and an `occluded` flag with the `covering` element when something sits on top.
- `getErrors(clear)`: the page errors collected since the last call. With `true`, the buffer is cleared.
- `resolveTarget(target)`: maps a target string to a node. The order in the code is: a numeric id, then a stable reference `r<N>` (the `data-udtk-ref` attribute, which survives re-rendering), then `@testid`, and then the other locators.

A node that has neither a test id nor an element id is marked `fragile: true`. The harness skips anything inside `.simulation-sandbox`, so that a diagnostic render is never offered as a target.

## 5.6 Security boundary

Honest statement, unchanged since the design decision of [Chapter 6](ch06-design-decisions.md#chapter-6-design-decisions): CDP is always on in release. Any local process of the same Windows user can connect to it and run JavaScript in the page. The boundary is the **Windows user account**, not a token and not a flag. Two things do close the web-page class: the CDP HTTP endpoint checks the `Host` header (only IP or localhost is accepted), and it sends no CORS headers. These two statements come from the recorded live probes of slice S0, not from code that can be read here. The CSP of the page allows `ws://127.0.0.1:*` in `connect-src` ([toolchain/tauri.conf.json](../../toolchain/tauri.conf.json)), so the page can reach the WebSocket door.
