# Telegram: The TDLib Client, Forum Topics, and Communities

This chapter explains how the `telegram` module talks to Telegram. It covers how the TDLib library is loaded, how one background thread receives every update, how forum topics and communities become a three-column view, and how the module keeps a local ByeDPI proxy in step with the VPN.

The original design note was written on 2026-09-01. This version was re-checked against the code on 2026-11-02. Where the old note described something the code no longer does, the code is described here, and the difference is named.

## What you will understand by the end

- How TDLib is loaded at runtime, and what the four functions of its JSON interface do.
- Why one receiver thread routes every reply and every update.
- How a group becomes a forum, and how a community becomes a list of subchats.
- How the managed ByeDPI proxy is started, probed, and stopped, and how the VPN state changes it.
- How the three-column layout is built in the widget.

## Files involved

| File | Role |
| :--- | :--- |
| [modules/telegram/rust/telegram.rs](../../modules/telegram/rust/telegram.rs) | The module: TDLib loader, receiver, requests, updates, proxy, forum and community logic (6,257 lines). |
| [modules/telegram/ui/widget.js](../../modules/telegram/ui/widget.js) | The widget: chat list, topic column, thread view (7,086 lines). |
| [modules/telegram/ui/telegram.css](../../modules/telegram/ui/telegram.css) | Layout, including the topic grid (3,713 lines). |
| [core/api/src/paths.rs](../../core/api/src/paths.rs) | Locations of the data directory, the TDLib folder, and the ByeDPI binary. |

Native binaries sit in the repository's `opt/` folder. [toolchain/build.rs](../../toolchain/build.rs) copies them next to the built executable: the `copy_native_binaries` function scans `opt/tdlib-native/win32-x64`, `opt/libmpv`, `opt/singbox`, and `opt/byedpi` for `*.dll` and `*.exe` files. The TDLib folder contains `tdjson.dll` (27,526,656 bytes), `libssl-3-x64.dll`, `libcrypto-3-x64.dll`, and `z.dll`.

The old note lists `xray.exe` in `opt/`. That file is not in the repository now. Its role is taken by `sing-box.exe` (see [the VPN chapter](vpn.md)). The old note also mentions `scripts/update-deps.ps1`. There is no `scripts/` folder in the repository, so that updater does not exist.

---

## Step 1: Loading TDLib at runtime

TDLib is the official Telegram client library. The module uses its JSON interface, which needs four functions: create a client, send a request, receive an update or reply, and destroy the client.

The module does not link `tdjson.dll` at build time. It loads it when it first needs it, with `LoadLibraryExW`, and fetches each function with `GetProcAddress`:

```rust
let library = unsafe {
    LoadLibraryExW(PCWSTR(path_w.as_ptr()), None, LOAD_WITH_ALTERED_SEARCH_PATH)
}
// ...
let create_fn: TdCreate = load_symbol(library, b"td_json_client_create\0")?;
let send_fn: TdSend     = load_symbol(library, b"td_json_client_send\0")?;
let receive_fn: TdReceive = load_symbol(library, b"td_json_client_receive\0")?;
let destroy_fn: TdDestroy = load_symbol(library, b"td_json_client_destroy\0")?;
```

`LOAD_WITH_ALTERED_SEARCH_PATH` makes Windows resolve the DLL's own dependencies (OpenSSL and zlib) from its own folder. The comment in the code says the application's current folder is not reliable, because Tauri can change the search path.

If the load fails, the time is recorded in `last_td_load_failure`. For the next 20 seconds, `ensure_started` does not try again (`TD_LOAD_FAILURE_COOLDOWN`). Without this cooldown, a status poll every 750 ms would load the DLL over and over.

### 1.1 Where the DLL is found

`bundled_tdjson_path` returns the first existing file from this list:

