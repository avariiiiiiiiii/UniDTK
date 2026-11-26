//! The menu's own pane for a module's declared settings (`r[modules.settings-contract]`):
//! the fields come from what the module declares (`settings_get`), every write goes
//! through `settings_set` with the version it read, and a refusal shows under its field.
//! A module that also registers a pane of its own (`WidgetType::settings`) gets it drawn
//! below these fields, as its live section ([`Composite`]).

use std::collections::HashMap;

use rune::el::{col, el, row, text, El};
use rune::TextEdit;
use serde_json::{json, Value};

use crate::controls::{self, ButtonKind, Look};
use crate::settings::{Act, Settings, View};

/// The refusal for a write from an old read (`unidtk_api::settings::STALE`); seeing it,
/// the pane reads again instead of showing it.
const STALE_PREFIX: &str = "Настройку только что изменили";

#[derive(Default, PartialEq)]
enum Load {
    #[default]
    Loading,
    Failed,
    Ready,
}

/// One declared setting, as `settings_get` describes it.
struct Spec {
    key: String,
    label: String,
    summary: String,
    kind: Value,
    secret: bool,
}

#[derive(Default)]
pub struct GenericPane {
    module: String,
    load: Load,
    specs: Vec<Spec>,
    /// `key` → (version, value).
    values: HashMap<String, (u64, Value)>,
    /// Text being typed: a Text or Number field, or a list's new row.
    edits: HashMap<String, TextEdit>,
    /// The key whose write is in flight: one at a time, so a quick second edit does not
    /// race the first one's version.
    saving: Option<String>,
    /// A refusal, under its field.
    errors: HashMap<String, String>,
}

impl GenericPane {
    pub fn new(module: &str) -> Self {
        Self { module: module.to_string(), ..Self::default() }
    }

    fn fill(&mut self, reply: &Value) {
        self.specs = reply["specs"].as_array().into_iter().flatten().filter_map(|spec| {
            Some(Spec {
                key: spec["key"].as_str()?.to_string(),
                label: spec["label"].as_str().unwrap_or_default().to_string(),
                summary: spec["summary"].as_str().unwrap_or_default().to_string(),
                kind: spec.clone(),
                secret: spec["secret"] == true,
            })
        }).collect();
        self.values = reply["values"].as_object().into_iter().flatten().map(|(key, stored)| {
            (key.clone(), (stored["version"].as_u64().unwrap_or(0), stored["value"].clone()))
        }).collect();
        self.edits.clear();
        for spec in &self.specs {
            let value = self.values.get(&spec.key).map(|(_, v)| v.clone()).unwrap_or(Value::Null);
            let text = match spec.kind["kind"].as_str() {
                Some("text") => value.as_str().unwrap_or_default().to_string(),
                Some("number") => value.as_i64().map(|n| n.to_string()).unwrap_or_default(),
                _ => String::new(),
            };
            self.edits.insert(spec.key.clone(), TextEdit::new(text));
        }
        self.errors.clear();
    }

    fn reload(&mut self, act: &mut Act) {
        self.load = Load::Loading;
        act.call("settings_get", json!({ "module": self.module }), |pane: &mut Self, result, _| {
            pane.load = match result {
                Ok(reply) if reply["specs"].is_array() => {
                    pane.fill(&reply);
                    Load::Ready
                }
                _ => Load::Failed,
            };
        });
    }

    fn value(&self, key: &str) -> Value {
        self.values.get(key).map(|(_, value)| value.clone()).unwrap_or(Value::Null)
    }

    /// Stores `value` for `key` with the version this pane read.
    fn save(&mut self, key: &str, value: Value, act: &mut Act) {
        if self.load != Load::Ready || self.saving.is_some() {
            return;
        }
        let version = self.values.get(key).map(|(version, _)| *version).unwrap_or(0);
        self.saving = Some(key.to_string());
        self.errors.remove(key);
        let key = key.to_string();
        act.call(
            "settings_set",
            json!({ "module": self.module, "key": key, "value": value, "expectedVersion": version }),
            move |pane: &mut Self, result, act| {
                pane.saving = None;
                match result {
                    Ok(stored) => {
                        let value = stored["value"].clone();
                        pane.values.insert(key.clone(), (stored["version"].as_u64().unwrap_or(0), value.clone()));
                        match value {
                            Value::String(text) => { pane.edits.insert(key, TextEdit::new(text)); }
                            Value::Number(number) => { pane.edits.insert(key, TextEdit::new(number.to_string())); }
                            Value::Array(_) => { pane.edits.insert(key, TextEdit::default()); }
                            _ => {}
                        }
                    }
                    Err(error) if error.starts_with(STALE_PREFIX) => pane.reload(act),
                    Err(error) => { pane.errors.insert(key, error); }
                }
            },
        );
    }

