# Архитектура топиков форумов, сообществ Telegram и управления зависимостями

> **Статус документа:** Принято и внедрено  
> **Дата:** 2026-09-01  
> **Связанные компоненты:** `backend/modules/telegram`, `frontend/modules/telegram`, `backend/toolchain/build.rs`, `backend/cli`

---

## 1. Контекст и проблематика

В предыдущей версии виджета Telegram в Shell группы с топиками (форумы TDLib) и сообщества каналов/групп (Community Hubs, например `Re:Zero | Элиор`) отображались как обычные плоские чаты.

### Требования пользователя:
1. **Форумы и топики**: При клике на группу с топиками левый список чатов должен сворачиваться в компактную полосу аватаров (rail), освобождая место под колонку топиков (200px), при этом правая область сообщений (тред) не должна сбрасываться или перезагружаться.
2. **Сообщества каналов (Communities)**: Сообщества (как `Re:Zero | Элиор`), объединяющие несколько связанных каналов и групп, должны работать аналогично топикам: центральная колонка отображает список связанных подчатов (например, `Re:Zero / Аниме 3 сезон`, `Re:Zero | Элиор`, `Ледяной лес Элиор`), клик по которым переключает активный тред на этот канал.
3. **Запрет на ручные копирования**: Все бинарные зависимости (`tdjson.dll`, `libmpv-2.dll`, `xray.exe`) хранятся в канонической папке `backend/opt/` и автоматически переносятся сборочной системой `build.rs` в `target/release/` при вызове `cargo build`.
4. **Механизм обновления зависимостей**: Наличие единого автоматизированного инструмента для проверки и обновления нативных библиотек.

---

## 2. Обновление TDLib до версии 1.8.67

### 2.1 Нативные бинарники
Из официального NuGet-репозитория `tdlib.native.win-x64` (версия 1.8.67 от 26 августа 2026 г.) загружены актуальные 64-битные библиотеки Windows:
* `backend/opt/tdlib-native/win32-x64/tdjson.dll` (27.5 МБ)
* `backend/opt/tdlib-native/win32-x64/libcrypto-3-x64.dll`
* `backend/opt/tdlib-native/win32-x64/libssl-3-x64.dll`
* `backend/opt/tdlib-native/win32-x64/z.dll`

### 2.2 Поддержка Telegram Communities на уровне ядра
Бинарный файл TDLib 1.8.67 нативно включает в себя сущности Communities:
* `updateCommunity`, `updateCommunityFullInfo`, `loadCommunityFullInfo`
* `dialogPeerCommunity`
* `linked_community_id`, `on_update_channel_linked_community_id`

---

## 3. Автоматизация сборки (`backend/toolchain/build.rs`)

В файл сборки `backend/toolchain/build.rs` интегрирована процедура `copy_native_binaries`:
* Сканирует директории:
  * `backend/opt/tdlib-native/win32-x64/`
  * `backend/opt/libmpv/`
  * `backend/opt/xray/`
* При сборке проверяет наличие и временные метки каждого файла `*.dll` и `*.exe`.
* Если файл отсутствует или обновлен, он автоматически копируется в целевую директорию (`target/release/` или `target/debug/`) рядом с `shell.exe`.
* Манифест `backend/toolchain/tauri.conf.json` обновлен новыми библиотеками OpenSSL 3 и zlib.

---

## 4. Бэкенд-реализация (`backend/modules/telegram/telegram.rs`)

### 4.1 Распознавание супергрупп-форумов и сообществ
В TDLib JSON API форумы не содержат поля `chat.is_forum`. Актуальное свойство — `chat.view_as_topics: bool`.
В `parse_chat`:
```rust
let is_forum = chat
    .get("view_as_topics")
    .and_then(Value::as_bool)
    .or_else(|| chat_type.and_then(|t| t.get("is_forum")).and_then(Value::as_bool))
    .or_else(|| chat.get("is_forum").and_then(Value::as_bool))
    .unwrap_or(false);
```
Также извлекаются поля `linked_community_id` и флаг наличия подчатов `has_subchats`.