1. `tdjson.dll` in the working directory, under the `opt/tdlib-native/win32-x64` layout and an older `backend/opt` layout,
2. `tdjson.dll` next to the executable, and a path under the older `app-tauri/backend/opt` layout,
3. `opt/tdlib-native/win32-x64/tdjson.dll` in the source tree, found through `CARGO_MANIFEST_DIR`.

The `TDJSON_PATH` environment variable or the `tdjson_path` setting overrides the search.

### 1.2 Credentials

`ensure_started` needs an API ID and an API hash. It reads `TELEGRAM_API_ID` and `TELEGRAM_API_HASH` from the environment first, then the saved configuration. The module ships default values (`DEFAULT_TELEGRAM_API_ID` and `DEFAULT_TELEGRAM_API_HASH`, at the top of `telegram.rs`). If the ID does not parse or the hash is empty, the module enters the `mock` mode with the hint `Enter Telegram API ID, API hash and tdjson.dll path.`

### 1.3 Data on disk

Each TDLib account has its own database folder under the data directory: `%LOCALAPPDATA%\UniDTK\tdlib` for the default account, and `tdlib\accounts\<id>` for others (`tdlib_account_dir`). The configuration is `%LOCALAPPDATA%\UniDTK\telegram_config.json` (`telegram_config_path`).

---

## Step 2: The receiver thread

TDLib works asynchronously. A request goes in, and its reply comes out later, mixed with every update Telegram sends. The module handles this with one thread that owns all reads.

### 2.1 Starting the thread

`ensure_started` creates the `TdJson` client, sets the log level, and starts the thread:

```rust
let verbosity: i64 = std::env::var("UNIDTK_TD_LOG_LEVEL")
    .ok()
    .and_then(|value| value.trim().parse().ok())
    .unwrap_or(2);
let _ = td.send(&json!({"@type": "setLogVerbosityLevel", "new_verbosity_level": verbosity}).to_string());
g.receiver_handle = Some(std::thread::spawn(move || {
    receiver_loop(td_thread, inner_thread, pending, shutdown);
}));
g.send(json!({"@type": "getAuthorizationState"}))?;
```

The default verbosity is 2, which keeps TDLib to warnings and errors, because the app's own log is the main record. `UNIDTK_TD_LOG_LEVEL` raises it when TDLib itself needs debugging.

### 2.2 The loop

```rust
while !shutdown.load(Ordering::SeqCst) {
    let text = match td.receive(0.5) { Ok(Some(t)) => t, Ok(None) => continue, Err(_) => break };
    let value: Value = serde_json::from_str(&text)?;
    if let Some(extra) = value.get("@extra").and_then(...) {
        if let Some(tx) = pending.lock().unwrap().remove(&extra) {
            let _ = tx.send(value);
            continue;
        }
    }
    locked.handle_update(value);
}
```

`receive(0.5)` waits up to half a second for the next message, so the loop checks the shutdown flag twice per second. Every message is routed in one of two ways:

- If it carries an `@extra` number that matches a request in the `pending` map, it is a reply. It goes to the waiting caller through a channel.
- Anything else is an update, and it is handed to `TelegramInner::handle_update`.

### 2.3 Making a request

`td_request` sends a request with a fresh `@extra` number, registers a channel for that number, and waits for the reply without holding the module's main lock:

```rust
let (tx, rx) = mpsc::channel();
pending.lock().unwrap().insert(extra, tx);
td.send(&request.to_string())?;
// ... then:
match rx.recv_timeout(timeout) { ... }
```

If the wait times out, the entry is removed from `pending`. If the client is shut down while a request waits, the channel disconnects, and the caller gets `TDLib client stopped`. This case is kept apart from a timeout on purpose: a shutdown is not a network failure, and it must not trigger the cooldown logic.

### 2.4 Shutting down

`shutdown_client` sets the shutdown flag, sends a harmless `getOption` request to wake a blocked `receive`, joins the thread, and only then drops the `TdJson` value. The drop calls `td_json_client_destroy` and then `FreeLibrary`. The order matters: destroying the client while the thread is still inside `receive` would be a use-after-free.

