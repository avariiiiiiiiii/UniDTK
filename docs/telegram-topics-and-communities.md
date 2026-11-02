# Architecture of Forum Topics, Telegram Communities and Dependency Management

> **Document status:** Accepted and implemented  
> **Date:** 2026-09-01  
> **Related components:** `backend/modules/telegram`, `frontend/modules/telegram`, `backend/toolchain/build.rs`, `backend/cli`

---

## 1. Context and problem statement

In the previous version of the Telegram widget in Shell, groups with topics (TDLib forums) and channel/group communities (Community Hubs, e.g. `Re:Zero | Элиор`) were shown as ordinary flat chats.

### User requirements:
1. **Forums and topics**: when a group with topics is clicked, the left chat list must collapse into a compact strip of avatars (rail), freeing space for a topic column (200px), while the right message area (the thread) must not be reset or reloaded.
2. **Channel communities (Communities)**: communities (such as `Re:Zero | Элиор`) that group several related channels and groups must work like topics: the middle column shows the list of related subchats (e.g. `Re:Zero / Аниме 3 сезон`, `Re:Zero | Элиор`, `Ледяной лес Элиор`), and clicking one switches the active thread to that channel.
3. **No manual copying**: all binary dependencies (`tdjson.dll`, `libmpv-2.dll`, `xray.exe`) are kept in the canonical folder `backend/opt/` and are copied automatically by the build system `build.rs` into `target/release/` when `cargo build` runs.
4. **Dependency update mechanism**: a single automated tool for checking and updating the native libraries.

---

## 2. Updating TDLib to version 1.8.67

### 2.1 Native binaries
The current 64-bit Windows libraries were downloaded from the official NuGet repository `tdlib.native.win-x64` (version 1.8.67, dated 26 August 2026):
* `backend/opt/tdlib-native/win32-x64/tdjson.dll` (27.5 MB)
* `backend/opt/tdlib-native/win32-x64/libcrypto-3-x64.dll`
* `backend/opt/tdlib-native/win32-x64/libssl-3-x64.dll`
* `backend/opt/tdlib-native/win32-x64/z.dll`

### 2.2 Telegram Communities support at the core level
The TDLib 1.8.67 binary natively includes the Communities entities:
* `updateCommunity`, `updateCommunityFullInfo`, `loadCommunityFullInfo`
* `dialogPeerCommunity`
* `linked_community_id`, `on_update_channel_linked_community_id`

---

## 3. Build automation (`backend/toolchain/build.rs`)

The build file `backend/toolchain/build.rs` integrates the `copy_native_binaries` procedure:
* Scans the directories:
  * `backend/opt/tdlib-native/win32-x64/`
  * `backend/opt/libmpv/`
  * `backend/opt/xray/`
* For each `*.dll` and `*.exe` file, checks existence and timestamps during the build.
* If a file is missing or outdated, it is automatically copied into the target directory (`target/release/` or `target/debug/`) next to `shell.exe`.
* The manifest `backend/toolchain/tauri.conf.json` was updated with the new OpenSSL 3 and zlib libraries.

---

## 4. Backend implementation (`backend/modules/telegram/telegram.rs`)

### 4.1 Detecting supergroup forums and communities
In the TDLib JSON API, forums do not contain a `chat.is_forum` field. The current property is `chat.view_as_topics: bool`.
In `parse_chat`:
```rust
let is_forum = chat
    .get("view_as_topics")
    .and_then(Value::as_bool)
    .or_else(|| chat_type.and_then(|t| t.get("is_forum")).and_then(Value::as_bool))
    .or_else(|| chat.get("is_forum").and_then(Value::as_bool))
    .unwrap_or(false);
```
The `linked_community_id` field and the `has_subchats` flag (indicating the presence of subchats) are also extracted.