    fn submit_text(&mut self, key: &str, act: &mut Act) {
        let Some(spec) = self.specs.iter().find(|s| s.key == key) else { return };
        let typed = self.edits.get(key).map(|e| e.text.trim().to_string()).unwrap_or_default();
        let value = match spec.kind["kind"].as_str() {
            Some("number") => match typed.parse::<i64>() {
                Ok(number) => json!(number),
                Err(_) => {
                    self.errors.insert(key.to_string(), format!("«{}»: нужно число", spec.label));
                    return;
                }
            },
            _ => json!(typed),
        };
        self.save(key, value, act);
    }

    fn add_row(&mut self, key: &str, act: &mut Act) {
        let typed = self.edits.get(key).map(|e| e.text.trim().to_string()).unwrap_or_default();
        if typed.is_empty() {
            return;
        }
        let mut rows = self.value(key).as_array().cloned().unwrap_or_default();
        rows.push(json!(typed));
        self.save(key, Value::Array(rows), act);
    }

    fn remove_row(&mut self, key: &str, index: usize, act: &mut Act) {
        let mut rows = self.value(key).as_array().cloned().unwrap_or_default();
        if index < rows.len() {
            rows.remove(index);
            self.save(key, Value::Array(rows), act);
        }
    }

    fn field(&self, view: &View<'_>, spec: &Spec) -> Option<El> {
        let look = view.look;
        let key = spec.key.clone();
        let busy = self.saving.is_some();
        let body = match spec.kind["kind"].as_str()? {
            "switch" => {
                let on = self.value(&key) == true;
                let toggle = controls::switch(look, format!("hit:setting-{key}"), on, view.on(move |pane: &mut Self, act| {
                    let next = pane.value(&key) != true;
                    pane.save(&key, json!(next), act);
                }))
                .role("switch")
                .label(spec.label.clone())
                .checked(on);
                return Some(self.with_error(look, &spec.key, switch_row(look, &spec.label, &spec.summary, toggle)));
            }
            "text" | "number" => {
                let edit = self.edits.get(&key)?;
                let placeholder = spec.kind["placeholder"].as_str().unwrap_or_default().to_string();
                let (k1, k2) = (key.clone(), key.clone());
                let field = well(look, input(look, edit, &spec.label, &placeholder, spec.secret, format!("hit:setting-{key}"))
                    .on_edit(view.edit(move |pane: &mut Self| pane.edits.entry(k1.clone()).or_default()))
                    .on_submit(view.on(move |pane: &mut Self, act| pane.submit_text(&k2, act))));
                col().gap(6.0).child(controls::field_label(look, &spec.label, "")).child(field)
            }
            "choice" => {
                let options: Vec<(String, String)> = spec.kind["options"].as_array().into_iter().flatten()
                    .filter_map(|pair| Some((pair[0].as_str()?.to_string(), pair[1].as_str()?.to_string())))
                    .collect();
                let refs: Vec<(&str, &str)> = options.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
                let checked = self.value(&key).as_str().unwrap_or_default().to_string();
                let chosen = key.clone();
                let pills = controls::pills(look, &format!("hit:setting-{key}"), &refs, &checked,
                    view.on_value(move |pane: &mut Self, value: &str, act| pane.save(&chosen, json!(value), act)));
                col().gap(6.0).child(controls::field_label(look, &spec.label, "")).child(pills)
            }
            "text_list" => {
                let rows = self.value(&key).as_array().cloned().unwrap_or_default();
                let mut list = col().gap(4.0);
                for (index, row_value) in rows.iter().enumerate() {
                    let shown = row_value.as_str().unwrap_or_default().to_string();
                    let remove_key = key.clone();
                    let mut line = text(shown).font(look.font(13.0, 400.0)).color(look.fg).single_line().truncate().grow(1.0).min_w(0.0);
                    if spec.secret {
                        line = line.secure().revealed(true);
                    }
                    list = list.child(
                        row().w_full().gap(8.0).items_center().px(10.0).py(6.0).radius(9.0).fill(look.well.alpha(0.18))
                            .child(line)
                            .child(controls::small_button(look, format!("hit:setting-{key}-remove-{index}"), "×", ButtonKind::Normal, view.on(move |pane: &mut Self, act| pane.remove_row(&remove_key, index, act)))),
                    );
                }
                let edit = self.edits.get(&key)?;
                let placeholder = spec.kind["placeholder"].as_str().unwrap_or_default().to_string();
                let (k1, k2, k3) = (key.clone(), key.clone(), key.clone());
                let add_field = well(look, input(look, edit, &spec.label, &placeholder, spec.secret, format!("hit:setting-{key}-new"))
                    .on_edit(view.edit(move |pane: &mut Self| pane.edits.entry(k1.clone()).or_default()))
                    .on_submit(view.on(move |pane: &mut Self, act| pane.add_row(&k2, act))));
                col().gap(6.0)
                    .child(controls::field_label(look, &spec.label, ""))
                    .child(list)
                    .child(add_field)
                    .child(row().child(controls::button(look, format!("hit:setting-{key}-add"), if busy { "…" } else { "Добавить" }, ButtonKind::Primary, view.on(move |pane: &mut Self, act| pane.add_row(&k3, act)))))
            }
            // A record is the module's: its own pane section draws it.
            _ => return None,
        };
        Some(self.with_error(look, &spec.key, el().w_full().py(8.0).child(body)))
    }