---

## Step 3: Handling updates

`handle_update` is one `match` on the `@type` field of each update. Three groups matter here:

- **Connection state.** `updateConnectionState` is normalized to `ready`, `connecting`, `connecting_to_proxy`, and so on. It is sent to the frontend as the event `telegram-connection-state`, and it drives the proxy heal described in Step 5.4.
- **Chat list changes.** `updateNewChat`, `updateChatTitle`, `updateChatLastMessage`, `updateChatReadInbox`, `updateChatPosition`, `updateChatNotificationSettings`, `updateChatViewAsTopics`, `updateCommunity`, and `updateCommunityFullInfo` go to `handle_chat_update`. Each one emits `telegram-chat-update`. `updateChatAddedToList` and `updateChatRemovedFromList` go to `handle_chat_list_update`, which also emits that event.
- **Messages.** `updateNewMessage` is parsed and placed in the cache for its chat and topic (Step 6.3).

Only the types above emit `telegram-chat-update`. The comment at that match explains why: TDLib sends many other updates even at rest (presence, options, file progress), and emitting an event for each one used to rebuild the whole widget about once a second. That rebuild reset the avatar cache and fought the message scroll position.

---

## Step 4: The managed ByeDPI proxy

### 4.1 What ByeDPI does

ByeDPI (`ciadpi.exe`) is a local SOCKS5 proxy. It does not need a server, administrator rights, or a driver. It alters the first packets of a connection (split, disorder, and TLS record splitting) so that the network's DPI cannot read the destination. The module starts it with this fixed profile:

```rust
const BYEDPI_ARGS: &[&str] = &[
    "--split", "1",
    "--disorder", "3+s",
    "--mod-http", "h,d",
    "--auto", "torst",
    "--tlsrec", "1+s",
];
```

It listens on `127.0.0.1:12909` (`MANAGED_PROXY_PORT`). The port is different from the VPN's SOCKS port 12908, so the two cannot collide.

The module tells TDLib to use a SOCKS5 proxy at `127.0.0.1:12909`, through the request `addProxy`. TDLib then sends its Telegram traffic through ByeDPI.

### 4.2 Starting the proxy

`ensure_managed_proxy` runs these steps:

1. If a child process from an earlier call is still alive and its port answers, it is reused.
2. If the port already answers as a SOCKS5 server, something is listening there, and the module reuses that listener.
3. Otherwise it starts `ciadpi.exe -i 127.0.0.1 -p 12909` with the profile above, with no console window (`CREATE_NO_WINDOW`).
4. It checks the child every 100 ms, for up to 50 tries (5 seconds). If the child exits, the error is `ciadpi exited early`. If the port opens, the child is kept.

A port that is open is not enough. `socks5_probe` sends the SOCKS5 greeting `05 01 00` and requires the reply `05 00`. Without this check, another program on the same port would be taken for ByeDPI, and TDLib would be sent to a service that cannot proxy anything.

If a start fails, `PROXY_ENSURE_COOLDOWN` (30 seconds) stops the module from trying again on every poll.

### 4.3 Stopping the proxy

`stop_managed_proxy` kills the child it started, if there is one, and waits for it. It runs when the proxy mode is changed away from `byedpi`, when the VPN takes over, and at shutdown (`begin_shutdown`).

### 4.4 The proxy mode

`normalize_proxy_type` turns the saved setting into one of the modes: an empty or missing value means `byedpi`, and the old value `vless` (from a managed VLESS tunnel that no longer exists) is migrated to `byedpi`. The other modes are `socks5`, `http`, and `none`. Those are the user's explicit choices, and the module leaves them alone.

---

## Step 5: The VPN and the proxy

### 5.1 Reading the VPN state

The module does not import the VPN module. It reads the state from the contract:

```rust
fn vpn_connected_from_contract() -> bool {
    unidtk_api::state("vpn")
        .and_then(|payload| payload.get("connected").and_then(|v| v.as_bool()))
        .unwrap_or(false)
}
```