### 4.2 Fallback for community subchats in `get_forum_topics`
When the user opens a community (for example the channel `Re:Zero | Элиор`), a direct TDLib `getForumTopics` request returns `400: The chat is not a forum`, because this is a community entity.
`get_forum_topics` implements an intelligent fallback:
* If `getForumTopics` returns an error or an empty array, the core checks the loaded chat database `inner.chats`.
* It finds all channels and groups linked to the parent community:
  * `c.linked_community_id == parent_id`
  * A match of the community prefix/tag (e.g. the chat `Ледяной лес Элиор` and the channel `Re:Zero / Аниме 3 сезон`).
* The found subchats are wrapped into the `forumTopics` format, passing `subchat_id`:
```rust
json!({
    "info": {
        "message_thread_id": 0,
        "name": c.title,
        "subchat_id": c.id,
    },
    "unread_count": c.unread,
    "last_message": {
        "content": { "text": { "text": c.preview } }
    }
})
```

---

## 5. Frontend implementation (`widget.js` & `styles.css`)

### 5.1 Three-column grid
`frontend/styles.css` defines the `.tg-layout.forum-topics` mode:
* Grid: `grid-template-columns: 46px 200px minmax(0, 1fr)`.
* **Column 1 (`.tg-chat-list`)**: shrinks to 46px, chat rows are centered, the text blocks (`.tg-row-main`, `.tg-row-unread`, `.tg-row-time`) are hidden, leaving a compact rail of avatars in the scrollable chat list.
* **Column 2 (`.tg-topics-list`)**: 200px wide. Contains a header with the button `← Все чаты` ("← All chats", `data-action="back-to-chats"`), the name of the parent group/community, and an interactive list of topics/subchats.
* **Column 3 (`.tg-thread`)**: takes all remaining width. It is not reset when topics are opened, which keeps the reading flow continuous.

### 5.2 Caching messages per topic
* Messages are cached under the key `chatId::threadId` (for topics) and `chatId` (for subchats and ordinary chats).
* When a topic is selected, `refreshTelegramMessages` receives an integer `threadId`, and the result is stored in an isolated cache segment.

### 5.3 Back navigation
* Clicking the `← Все чаты` ("← All chats") button resets `telegramRuntime.forumOpenChatId = null`.
* The widget grid returns to the two-column view (`.forum-topics` is removed), and the chat list expands back.

---

## 6. Automatic dependency updates (`scripts/update-deps.ps1`)

A script for automatic updating of the native binaries was created:

```powershell
# Check versions only:
.\scripts\update-deps.ps1 -CheckOnly

# Update TDLib from NuGet:
.\scripts\update-deps.ps1 -Component tdlib

# Update every component (TDLib, libmpv, Xray), then run shell doctor:
.\scripts\update-deps.ps1 -All
```

### Update sources:
1. **TDLib**: official NuGet `tdlib.native.win-x64` -> `backend/opt/tdlib-native/win32-x64/`.
2. **libmpv**: GitHub Releases `shinchiro/mpv-winbuild-cmake` -> `backend/opt/libmpv/`.
3. **Xray**: GitHub Releases `XTLS/Xray-core` -> `backend/opt/xray/`.

---

## 7. Verification protocol and results

Verification was performed according to the 5-step end-to-end check protocol:

| Stage | Command / Method | Result |
| :--- | :--- | :--- |
| **1. Static check** | `cargo check`, `node --check` | No errors (0 warnings) |
| **2. Full build** | `cargo build --release` | Exit code 0 (2m 30s) |
| **3. Runtime diagnostics** | `shell doctor` | All DLLs found, IPC active |
| **4. Forum testing** | `shell act click "qbc"` -> `shell act click "General"` | 4 topics opened in the column, messages of the General thread loaded |
| **5. Community testing** | `shell act click "Re:Zero \| Элиор"` -> `shell act click "Ледяной лес Элиор"` | 3 community channels shown in the column, the subchat `Ледяной лес Элиор` opened |
| **6. Back navigation** | `shell act click "Назад к чатам"` | Smooth closing of the topic column, return to the chat list |
