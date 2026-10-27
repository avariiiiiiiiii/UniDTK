# Архитектурная спецификация и план: Исправление перехвата реестра HKCU/Folder и системных апплетов

> **Статус:** Согласовано с 5 независимыми субагентами, готово к реализации  
> **Дата:** 2026-09-04  
> **Целевые подсистемы:** `backend/modules/explorer`, `backend/core`, `frontend/modules/explorer`, `frontend/core`

---

## 1. Описание проблемы и симптомы

Пользователь столкнулся с нежелательными системными перехватами при использовании оболочки:
1. **Открытие Панели управления из меню «Пуск»:** вместо системного окна Панели управления открывается Nothing Explorer.
2. **Запуск `ncpa.cpl` через `Win + R`:** вместо апплета «Сетевые подключения» открывается Nothing Explorer.
3. **Странные названия вкладок:** в проводнике появляются вкладки с именами вида `::{26EE0668-A00A-44D7-9371-BEB064C98683}\3\::{7007ACC7-3202-11D1-AAD2-00805FC1270E}`.
4. **Случайные папки:** при сбое навигации проводник Windows инициирует fallback-открытие рабочей или родительской папки (System32, Programs), которая также перехватывается.

---

## 2. Корневая причина (Root Cause Analysis)

### 2.1 Иерархия Windows Shell Namespace в реестре
В реестре Windows разделены классы объектов оболочки:
- **`HKCR\Directory` (`HKCU\Software\Classes\Directory`)**: описывает **только физические каталоги** на диске (`C:\Users`, `D:\Projects`).
- **`HKCR\Drive` (`HKCU\Software\Classes\Drive`)**: описывает **только корневые разделы дисков** (`C:\`, `D:\`).
- **`HKCR\Folder` (`HKCU\Software\Classes\Folder`)**: базовый контейнер для **ВСЕГО пространства имен Shell Namespace**. В него входят как физические каталоги, так и **виртуальные системные папки (CLSID)**:
  - Панель управления: `::{26EE0668-A00A-44D7-9371-BEB064C98683}`
  - Сетевые подключения (`ncpa.cpl`): `::{7007ACC7-3202-11D1-AAD2-00805FC1270E}`
  - Принтеры, Устройства, Шрифты, MTP-устройства (смартфоны), ZIP-архивы.

### 2.2 Где происходила ошибка в коде
В `backend/modules/explorer/src/registry.rs`:
```rust
const FOLDER_OPEN: &str = r"HKCU\Software\Classes\Folder\shell\open";
const FOLDER_OPEN_CMD: &str = r"HKCU\Software\Classes\Folder\shell\open\command";
const FOLDER_EXPLORE: &str = r"HKCU\Software\Classes\Folder\shell\explore";
const FOLDER_EXPLORE_CMD: &str = r"HKCU\Software\Classes\Folder\shell\explore\command";
```
В функции `install_as_default_file_manager()`:
```rust
set_open(FOLDER_OPEN, FOLDER_OPEN_CMD, &open_path);
set_open(FOLDER_EXPLORE, FOLDER_EXPLORE_CMD, &open_path);
```
При этом `set_open` выставляет `DelegateExecute = ""` (пустую строку), блокируя системный COM-обработчик Windows Explorer (`{11dbb47c-a525-400b-9e80-a54615a090c0}`).
Каждый раз при старте приложения в `backend/core/main.rs:1174-1179` эта функция вызывается в фоновом потоке, перезаписывая ветку `HKCU\Software\Classes\Folder`.

В результате Windows при запуске любых виртуальных папок (Панель управления, `ncpa.cpl`) вызывает:
```cmd
"shell.exe" open "::{26EE0668-A00A-44D7-9371-BEB064C98683}"
```
Бэкенд принимал этот аргумент, передавал во фронтенд через событие `explorer-open`, фронтенд создавал вкладку с названием GUID, пытался прочитать виртуальный путь через `std::fs::read_dir`, падал с ошибкой «Path does not exist», а испорченная вкладка сохранялась в базу `shell.redb`.

---

## 3. Заключения 5 независимых субагентов

### Субагент 1: Win32 & Thread Safety
- **Кэширование ассоциаций:** Проводник Windows кэширует реестровые глаголы в оперативной памяти. Простого удаления ключей в реестре недостаточно — обязательно вызвать Win32 API `SHChangeNotify(SHCNE_ASSOCCHANGED, SHCNF_IDLIST, None, None)`, иначе запущенные процессы будут использовать старый перехват до перезагрузки.
- **Очистка ключей:** Использовать `RegDeleteTreeW`, корректно обрабатывая `ERROR_FILE_NOT_FOUND` (код 2) как успех. Очищать родительскую пустую ветку `Software\Classes\Folder\shell` через `RegDeleteKeyW`.
- **Защита от Fork Bomb:** При делегации системных GUID в `explorer.exe` необходимо исключить зацикливание через предварительный сброс ассоциаций и фильтрацию известных внутренних GUID оболочки.
- **COM Apartments:** В `create_shortcut_native` вызывать `CoUninitialize()` только если `CoInitializeEx` вернул успешный код.

### Субагент 2: Win32 IPC & Message Loop
- **Критический скрытый баг в `main.rs:883`:** В текущей реализации `single_instance`, если `parse_open_arg` возвращает `None`, управление падает на строку 883:
  ```rust
  if let Some(window) = app.get_webview_window("wallpaper") {
      let _ = window.hide();
      wallpaper::attach(...);
      let _ = window.show();
  }
  ```
  Код ошибочно считает любой нераспознанный аргумент вторым запуском обоев! При попытке проигнорировать аргумент или запустить CLI обои начинают мерцать и перезакрепляться под `WorkerW`.
- **Решение:** Ввести строгий 4-позиционный enum `LaunchIntent`:
  - `OpenInShell(String)` — валидный локальный путь, Корзина или «Этот компьютер».
  - `ForwardToNativeExplorer(String)` — системный виртуальный CLSID.
  - `ReattachWallpaper` — строго пустой запуск `shell.exe` без аргументов (`argv.len() == 1`).
  - `Ignore` — прочие аргументы / флаги.

### Субагент 3: Performance & Resources
- Вызов `RegDeleteTreeW` выполняется за <0.5 мс на первом запуске и за <50 мкс на последующих (при отсутствии ключей). Нагрузка на реестр и CPU ничтожна.
- Вызов `Command::new("explorer.exe").spawn()` создает короткоживущий процесс-хелпер, который моментально передает DDE/COM-команду основному процессу `explorer.exe` и завершается.
- Не использовать синхронный `.status()` или `.output()`, чтобы не блокировать поток на 100 мс.

### Субагент 4: Frontend & DOM Regressions
- **Очистка `redb`:** В `tabs.js:restoreTabs()` при обнаружении вкладок с путями `::{` или `shell:` они должны отфильтровываться, и сразу же вызываться `await this._saveTabs()`, чтобы вычистить мусор из базы.
- **Защита входных каналов:** `openExplorer()` в `shell-core.js` и `addTab()` в `tabs.js` обязаны пропускать входящие пути через `sanitizeTabPath()`, блокируя повторное попадание GUID во вкладки.
- **Безопасность `getFolderName`:** Защитить от `null`/`undefined`, нормализовать букву диска к верхнему регистру (`C:\`), возвращать понятные имена для системных GUID вместо 38-значных шифров.
- **Отказоустойчивость `navigateToSilent`:** При ошибке чтения каталога отображать пустое состояние с сообщением об ошибке и откатываться к «Этот компьютер», предотвращая «зависший» черный экран.

### Субагент 5: Cold Logon & Desktop Icons
- **Главная ловушка плана — иконка «Этот компьютер»:** Иконка на рабочем столе является виртуальным объектом `CLSID {20D04FE0-3AEA-1069-A2D8-08002B30309D}`. Раньше она открывалась в Shell только потому, что был перехвачен `Folder`. Если `Folder` удалить и ничего не добавить взамен, двойной клик по иконке «Этот компьютер» на рабочем столе станет открывать стандартный проводник Windows.
- **Решение:** Точечно зарегистрировать именно этот CLSID в `registry.rs`:
  `HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command` -> `"shell.exe" open`
  и нормализовать его в пустую строку `""` (вид накопителей `::MY_PC::`).
- **Исправление побочных дефектов:** Отказ от `Folder` автоматически возвращает нативную поддержку MTP (подключенные телефоны) и архивов ZIP.

---

## 4. Пошаговый план реализации

### Файл 1: `backend/modules/explorer/src/registry.rs`
1. Удалить константы `FOLDER_OPEN`, `FOLDER_OPEN_CMD`, `FOLDER_EXPLORE`, `FOLDER_EXPLORE_CMD`.
2. Добавить константы для точечной регистрации «Этот компьютер»:
   ```rust
   const THIS_PC_OPEN: &str = r"HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open";
   const THIS_PC_OPEN_CMD: &str = r"HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command";
   ```
3. В `install_as_default_file_manager()`:
   - Удалить вызовы `set_open` для `FOLDER_*`.
   - Зарегистрировать `THIS_PC_OPEN` / `THIS_PC_OPEN_CMD` с командой `open_this_pc`.
   - Добавить блок активной миграции / очистки:
     ```rust
     let legacy_keys = [
         r"Software\Classes\Folder\shell\open",
         r"Software\Classes\Folder\shell\explore",
     ];
     for key in legacy_keys {
         let wide: Vec<u16> = key.encode_utf16().chain(Some(0)).collect();
         unsafe {
             let _ = RegDeleteTreeW(HKEY_CURRENT_USER, PCWSTR(wide.as_ptr()));
         }
     }
     let folder_shell: Vec<u16> = r"Software\Classes\Folder\shell".encode_utf16().chain(Some(0)).collect();
     unsafe {
         let _ = RegDeleteKeyW(HKEY_CURRENT_USER, PCWSTR(folder_shell.as_ptr()));
     }
     ```
   - Добавить оповещение оболочки:
     ```rust
     unsafe {
         windows::Win32::UI::Shell::SHChangeNotify(
             windows::Win32::UI::Shell::SHCNE_ASSOCCHANGED,
             windows::Win32::UI::Shell::SHCNF_IDLIST,
             None,
             None,
         );
     }
     ```
4. В `uninstall_as_default_file_manager()`:
   - Добавить `{20D04FE0-...}` в список удаляемых ключей.
   - Вызвать `SHChangeNotify`.

---

### Файл 2: `backend/modules/explorer/src/window.rs`
1. Определить перечисление `LaunchIntent`:
   ```rust
   #[derive(Debug, PartialEq, Eq)]
   pub enum LaunchIntent {
       OpenInShell(String),
       ForwardToNativeExplorer(String),
       ReattachWallpaper,
       Ignore,
   }
   ```
2. Реализовать функцию `parse_launch_intent(args: &[String]) -> LaunchIntent`:
   - Если `args` содержит только имя самого бинарника -> `LaunchIntent::ReattachWallpaper`.
   - Распознавать команду `open` и позиционные аргументы.
   - Проверять CLSID:
     - Корзина `{645FF040-...}` -> `OpenInShell("C:\\$Recycle.Bin")`.
     - «Этот компьютер» `{20D04FE0-...}` или Win+E `{52205FD8-...}` -> `OpenInShell(String::new())`.
     - Прочие `::{...}` или `shell:::{...}` -> `ForwardToNativeExplorer(arg)`.
   - Физические пути -> `OpenInShell(normalize_open_path(...))`.
   - Иначе -> `LaunchIntent::Ignore`.
3. Реализовать безопасный запуск системного проводника:
   ```rust
   pub fn forward_to_native_explorer(target: &str) {
       let sysroot = std::env::var("SystemRoot").unwrap_or_else(|_| r"C:\Windows".to_string());
       let explorer_exe = format!(r"{sysroot}\explorer.exe");
       crate::log_line(format!("explorer: delegating virtual target {target:?} to native explorer.exe"));
       let _ = std::process::Command::new(&explorer_exe)
           .arg(target)
           .current_dir(&sysroot)
           .spawn();
   }
   ```

---

### Файл 3: `backend/core/main.rs`
1. В `single_instance` плагине (строка 875):
   ```rust
   match explorer::window::parse_launch_intent(&argv) {
       explorer::window::LaunchIntent::OpenInShell(path) => {
           log_line(format!("single-instance: explorer open {path:?}"));
           let _ = app.emit("explorer-open", path);
       }
       explorer::window::LaunchIntent::ForwardToNativeExplorer(clsid) => {
           explorer::window::forward_to_native_explorer(&clsid);
       }
       explorer::window::LaunchIntent::ReattachWallpaper => {
           log_line("single-instance: bare launch, re-attaching wallpaper under WorkerW");
           let app_handle = app.clone();
           let _ = run_on_main_thread(&app_handle, show_and_attach_wallpaper);
       }
       explorer::window::LaunchIntent::Ignore => {
           log_line(format!("single-instance: ignored non-explorer args {argv:?}"));
       }
   }
   ```
2. В блоке холодного старта (строка 1164):
   - Использовать `parse_launch_intent(&args)`. При `ForwardToNativeExplorer` вызывать `forward_to_native_explorer`, при `OpenInShell` отправлять событие во фронтенд.

---

### Файл 4: `frontend/modules/explorer/tabs.js`
1. Добавить валидатор `sanitizeTabPath(raw)`:
   - Проверять тип, отсекать кавычки.
   - `::MY_PC::` и CLSID `{20d04fe0-...}` нормализовать в `::MY_PC::`.
   - Пути, начинающиеся с `::{`, `shell:` или сериализационный мусор (`null`, `undefined`) -> возвращать `null`.
   - Проверять наличие диска `^[A-Za-z]:[\\/]` или UNC `\\\\`.
2. В `restoreTabs()`:
   - Фильтровать сохраненные вкладки через `sanitizeTabPath`.
   - Если хотя бы одна вкладка была отброшена, сразу вызывать `await this._saveTabs()`, очищая базу `redb`.
   - При пустом результате создавать вкладку `::MY_PC::`.
3. В `addTab()`:
   - Валидировать входной путь: `let target = sanitizeTabPath(path) || '::MY_PC::';`.
4. В `getFolderName(path)`:
   - Добавить проверку на `null`/`undefined`.
   - Для известных CLSID возвращать `'This PC'`, `'Recycle Bin'`, `'Folder'`.

---

### Файл 5: `frontend/core/shell-core.js`
1. В функции `openExplorer(path)`:
   - Санитизировать входящий `path`. Если передан неподдерживаемый виртуальный CLSID, не открывать под него битую вкладку.

---

### Файл 6: `frontend/modules/explorer/explorer.js`
1. В `navigateToSilent(path)`:
   - При ошибке `scanDirectory` вызывать `this.showEmpty(true, e)` и при необходимости откатывать активную вкладку к `this.showThisPC()`, исключая зависание пустого экрана.

---

## 5. План верификации и контрольные проверки

1. **Компиляция бэкенда и CLI:**
   - `shell check` (проверка синтаксиса и типов).
   - `shell build` (релизная сборка ядра).
2. **Проверка состояния реестра через PowerShell / CMD:**
   - `reg query "HKCU\Software\Classes\Folder\shell" /s` -> ключи должны отсутствовать (ошибка 1 / File Not Found).
   - `reg query "HKCU\Software\Classes\Directory\shell\open\command"` -> `"shell.exe" open "%1"`.
   - `reg query "HKCU\Software\Classes\CLSID\{20D04FE0-3AEA-1069-A2D8-08002B30309D}\shell\open\command"` -> `"shell.exe" open`.
3. **Функциональные проверки системных апплетов:**
   - Запуск `ncpa.cpl` через `Win + R` -> открывается нативное окно «Сетевые подключения» Windows.
   - Запуск «Панели управления» из меню «Пуск» -> открывается нативная Панель управления Windows.
   - Двойной клик по обычной папке на рабочем столе -> открывается Nothing Explorer.
   - Двойной клик по диску C: -> открывается Nothing Explorer.
   - Двойной клик по иконке «Этот компьютер» -> открывается раздел накопителей Nothing Explorer (`::MY_PC::`).
   - Нажатие `Win + E` -> открывается Nothing Explorer.
4. **Проверка логов и отсутствия мусора:**
   - `shell logs --level ERROR` -> отсутствие исключений во фронтенде и бэкенде.
   - `shell eval "JSON.stringify(window.nothingExplorer?.tabManager?.tabs.map(t => t.title))"` -> отсутствие вкладок с именами `::{GUID}`.
5. **Визуальный аудит:**
   - `shell shot` -> скриншот экрана через CLI для проверки корректности верстки и отсутствия сломанных виджетов.