`subscribe_vpn_events` registers a handler for `vpn` events. The handler calls `on_vpn_state_changed`, which calls `sync_vpn_proxy`. This is the only link between the two modules, and it runs in one direction, through the contract.

### 5.2 What happens when the VPN changes

`sync_vpn_proxy` decides what TDLib should use:

- **VPN up:** Telegram traffic already leaves through the VPN's TUN. The module stops ByeDPI and tells TDLib to connect directly (`disableProxy`).
- **VPN down:** the module starts ByeDPI, if needed, and points TDLib at `127.0.0.1:12909`.
- **Heal:** if the VPN is down and the port is closed, the proxy is applied again. Without this, a single application after login could leave Telegram without any route.

The function does nothing when the mode is not `byedpi`, or when TDLib is not running yet. In the second case, `apply_proxy_config` reads the VPN state directly when TDLib starts, so the first setup already takes the VPN into account.

A note on the code: a comment near `ensure_started` says `apply_proxy_config()` reads `vpn::is_connected()`. The code now reads the contract (`vpn_connected_from_contract`). The comment is out of date and should be corrected.

### 5.3 Why the frontend does not have to poll

The comment on `sync_vpn_proxy` explains why this is event-driven. The frontend stops polling the Telegram status once the account is authorized. Before the event existed, a toggle of the VPN could leave the proxy in the wrong state until the next poll, which might never come.

### 5.4 The stuck-connection heal

When `updateConnectionState` reports `connecting` or `connecting_to_proxy` for a long time, the module applies the proxy configuration again. The rules are in `on_connection_state_changed`:

- Nothing happens for 120 seconds after the TDLib client starts (`PROXY_HEAL_GRACE`), because a cold MTProto handshake through a DPI desync proxy can take tens of seconds.
- The state must have lasted at least 30 seconds (`PROXY_HEAL_BACKOFF_SECS[0]`).
- Each heal is followed by a wait: 30, 120, or 300 seconds (`PROXY_HEAL_BACKOFF_SECS`), with the last value repeated.
- No heal is attempted while the VPN is up, or while a proxy command is still in flight.

When the state returns to `ready`, the counters are reset.

---

## Step 6: Forum topics

### 6.1 How a group becomes a forum

In TDLib's JSON API, a supergroup is a forum when its `view_as_topics` field is true. The code also accepts an `is_forum` field, in two places, in case a payload carries it. `parse_chat` checks all three in this order:

```rust
let is_forum = chat
    .get("view_as_topics")
    .and_then(Value::as_bool)
    .or_else(|| chat_type.and_then(|t| t.get("is_forum")).and_then(Value::as_bool))
    .or_else(|| chat.get("is_forum").and_then(Value::as_bool))
    .unwrap_or(false);
```

The flag can change later. An `updateChatViewAsTopics` update sets `chat.is_forum` again in `handle_chat_update`.

### 6.2 Reading the topics

`get_forum_topics` asks TDLib for the topics with `getForumTopics`, with a limit of 100. If TDLib returns at least one topic, that answer is used as it is.

### 6.3 Messages in a topic

A message in a forum carries a `message_thread_id`. The module keeps it only for forum chats:

```rust
let thread_id = if is_forum {
    parsed.message_thread_id.filter(|id| *id != 0)
} else {
    None
};
let cache_key = cache_key_for(&chat_id, thread_id);
```

`cache_key_for` joins the chat ID and the thread ID with `::`: `chatId::threadId` for a topic, and the bare `chatId` otherwise. Messages of different topics are therefore cached separately.

---

## Step 7: Communities and subchats

A community, such as `Re:Zero | Элиор`, is a set of related channels and groups. TDLib links each channel to its community with a `linked_community_id` field. This is the field the module reads, in `handle_chat_update` and `parse_chat`.

