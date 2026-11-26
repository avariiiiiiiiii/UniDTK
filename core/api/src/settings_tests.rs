use super::*;
use serde_json::json;

const SPECS: &[SettingSpec] = &[
    SettingSpec { key: "auto", label: "Авто", summary: "", kind: SettingKind::Switch { default: false }, secret: false },
    SettingSpec { key: "name", label: "Имя", summary: "", kind: SettingKind::Text { placeholder: "", max_chars: 5 }, secret: false },
    SettingSpec { key: "port", label: "Порт", summary: "", kind: SettingKind::Number { min: 1, max: 65535, default: 443 }, secret: false },
    SettingSpec { key: "mode", label: "Режим", summary: "", kind: SettingKind::Choice { options: &[("a", "А"), ("b", "Б")], default: "a" }, secret: false },
    SettingSpec { key: "links", label: "Ссылки", summary: "", kind: SettingKind::TextList { placeholder: "", max_items: 2, max_chars: 10 }, secret: true },
    SettingSpec { key: "home", label: "Дом", summary: "", kind: SettingKind::Record, secret: true },
];

fn spec(key: &str) -> &'static SettingSpec {
    SPECS.iter().find(|s| s.key == key).unwrap()
}

#[test]
fn every_kind_has_a_default() {
    assert_eq!(spec("auto").default_value(), json!(false));
    assert_eq!(spec("name").default_value(), json!(""));
    assert_eq!(spec("port").default_value(), json!(443));
    assert_eq!(spec("mode").default_value(), json!("a"));
    assert_eq!(spec("links").default_value(), json!([]));
    assert_eq!(spec("home").default_value(), json!({}));
}

#[test]
fn a_value_is_checked_against_its_kind() {
    assert_eq!(spec("auto").check(json!(true)).unwrap(), json!(true));
    assert!(spec("auto").check(json!("yes")).is_err());
    assert_eq!(spec("name").check(json!(" ab ")).unwrap(), json!("ab"));
    assert!(spec("name").check(json!("abcdef")).is_err());
    assert!(spec("port").check(json!(0)).is_err());
    assert!(spec("port").check(json!(70000)).is_err());
    assert_eq!(spec("port").check(json!(8443)).unwrap(), json!(8443));
    assert!(spec("mode").check(json!("c")).is_err());
    // Rows are trimmed and empty rows dropped; too many or too long is refused.
    assert_eq!(spec("links").check(json!([" x ", "", "y"])).unwrap(), json!(["x", "y"]));
    assert!(spec("links").check(json!(["a", "b", "c"])).is_err());
    assert!(spec("links").check(json!(["01234567890"])).is_err());
    assert!(spec("home").check(json!([1])).is_err());
    assert_eq!(spec("home").check(json!({ "k": 1 })).unwrap(), json!({ "k": 1 }));
}

#[test]
fn a_refusal_never_repeats_a_secret_value() {
    let error = spec("links").check(json!(["SeCrEt-long-value"])).unwrap_err();
    assert!(!error.contains("SeCrEt"), "{error}");
}

#[test]
fn the_in_memory_store_keeps_versions_and_refuses_a_stale_write() {
    let store = InMemorySettings::new(SPECS);
    let fresh = store.get("auto").unwrap();
    assert_eq!((fresh.version, fresh.value.clone()), (0, json!(false)));
    let first = store.set("auto", json!(true), Some(0)).unwrap();
    assert_eq!(first.version, 1);
    assert!(store.set("auto", json!(false), Some(0)).is_err(), "a write from an old read is refused");
    // An unchanged value keeps its version: nothing that watches it moves.
    assert_eq!(store.set("auto", json!(true), Some(1)).unwrap().version, 1);
    assert_eq!(store.set("auto", json!(false), None).unwrap().version, 2);
    assert!(store.set("unknown", json!(1), None).is_err());
    assert!(store.set("port", json!("x"), None).is_err());
}