    fn with_error(&self, look: &Look, key: &str, field: El) -> El {
        match self.errors.get(key) {
            Some(error) => col().w_full().gap(4.0).child(field)
                .child(text(error.clone()).key(format!("setting-error-{key}")).font(look.font(12.0, 400.0)).color(look.status_error).wrap()),
            None => field,
        }
    }
}

impl Settings for GenericPane {
    fn load(&mut self, act: &mut Act) {
        self.reload(act);
    }

    fn pane(&self, view: &View<'_>) -> El {
        let look = view.look;
        match self.load {
            Load::Loading => el().px(4.0).py(16.0).child(text("Загрузка…").font(look.font(13.0, 400.0)).color(look.fg.alpha(0.78))),
            // Never editable fields over a failed read: a write would replace what is stored.
            Load::Failed => col().px(4.0).py(16.0).gap(10.0)
                .child(text("Не удалось загрузить настройки.").font(look.font(13.0, 400.0)).color(look.status_error).wrap())
                .child(row().child(controls::button(look, "hit:settings-retry".into(), "Повторить", ButtonKind::Normal, view.on(|pane: &mut Self, act| pane.reload(act))))),
            Load::Ready => {
                let mut pane = col().w_full();
                for spec in &self.specs {
                    if let Some(field) = self.field(view, spec) {
                        pane = pane.child(field);
                    }
                }
                pane
            }
        }
    }
}

/// The declared fields, then the module's own section.
pub struct Composite {
    pub generic: GenericPane,
    pub live: Option<Box<dyn Settings>>,
}

impl Settings for Composite {
    fn load(&mut self, act: &mut Act) {
        self.generic.load(act);
        if let Some(live) = self.live.as_mut() {
            live.load(act);
        }
    }

    fn pane(&self, view: &View<'_>) -> El {
        let mut pane = col().w_full().child(self.generic.pane(view));
        if let Some(live) = &self.live {
            pane = pane.child(live.pane(view));
        }
        pane
    }
}

fn input(look: &Look, edit: &TextEdit, label: &str, placeholder: &str, secret: bool, key: String) -> El {
    let field = controls::text_field(edit, look.font(13.0, 400.0), look.fg)
        .single_line()
        .key(key)
        .label(label.to_string())
        .placeholder(placeholder.to_string(), look.fg.alpha(0.55));
    if secret { field.secure().revealed(true) } else { field }
}

fn well(look: &Look, field: El) -> El {
    el().w_full().min_h(32.0).px(10.0).py(6.0).radius(9.0)
        .border(1.0, look.elev.alpha(0.16)).border_layout()
        .fill(look.well.alpha(0.25))
        .child(field)
}

fn switch_row(look: &Look, name: &str, sub: &str, switch: El) -> El {
    let mut texts = col().grow(1.0).min_w(0.0).gap(2.0)
        .child(text(name.to_string()).font(look.font(13.0, 400.0)).color(look.fg).single_line());
    if !sub.is_empty() {
        texts = texts.child(text(sub.to_string()).font(look.font(12.0, 400.0)).color(look.fg.alpha(0.72)).single_line());
    }
    row().w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center()
        .hover(look.elev.alpha(0.07))
        .child(texts)
        .child(switch)
}
