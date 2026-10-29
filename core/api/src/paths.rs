//! Пути UniDTK: где лежит **программа** и где лежат **данные пользователя**.
//!
//! Разделение ровно по правилу Windows:
//!
//! * **программа** — `C:\Program Files\UniDTK`: исполняемые файлы и нативные
//!   зависимости (`unidtk.exe`, `udtk.exe`, `vpn-helper.exe`, `sing-box.exe`,
//!   DLL, модели AI DJ). Каталог доступен только на чтение — приложение не
//!   имеет права писать рядом с собой **ничего**;
//! * **данные** — `%LOCALAPPDATA%\UniDTK`: базы, журналы, кэш, сессия Telegram,
//!   библиотека музыки. Всё изменяемое живёт только здесь.
//!
//! Почему это контракт, а не знание ядра: чистые функции от окружения, поэтому
//! и ядро, и любой модуль получают один и тот же ответ. Модулю не нужно знать
//! раскладку, а переезд каталогов не задевает ни один модуль.

use std::path::PathBuf;

/// Каталог программы: рядом с `unidtk.exe`.
///
/// Отсюда только **читают** (DLL, sing-box, хелпер, модели). Под Program Files
/// каталог read-only, поэтому любая запись сюда — ошибка проектирования.
pub fn install_dir() -> PathBuf {
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|parent| parent.to_path_buf()))
        .unwrap_or_else(|| PathBuf::from("."))
}

/// Каталог данных пользователя: `%LOCALAPPDATA%\UniDTK`, а при недоступности
/// известных папок Windows — текущий каталог.
///
/// Разрешается через Windows known-folder API (`dirs`), а не через переменную
/// `%LOCALAPPDATA%`: задачи планировщика запускают процессы, в которых эта
/// переменная не гарантирована.
pub fn data_dir() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    let dir = base.join("UniDTK");
    let _ = std::fs::create_dir_all(&dir);
    dir
}

/// Производные данные, которые можно удалить целиком:
/// `%LOCALAPPDATA%\UniDTK\cache`. Сюда попадают хранилища модулей, объявленные
/// как [`unidtk_api::StoreKind::OwnedCache`].
pub fn cache_dir() -> PathBuf {
    data_dir().join("cache")
}

/// Журналы: `%LOCALAPPDATA%\UniDTK\logs`.
pub fn logs_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Библиотека музыки по умолчанию: `%LOCALAPPDATA%\UniDTK\music`.
///
/// Каталог можно переопределить в конфиге плеера (`library_dir`): это
/// пользовательский контент, и держать его строго внутри данных приложения —
/// решение владельца, а не требование Windows.
pub fn music_dir() -> PathBuf {
    data_dir().join("music")
}

/// Модели (ONNX AI DJ) — если приложение получило их не из установщика.
pub fn models_dir() -> PathBuf {
    data_dir().join("models")
}

/// Бинарник sing-box: **только** рядом с программой.
///
/// Фоллбэков больше нет: раньше третьим кандидатом стоял `sing-box.exe` из
/// комплекта сторонней программы (`C:\Program Files\FlyFrogLLC\Happ\...`), то
/// есть наша VPN-подсистема могла молча подняться на чужом бинарнике.
/// `sing-box.exe` обязан приходить с установщиком вместе с приложением.
pub fn singbox_exe() -> PathBuf {
    install_dir().join("sing-box.exe")
}

/// ByeDPI (`ciadpi.exe`): локальный DPI-обход для модуля telegram. Только
/// рядом с программой, без PATH-фоллбэка: приложение обязано шипать свой
/// экземпляр.
pub fn byedpi_exe() -> PathBuf {
    install_dir().join("ciadpi.exe")
}

/// Нативный VPN-хелпер: рядом с программой, как и остальные бинарники.
/// Задачи планировщика запускают его оттуда же (повышенными).
pub fn vpn_helper_exe() -> PathBuf {
    install_dir().join("vpn-helper.exe")
}

/// Пассивный просмотр пути каталога данных БЕЗ вызова create_dir_all и миграции.
/// Используется в диагностике для соблюдения правила Zero Disk Pollution.
pub fn data_dir_view() -> PathBuf {
    let base = dirs::data_local_dir().unwrap_or_else(|| PathBuf::from("."));
    base.join("UniDTK")
}

/// Канонический поиск бинарника или DLL: проверяет install_dir, target/{release,debug}
/// и папки исходных библиотек в checkout (opt/).
pub fn resolve_binary(name: &str) -> Option<PathBuf> {
    let install = install_dir();
    let direct = install.join(name);
    if direct.is_file() {
        return Some(direct);
    }

    let mut curr = install;
    for _ in 0..5 {
        for sub in ["target/release", "target/debug"] {
            let p = curr.join(sub).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        for opt in [
            "opt",
            "opt/singbox",
            "opt/byedpi",
            "opt/libmpv",
            "opt/tdlib-native/win32-x64",
        ] {
            let p = curr.join(opt).join(name);
            if p.is_file() {
                return Some(p);
            }
        }
        if !curr.pop() {
            break;
        }
    }
    None
}

/// Канонический поиск файла модели или ассета: проверяет install/models,
/// data/models и opt/models.
pub fn resolve_asset(relative_path: &str) -> Option<PathBuf> {
    let install = install_dir();
    let direct = install.join("models").join(relative_path);
    if direct.is_file() {
        return Some(direct);
    }

    let data = data_dir_view().join("models").join(relative_path);
    if data.is_file() {
        return Some(data);
    }

    let mut curr = install;
    for _ in 0..5 {
        let p = curr.join("opt/models").join(relative_path);
        if p.is_file() {
            return Some(p);
        }
        if !curr.pop() {
            break;
        }
    }
    None
}