The problem is that a community is not a forum. Asking TDLib for its topics fails with `The chat is not a forum`. `get_forum_topics` therefore has a fallback:

1. If `getForumTopics` returns an error or no topics, the module looks through the chats it has already loaded.
2. It collects every chat whose `linked_community_id` equals the parent's ID, and also every chat whose title matches the parent's title pattern.
3. It returns those chats in the same shape as topics, with `subchat_id` set to the chat's ID, sorted by name.

The title pattern is hard-coded:

```rust
(parent_title.contains("Элиор") && c.title.contains("Элиор")) ||
(parent_title.contains('|') && c.title.starts_with(parent_title.split('|').next()...))
```

The first part matches any title that contains `Элиор`. The second matches a title that starts with the part of the parent's title before `|`. This is a heuristic written for one set of communities. It is not a TDLib feature, and a community with other names will not be found by it. Only the `linked_community_id` match is structural.

The same `Элиор` rule sets the `has_subchats` flag in `parse_chat`: a channel whose title contains `Элиор` is marked as having subchats. The widget uses that flag (Step 8.3), so the heuristic affects the UI as well.

When a subchat is opened, the widget passes its `subchat_id` as a chat ID to `telegram_messages`, so the same thread view shows it.

---

## Step 8: The widget

### 8.1 The grid

A chat with topics or subchats switches the widget to three columns. The class `forum-topics` is added to `.tg-layout`, and the CSS sets:

```css
.tg-layout.forum-topics {
  grid-template-columns: 46px min(164px, calc(42% - 46px)) minmax(0, 1fr);
}
```

- **Column 1** (`.tg-chat-list`) shrinks to a 46-pixel rail of avatars. The text parts of each row are hidden.
- **Column 2** (`.tg-topics-list`) holds the topics or subchats. Its width is `min(164px, calc(42% - 46px))`, not the fixed 200 pixels in the old note. The width depends on the widget's size.
- **Column 3** (`.tg-thread`) takes the remaining width. The widget does not rebuild it when a topic is opened, so the messages stay on screen.

Widths change with a transition on the grid, so the switch between two and three columns is animated.

### 8.2 The header and the back button

The topics column starts with a button that returns to the full chat list:

```html
<button type="button" class="tg-topics-back-btn" data-action="back-to-chats" aria-label="Назад к чатам">
  ...
  <span>Все чаты</span>
</button>
```

The visible text is `Все чаты` (All chats). The arrow is an SVG icon. The `aria-label` is `Назад к чатам` (Back to chats). The old note showed a literal `←` character in the text; it is not in the code.

The button's handler is wired in `wireTelegramTopicsSection`. It sets `forumOpenChatId` to `null` and redraws the widget. The column closes, but the selected topic is kept, with its reading position, so the thread on the right does not change. The chat list has a separate handler for a `back-to-chats` element (around `widget.js` line 2787), which clears the topic selection for that chat. Opening a plain chat that is not a forum also clears `forumOpenChatId`.

### 8.3 Which chats open the column

A click on a chat row checks three flags:

```javascript
if (chat?.isForum || chat?.hasSubchats || chat?.linkedCommunityId) {
  telegramRuntime.forumOpenChatId = chat.id;
  loadForumTopics(chat.id);
  ...
}
```

`hasSubchats` is the flag set by the `Элиор` heuristic (Step 7). `linkedCommunityId` comes from TDLib. Any of the three opens the column.

### 8.4 Which thread is shown

`setTelegramForumThread` stores the selected topic in `forumTopicByChat`, a map from chat ID to thread ID. A thread ID of `0` or an empty value is treated as "no topic". `telegramForumThreadId` reads the map.

The message loader does not receive the thread ID as a parameter. `refreshTelegramMessages(card, widgetId, chatId, force)` reads the topic from the map and sends it to the `telegram_messages` command as `threadId`. Its message cache key comes from the same map (`telegramMessageCacheKey`). The old note said the thread ID was passed in; the code reads it from the map.

