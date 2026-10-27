/// Parse `unidtk.exe open [path]` (and explorer-style `/e,` `/select,`).
/// `Some("")` → This PC, `Some(path)` → folder, `None` → not an explorer launch.
pub fn parse_open_arg(args: &[String]) -> Option<String> {
    let rest: Vec<&str> = args
        .iter()
        .skip(1)
        .map(String::as_str)
        .filter(|s| !s.starts_with("--"))
        .collect();
    if rest.is_empty() {
        return None;
    }
    if rest[0].eq_ignore_ascii_case("open") {
        if rest.len() >= 2 {
            return Some(normalize_open_path(rest[1]));
        }
        return Some(String::new());
    }
    if looks_like_path(rest[0]) {
        return Some(normalize_open_path(rest[0]));
    }
    None
}

fn normalize_open_path(raw: &str) -> String {
    let mut s = raw.trim().trim_matches('"').to_string();
    let lower = s.to_ascii_lowercase();
    if lower.contains("{645ff040-5081-101b-9f08-00aa002f954e}") {
        let drive = std::env::var("SystemDrive").unwrap_or_else(|_| "C:".to_string());
        return format!(r"{}\$Recycle.Bin", drive.trim_end_matches('\\'));
    }
    if let Some(rest) = lower.strip_prefix("/select,") {
        let path = raw.trim().trim_matches('"');
        let idx = path.len().saturating_sub(rest.len());
        let selected = path[idx..].trim().trim_matches('"');
        if let Some(parent) = std::path::Path::new(selected).parent() {
            return parent.to_string_lossy().into_owned();
        }
        return selected.to_string();
    }
    if let Some(rest) = lower.strip_prefix("/e,") {
        let path = raw.trim().trim_matches('"');
        let idx = path.len().saturating_sub(rest.len());
        return path[idx..].trim().trim_matches('"').to_string();
    }
    if s.ends_with('\\') && s.len() > 3 {
        s.pop();
    }
    s
}

fn looks_like_path(s: &str) -> bool {
    let s = s.trim().trim_matches('"');
    if s.len() >= 2 && s.as_bytes().get(1) == Some(&b':') {
        return true;
    }
    let lower = s.to_ascii_lowercase();
    lower.starts_with("\\\\")
        || lower.starts_with("/select,")
        || lower.starts_with("/e,")
        || lower.starts_with("::{")
}
