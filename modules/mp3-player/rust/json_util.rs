use serde_json::Value;

pub fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter().find_map(|key| {
        value
            .get(*key)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToString::to_string)
    })
}

pub fn int_field(value: &Value, keys: &[&str]) -> Option<i64> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_i64))
}

pub fn thumbnail_url(value: &Value) -> Option<String> {
    let thumbs = value.get("thumbnails").and_then(Value::as_array)?;
    thumbs
        .iter()
        .rev()
        .find_map(|thumb| string_field(thumb, &["url"]))
}