### 8.5 The tail first, the head after the morph

The browser lays out a freshly inserted message row for about 0.3 ms, measured live: 73 rows took 23 ms, 30 rows 9 ms. A click that builds the whole cached history (up to 300 messages) therefore holds the main thread before the first frame of the morph. So two clicks build only the end of the history:

- the open of the panel (`renderTelegramWidget` with `preparing`);
- a chat switch inside the open panel (`telegramSetActiveChat`, the morph path).

`telegramPreparedTailStart` picks the first row: the last `TELEGRAM_PREPARED_TAIL_MESSAGES` (30) messages, reaching back to the saved reading position and to a pending jump target, and never starting inside an album. `buildMessagesInnerHtml(chat, messages, from)` renders from that row. Each row still sees its real predecessor, and the reply map covers the whole history, so a tail row is the same markup it has in the full list. The load-earlier button belongs to the head and is left out.

When the surface is open (`activateTelegramExpandedRender`) or the morph commits, `fillTelegramHistoryHead` writes the rows above the tail, adds the height they take to `scrollTop`, and wires only the new rows. The tail's nodes and media stay. It writes nothing when the history changed since the tail was built: it rebuilds the tail markup from the current data and compares it with the markup on screen. Then the section cache still holds the tail, and the next `updateTelegramWidget` rebuilds the list in full. A rebuild of the scroller drops the pending request, so a head is never written twice.

When the history changes while the chat is open (a new message, an edit of the last one, a refresh that arrives during the open), `patchTelegramThreadRows` brings the rows up to date in place. It builds the new markup as a head plus one string per row (`telegramMessageRowStrings`), finds the first byte where it differs from `_tgSectionCache.thread` (the markup the DOM was written from), and rewrites only the rows from the row that holds that byte. Older rows keep their nodes, media and scroll anchor (`captureTelegramScroll` / `restoreTelegramScroll` around the edit), and only the new nodes are wired. With a head fill pending, the cache holds the tail markup, so the comparison runs on the tail and the fill request is renewed with the new markup. The function returns `false`, and `updateTelegramWidget` rewrites the whole list, when the DOM cannot be proven to match: an error or loading state, a pending jump anchor, a changed head (the load-earlier button appears or goes), or a child count that is not the row count after the edit.

Time labels use two module-level `Intl.DateTimeFormat` objects. `toLocaleTimeString` with options builds a new formatter on every call, about 0.5 ms each, and that was half of the click before session 136.

---

## Step 9: The verification record

The original note recorded a five-stage check, run on 2026-09-01. It is copied here as history. It was not run again for this rewrite. The commands it names (`shell act click`) are from an older CLI.

| Stage | Command or method | Result as recorded |
| :--- | :--- | :--- |
| 1. Static check | `cargo check`, `node --check` | No errors, 0 warnings |
| 2. Full build | `cargo build --release` | Exit code 0, 2 min 30 s |
| 3. Runtime diagnostics | `shell doctor` | All DLLs found, IPC active |
| 4. Forum testing | click `qbc`, then `General` | 4 topics in the column; messages of the General thread loaded |
| 5. Community testing | click `Re:Zero \| Элиор`, then `Ледяной лес Элиор` | 3 community channels shown; the subchat opened |
| 6. Back navigation | click `Назад к чатам` | Topic column closed; chat list back |

---

## Summary

- TDLib is loaded at runtime from a list of candidate paths. Its four JSON functions are called through function pointers.
- One thread reads everything. Replies are matched to requests by `@extra`, and all other messages are updates.
- The local ByeDPI proxy is used only when the VPN is down. Its port is verified with a SOCKS5 greeting, not just a TCP connect.
- A forum is a supergroup with `view_as_topics`. A community is found by `linked_community_id`, and by a title heuristic that is specific to one set of communities.
- The widget shows three columns for a forum or community, keeps the thread on screen, and caches messages under `chatId::threadId`.