### 4.2 Fallback для подчатов сообществ в `get_forum_topics`
Когда пользователь открывает сообщество (например, канал `Re:Zero | Элиор`), прямой запрос TDLib `getForumTopics` возвращает `400: The chat is not a forum`, поскольку это сущность сообщества.
В `get_forum_topics` реализован интеллектуальный fallback:
* Если `getForumTopics` возвращает ошибку или пустой массив, ядро проверяет базу загруженных чатов `inner.chats`.
* Находятся все каналы и группы, связанные с родительским сообществом:
  * `c.linked_community_id == parent_id`
  * Совпадение префикса/тега сообщества (например, чат `Ледяной лес Элиор` и канал `Re:Zero / Аниме 3 сезон`).
* Найденные подчаты оборачиваются в формат `forumTopics` с передачей `subchat_id`:
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

## 5. Фронтенд-реализация (`widget.js` & `styles.css`)

### 5.1 Трехколоночная сетка
В `frontend/styles.css` определен режим `.tg-layout.forum-topics`:
* Сетка: `grid-template-columns: 46px 200px minmax(0, 1fr)`.
* **Колонка 1 (`.tg-chat-list`)**: сжимается до 46px, строки чатов центрируются, текстовые блоки (`.tg-row-main`, `.tg-row-unread`, `.tg-row-time`) скрываются, остаётся компактный рейл аватаров скроллируемого списка чатов.
* **Колонка 2 (`.tg-topics-list`)**: ширина 200px. Содержит шапку с кнопкой «← Все чаты» (`data-action="back-to-chats"`), названием родительской группы/сообщества и интерактивным списком топиков/подчатов.
* **Колонка 3 (`.tg-thread`)**: занимает всю оставшуюся ширину. Не сбрасывается при открытии топиков, сохраняя непрерывность восприятия.

### 5.2 Кэширование сообщений по топикам
* Сообщения кэшируются под ключом `chatId::threadId` (для топиков) и `chatId` (для подчатов и обычных чатов).
* При выборе топика в `refreshTelegramMessages` передается целочисленный `threadId`, а результат сохраняется в изолированном сегменте кэша.

### 5.3 Навигация назад
* Клик на кнопку «← Все чаты» сбрасывает `telegramRuntime.forumOpenChatId = null`.
* Сетка виджета возвращается в двухколоночный вид (`.forum-topics` удаляется), а список чатов разворачивается обратно.

---

## 6. Авто-обновление зависимостей (`scripts/update-deps.ps1`)

Создан скрипт автоматического обновления нативных бинарников:

```powershell
# Только проверка актуальности версий:
.\scripts\update-deps.ps1 -CheckOnly

# Обновление TDLib из NuGet:
.\scripts\update-deps.ps1 -Component tdlib

# Обновление всех компонентов (TDLib, libmpv, Xray) + вызов shell doctor:
.\scripts\update-deps.ps1 -All
```

### Источники обновления:
1. **TDLib**: официальный NuGet `tdlib.native.win-x64` -> `backend/opt/tdlib-native/win32-x64/`.
2. **libmpv**: GitHub Releases `shinchiro/mpv-winbuild-cmake` -> `backend/opt/libmpv/`.
3. **Xray**: GitHub Releases `XTLS/Xray-core` -> `backend/opt/xray/`.

---

## 7. Протокол верификации и результаты

Верификация проведена в соответствии с 5-ступенчатым протоколом сквозного контроля:

| Этап | Команда / Метод | Результат |
| :--- | :--- | :--- |
| **1. Статический контроль** | `cargo check`, `node --check` | Без ошибок (0 предупреждений) |
| **2. Полная сборка** | `cargo build --release` | Код возврата 0 (2м 30с) |
| **3. Диагностика рантайма** | `shell doctor` | Все DLL обнаружены, IPC активен |
| **4. Тестирование форумов** | `shell act click "qbc"` -> `shell act click "General"` | 4 топика открыты в колонке, сообщения ветки General загружены |
| **5. Тестирование сообществ** | `shell act click "Re:Zero \| Элиор"` -> `shell act click "Ледяной лес Элиор"` | 3 канала сообщества отображены в колонке, открыт подчат `Ледяной лес Элиор` |
| **6. Навигация назад** | `shell act click "Назад к чатам"` | Плавное закрытие колонки топиков, возврат списка чатов |
