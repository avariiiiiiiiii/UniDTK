//! The menu's "Вид" pane (`renderLook` in `menu.js`): the theme cards, the accent swatches and
//! "Перемешать цвета" (jelly), and "Тонкая настройка" with the pill choices, the thirteen
//! sliders, the palette and tint map (jelly), the remaining colours, the user's own themes and
//! "Размеры виджетов как в теме". A slider previews while dragged and commits on release; every
//! other control commits at once, as `setLiveToken` / `commitLiveChange` do.

use rune::el::{col, el, row, text, text_input, El};
use rune::{Color, Tx};
use serde_json::{json, Value};

use crate::controls::{self, ButtonKind, Look};
use crate::menu::{self, MenuHost};
use crate::theme_record::{self as record, SLOTS};

const SLOT_LABELS: [(&str, &str); 8] = [
    ("neutral", "Нейтральный"),
    ("blue", "Цвет 1"),
    ("aqua", "Цвет 2"),
    ("mint", "Цвет 3"),
    ("lilac", "Цвет 4"),
    ("peach", "Цвет 5"),
    ("magenta", "Цвет 6"),
    ("coral", "Коралловый"),
];

/// `PALETTE_FIELDS`: field, label, hex or "r, g, b", the material it is offered in.
const COLOR_FIELDS: [(&str, &str, bool, Option<&str>); 10] = [
    ("accent", "Акцент", true, Some("flat")),
    ("fgRgb", "Текст и значки", false, Some("flat")),
    ("bgElevRgb", "Оттенок элементов", false, None),
    ("surfaceRgb", "Фон панелей", false, None),
    ("surfaceDeepRgb", "Глубокий фон", false, None),
    ("scrimRgb", "Затемнение", false, None),
    ("statusOk", "Статус: норма", true, None),
    ("statusWarn", "Статус: внимание", true, None),
    ("statusError", "Статус: ошибка", true, None),
    ("statusInfo", "Статус: сведения", true, None),
];

struct SliderSpec {
    field: &'static str,
    label: &'static str,
    min: f32,
    max: f32,
    step: f32,
    material: Option<&'static str>,
}

/// `SLIDERS`.
const SLIDERS: [SliderSpec; 13] = [
    SliderSpec { field: "glassOpacity", label: "Плотность стекла", min: 0.4, max: 1.0, step: 0.02, material: None },
    SliderSpec { field: "blur", label: "Размытие фона", min: 0.0, max: 36.0, step: 4.0, material: None },
    SliderSpec { field: "radiusCard", label: "Скругление углов", min: 0.0, max: 28.0, step: 2.0, material: None },
    SliderSpec { field: "motionScale", label: "Длительность анимаций", min: 0.0, max: 2.0, step: 0.05, material: None },
    SliderSpec { field: "glassGrain", label: "Зерно", min: 0.0, max: 1.0, step: 0.02, material: None },
    SliderSpec { field: "glassDepth", label: "Тёмная кромка", min: 0.0, max: 1.0, step: 0.05, material: None },
    SliderSpec { field: "glassSheen", label: "Блик", min: 0.0, max: 1.0, step: 0.05, material: None },
    SliderSpec { field: "glassRim", label: "Светлый ободок", min: 0.0, max: 1.0, step: 0.05, material: None },
    SliderSpec { field: "glassShadow", label: "Тень", min: 0.0, max: 1.0, step: 0.05, material: None },
    SliderSpec { field: "shadowOffset", label: "Сдвиг тени", min: 0.0, max: 24.0, step: 1.0, material: None },
    SliderSpec { field: "inkDepth", label: "Светлота текста", min: 0.15, max: 0.38, step: 0.01, material: Some("jelly") },
    SliderSpec { field: "surfaceTint", label: "Оттенок раскрытых окон", min: 0.0, max: 1.0, step: 0.05, material: None },
    SliderSpec { field: "radiusStrip", label: "Скругление полос", min: 0.0, max: 64.0, step: 2.0, material: None },
];

/// The value a slider shows, as each `fmt` of `SLIDERS` writes it.
pub fn format_value(field: &str, ui: f32) -> String {
    let pct = |v: f32| format!("{}%", (v * 100.0).round() as i64);
    let px = |v: f32| format!("{} пикс.", trim(v));
    match field {
        "blur" | "radiusCard" | "shadowOffset" => px(ui),
        "motionScale" => if ui == 0.0 { "без анимаций".into() } else { format!("×{:.2}", ui) },
        "inkDepth" => format!("{:.2}", ui),
        "radiusStrip" => if ui >= 64.0 { "пилюля".into() } else { px(ui) },
        _ => pct(ui),
    }
}

fn trim(v: f32) -> String {
    let rounded = (v * 1000.0).round() / 1000.0;
    if rounded.fract() == 0.0 { format!("{}", rounded as i64) } else { format!("{rounded}") }
}

/// `toUi` / `fromUi`: the strip radius shows its top value as a full pill, stored as 999 px.
fn to_ui(field: &str, value: f32) -> f32 {
    if field == "radiusStrip" { value.min(64.0) } else { value }
}

fn from_ui(field: &str, ui: f32) -> f32 {
    if field == "radiusStrip" && ui >= 64.0 { 999.0 } else { ui }
}

/// `CHOICES`, then the `lookOptions` of the modules the desk draws (`draws`): a module with no
/// native half yet shows none of its options, which would change nothing on screen. Field,
/// label, options as (stored value, label).
fn choices(draws: impl Fn(&str) -> bool) -> Vec<(String, String, Vec<(Value, String)>)> {
    let mut out: Vec<(String, String, Vec<(Value, String)>)> = vec![
        ("motionProfile".into(), "Характер анимаций".into(), vec![
            (json!("responsive"), "Отзывчивый".into()), (json!("balanced"), "Сбалансированный".into()), (json!("fluid"), "Плавный".into()),
        ]),
        ("fontBody".into(), "Шрифт текста".into(), vec![
            (json!("'Geist', sans-serif"), "Geist".into()), (json!("'Segoe UI', 'Geist', sans-serif"), "Segoe UI".into()),
            (json!("system-ui, sans-serif"), "Системный".into()), (json!("'Ndot', monospace"), "Ndot".into()),
        ]),
        ("material".into(), "Материал карточек".into(), vec![(json!("flat"), "Плоское стекло".into()), (json!("jelly"), "Морское стекло".into())]),
    ];
    for module in crate::catalog::desk_modules().filter(|module| draws(&module.id)) {
        for option in module.meta.get("lookOptions").and_then(Value::as_array).into_iter().flatten() {
            let (Some(field), Some(label), Some(options)) = (option["field"].as_str(), option["label"].as_str(), option["options"].as_array()) else { continue };
            let options = options.iter().filter_map(|pair| Some((pair.get(0)?.clone(), pair.get(1)?.as_str()?.to_string()))).collect();
            out.push((field.into(), label.into(), options));
        }
    }
    out
}

/// The presets the menu offers (`getAllPresets`): the built-ins, then the user's own.
fn presets<S: MenuHost>(state: &S) -> Vec<Value> {
    let custom = state.layout().and_then(|l| l.config.get("customPresets")).and_then(Value::as_array).cloned().unwrap_or_default();
    record::builtins().into_iter()
        .chain(custom.iter().map(record::normalize).filter(|p| !record::is_retired(p)))
        .collect()
}

fn overrides<S: MenuHost>(state: &S) -> Value {
    state.layout().and_then(|l| l.config.get("colorOverrides")).cloned().unwrap_or(json!({}))
}

/// `commitLiveChange`: the record is normalized and stored, and its colours remembered for
/// its theme id.
pub fn commit<S: MenuHost>(state: &mut S, theme: &Value, tx: &mut Tx) {
    let theme = record::normalize(theme);
    let id = theme["id"].as_str().unwrap_or_default().to_string();
    state.preview_theme(theme.clone());
    state.apply(vec![
        json!({ "op": "theme", "theme": theme }),
        json!({ "op": "color_overrides", "id": id, "value": record::colors_of(&theme) }),
    ], tx);
}

fn set_field<S: MenuHost>(state: &mut S, field: &str, value: Value, tx: &mut Tx) {
    let mut theme = state.theme_record();
    theme[field] = value;
    commit(state, &theme, tx);
}

/// `selectPreset`: the preset with the user's colours for it; widget sizes follow where the
/// user kept the old preset's defaults (`migrateSizesToPreset`).
fn select_preset<S: MenuHost>(state: &mut S, id: &str, tx: &mut Tx) {
    let all = presets(state);
    let preset = all.iter().find(|p| p["id"] == id).cloned().unwrap_or_else(|| record::builtins()[0].clone());
    let found = record::normalize(&record::with_color_overrides(&preset, &overrides(state)));
    let old = state.theme_record();
    let positions = state.layout().and_then(|l| l.config.get("positions")).cloned().unwrap_or(json!({}));
    let mut ops = vec![json!({ "op": "theme", "theme": found })];
    for (widget, w, h) in record::size_moves(&positions, &found, Some(&old)) {
        ops.push(json!({ "op": "position", "id": widget, "w": w, "h": h }));
    }
    state.preview_theme(found);
    state.apply(ops, tx);
}

fn shuffle<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let current = state.theme_record();
    state.menu().previous_colors = Some(json!({ "palette": current["palette"], "tintMap": current["tintMap"] }));
    let targets = record::tint_targets(crate::catalog::desk_modules().map(|m| &m.meta));
    let rolled = record::roll_colors(record::new_color_seed(), &targets);
    apply_colors(state, &rolled, tx);
}

/// `applyColors`: the active theme with another palette and tint map, stored at once.
fn apply_colors<S: MenuHost>(state: &mut S, colors: &Value, tx: &mut Tx) {
    let mut theme = state.theme_record();
    theme["palette"] = colors["palette"].clone();
    theme["tintMap"] = colors["tintMap"].clone();
    commit(state, &theme, tx);
}

/// `restorePreviousColors`: swaps back, so a second undo would redo.
fn restore_colors<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let Some(back) = state.menu().previous_colors.take() else { return };
    let current = state.theme_record();
    state.menu().previous_colors = Some(json!({ "palette": current["palette"], "tintMap": current["tintMap"] }));
    apply_colors(state, &back, tx);
}

fn save_custom<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let name = state.menu().preset_name.text.trim().to_string();
    if name.is_empty() {
        return;
    }
    let mut preset = state.theme_record();
    preset["id"] = json!(format!("custom-{}", now_ms()));
    preset["name"] = json!(name);
    let preset = record::normalize(&preset);
    let mut list: Vec<Value> = presets(state).into_iter().filter(|p| p["isBuiltin"] != true).collect();
    list.push(preset.clone());
    state.menu().preset_name = rune::TextEdit::new("").max_chars(32);
    state.preview_theme(preset.clone());
    state.apply(vec![json!({ "op": "custom_presets", "presets": list }), json!({ "op": "theme", "theme": preset })], tx);
    state.toasts().show(format!("Тема «{name}» сохранена"), false, None);
}

fn delete_custom<S: MenuHost>(state: &mut S, id: &str, tx: &mut Tx) {
    let all = presets(state);
    let Some(preset) = all.iter().find(|p| p["id"] == id).cloned() else { return };
    let list: Vec<Value> = all.into_iter().filter(|p| p["isBuiltin"] != true && p["id"] != id).collect();
    let mut ops = vec![json!({ "op": "custom_presets", "presets": list }), json!({ "op": "color_overrides", "id": id, "value": null })];
    // The deleted preset was active: Nothing Classic.
    if state.theme_record()["id"] == id {
        let classic = record::normalize(&record::builtins()[0]);
        state.preview_theme(classic.clone());
        ops.push(json!({ "op": "theme", "theme": classic }));
    }
    state.apply(ops, tx);
    let name = preset["name"].as_str().unwrap_or_default().to_string();
    menu::undo_toast(state, format!("Тема «{name}» удалена"), Box::new(move |state: &mut S, tx: &mut Tx| {
        // `saveCustomPreset(preset.name, preset)`: the theme comes back under a new id and is applied.
        state.menu().preset_name = rune::TextEdit::new(name.clone()).max_chars(32);
        let mut back = preset.clone();
        back["id"] = json!(format!("custom-{}", now_ms()));
        let back = record::normalize(&back);
        let mut list: Vec<Value> = presets(state).into_iter().filter(|p| p["isBuiltin"] != true).collect();
        list.push(back.clone());
        state.menu().preset_name = rune::TextEdit::new("").max_chars(32);
        state.preview_theme(back.clone());
        state.apply(vec![json!({ "op": "custom_presets", "presets": list }), json!({ "op": "theme", "theme": back })], tx);
    }));
}

fn reset_sizes<S: MenuHost>(state: &mut S, tx: &mut Tx) {
    let theme = state.theme_record();
    let positions = state.layout().and_then(|l| l.config.get("positions")).cloned().unwrap_or(json!({}));
    let ops: Vec<Value> = record::size_moves(&positions, &theme, None)
        .into_iter()
        .map(|(id, w, h)| json!({ "op": "position", "id": id, "w": w, "h": h }))
        .collect();
    if !ops.is_empty() {
        state.apply(ops, tx);
    }
    state.toasts().show("Размеры виджетов — как в теме", false, None);
}

fn now_ms() -> u128 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map_or(0, |d| d.as_millis())
}

fn hex_color(value: &Value) -> Color {
    value.as_str().and_then(rune::color::hex).unwrap_or(Color::WHITE)
}

fn rgb_color(value: &Value) -> Color {
    value.as_str().map(|rgb| record::normalize_rgb(&json!(rgb)).unwrap_or_default())
        .and_then(|rgb| {
            let parts: Vec<u8> = rgb.split(", ").filter_map(|p| p.parse().ok()).collect();
            (parts.len() == 3).then(|| rune::color::hex(&format!("#{:02x}{:02x}{:02x}", parts[0], parts[1], parts[2]))).flatten()
        })
        .unwrap_or(Color::WHITE)
}

/// `.menu-theme`: a card in its own preset's colours, three swatches (the accent first) and the
/// name; jelly cards carry their sheen and rim.
fn theme_card<S: MenuHost>(look: &Look, preset: &Value, active: &Value, width: f32) -> El {
    let surface = rgb_color(&preset["surfaceRgb"]);
    let fg = rgb_color(&preset["fgRgb"]);
    let accent = hex_color(&preset["accent"]);
    let id = preset["id"].as_str().unwrap_or_default().to_string();
    let checked = preset["id"] == active["id"];
    let swatch = row().gap(6.0)
        .child(el().size(28.0, 28.0).radius(9.0).fill(accent))
        .child(el().size(28.0, 28.0).radius(9.0).fill(fg.alpha(0.18)))
        .child(el().size(28.0, 28.0).radius(9.0).fill(fg.alpha(0.18)));
    let mut card = col().key(format!("hit:preset:{id}")).grow(1.0).min_h(108.0).padding(12.0).gap(10.0).radius(16.0)
        .fill(surface)
        .border(2.0, if checked { look.accent } else { Color::TRANSPARENT }).border_layout()
        .child(swatch)
        .child(el().grow(1.0))
        .child(text(preset["name"].as_str().unwrap_or_default().to_string()).font(look.font(13.0, 600.0)).color(fg).single_line());
    card = if preset["material"] == "jelly" {
        card.radial_gradient([0.18, 0.08], [1.2 * width, 0.95 * 108.0], [(0.0, Color::WHITE.alpha(0.55)), (0.58, Color::WHITE.alpha(0.0))])
            .box_shadow(rune::Shadow { y: 1.5, color: Color::WHITE.alpha(0.8), inset: true, ..Default::default() })
            .box_shadow(rune::Shadow { y: 6.0, blur: 14.0, color: rune::color::hex("#2830462e").unwrap_or(Color::BLACK), ..Default::default() })
    } else {
        card.box_shadow(rune::Shadow { spread: 1.0, color: rune::color::hex("#80808040").unwrap_or(Color::BLACK), inset: true, ..Default::default() })
    };
    card.on_press_tx(move |state: &mut S, tx: &mut Tx| {
        let before = state.theme_record();
        let before_id = before["id"].as_str().unwrap_or_default().to_string();
        select_preset(state, &id, tx);
        if before_id != id {
            let name = state.theme_record()["name"].as_str().unwrap_or_default().to_string();
            menu::undo_toast(state, format!("Тема «{name}»"), Box::new(move |state: &mut S, tx: &mut Tx| select_preset(state, &before_id, tx)));
        }
    })
}

/// A colour well (`.menu-color`): 28 px, radius 8, the label beside it; it opens the system
/// colour dialog, as `<input type="color">` does in WebView2.
fn color_well<S: MenuHost>(look: &Look, key: String, colour: Color, label: &str, pick: impl Fn(&mut S, [u8; 3], &mut Tx) + Send + Sync + Clone + 'static) -> El {
    let initial = srgb_bytes(colour);
    row().key(key).min_h(32.0).gap(8.0).items_center().grow(1.0)
        .child(el().size(28.0, 28.0).radius(8.0).fill(colour))
        .child(text(label.to_string()).font(look.font(12.0, 400.0)).color(look.fg).single_line())
        .on_press_tx(move |state: &mut S, _tx: &mut Tx| {
            let pick = pick.clone();
            state.pick_color(initial, Box::new(move |state: &mut S, rgb, tx: &mut Tx| pick(state, rgb, tx)));
        })
}

pub fn srgb_bytes(colour: Color) -> [u8; 3] {
    let hex = colour.hex();
    [1, 3, 5].map(|i| u8::from_str_radix(&hex[i..i + 2], 16).unwrap_or(0))
}

pub fn pane<S: MenuHost>(state: &S, look: &Look) -> El {
    let active = state.theme_record();
    let jelly = active["material"] == "jelly";
    let material = active["material"].as_str().unwrap_or("flat").to_string();
    let for_material = |m: Option<&str>| m.is_none_or(|m| m == material);
    let mut pane = col().w_full();

    // Theme cards, two to a row.
    pane = pane.child(controls::heading(look, "Тема"));
    let all = presets(state);
    // Two columns 10 apart in the panel's body: its 1 px border and 16 px padding each side.
    let card_w = ((state.canvas().0 - 24.0).min(420.0) - 2.0 - 32.0 - 10.0) / 2.0;
    let mut grid = col().gap(10.0);
    for pair in all.chunks(2) {
        let mut line = row().gap(10.0);
        for preset in pair {
            line = line.child(theme_card::<S>(look, preset, &active, card_w));
        }
        if pair.len() == 1 {
            line = line.child(el().grow(1.0));
        }
        grid = grid.child(line);
    }
    pane = pane.child(grid);

    if jelly {
        pane = pane.child(controls::heading(look, "Цвета"));
        let mut seen: Vec<String> = Vec::new();
        for slot in SLOTS {
            if let Some(hex) = active["palette"][slot].as_str() {
                if !seen.iter().any(|s| s == hex) {
                    seen.push(hex.to_string());
                }
            }
        }
        let mut accents = row().gap(8.0).px(4.0).flex_wrap();
        for hex in seen {
            let checked = active["accent"].as_str().is_some_and(|a| a.eq_ignore_ascii_case(&hex));
            let colour = rune::color::hex(&hex).unwrap_or(Color::WHITE);
            let mut swatch = el().key(format!("hit:accent:{hex}")).label(format!("Акцент {hex}")).size(28.0, 28.0).radius(14.0).fill(colour).border(2.0, look.elev.alpha(0.2));
            if checked {
                // `box-shadow: 0 0 0 2px var(--bg-surface), 0 0 0 4px var(--accent)`.
                swatch = swatch
                    .box_shadow(rune::Shadow { x: 0.0, y: 0.0, blur: 0.0, spread: 2.0, color: look.surface, inset: false })
                    .box_shadow(rune::Shadow { x: 0.0, y: 0.0, blur: 0.0, spread: 4.0, color: look.accent, inset: false });
            }
            accents = accents.child(swatch.on_press_tx(move |state: &mut S, tx: &mut Tx| set_field(state, "accent", json!(hex), tx)));
        }
        pane = pane.child(accents).child(
            row().gap(8.0).py(8.0).child(controls::button(look, "hit:shuffle".into(), "Перемешать цвета", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| {
                shuffle(state, tx);
                menu::undo_toast(state, "Цвета перемешаны".into(), Box::new(|state: &mut S, tx: &mut Tx| restore_colors(state, tx)));
            })),
        );
    }

    // "Тонкая настройка".
    let fine_open = state.menu_ref().fine_open;
    let mut fine = col().w_full().mt(16.0).border_sides([1.0, 0.0, 0.0, 0.0], look.elev.alpha(0.1))
        .child(controls::summary(look, "hit:fine".into(), "Тонкая настройка", fine_open, |state: &mut S, _tx: &mut Tx| {
            let menu = state.menu();
            menu.fine_open = !menu.fine_open;
        }));
    if fine_open {
        let mut body = col().w_full();
        for (field, label, options) in choices(|id| state.draws(id)) {
            let current = active.get(&field).cloned().unwrap_or(Value::Null);
            let checked = options.iter().find(|(value, _)| value.to_string() == current.to_string()).map(|(value, _)| value.to_string()).unwrap_or_default();
            let keyed: Vec<(String, String)> = options.iter().map(|(value, label)| (value.to_string(), label.clone())).collect();
            let refs: Vec<(&str, &str)> = keyed.iter().map(|(v, l)| (v.as_str(), l.as_str())).collect();
            let field_for = field.clone();
            body = body.child(
                // `.menu-field { margin: 10px 0 }`: neighbours share one 10 px gap.
                col().pt(10.0).gap(6.0)
                    .child(controls::field_label(look, &label, ""))
                    .child(controls::pills(look, &format!("hit:choice:{field}"), &refs, &checked, move |state: &mut S, value: &str, tx: &mut Tx| {
                        let value: Value = serde_json::from_str(value).unwrap_or(Value::Null);
                        set_field(state, &field_for, value, tx);
                    })),
            );
        }
        for spec in SLIDERS.iter().filter(|s| for_material(s.material)) {
            let stored = active[spec.field].as_f64().unwrap_or(0.0) as f32;
            let ui = to_ui(spec.field, stored);
            let field = spec.field;
            body = body.child(
                // `.menu-field { margin: 10px 0 }`: neighbours share one 10 px gap.
                col().pt(10.0).gap(6.0)
                    .child(controls::field_label(look, spec.label, &format_value(field, ui)))
                    .child(controls::slider(
                        look,
                        format!("hit:slider:{field}"),
                        ui,
                        (spec.min, spec.max, spec.step),
                        move |state: &mut S, ui: f32, _tx: &mut Tx| {
                            let mut theme = state.theme_record();
                            let value = from_ui(field, ui);
                            theme[field] = json!(value);
                            if field == "radiusCard" {
                                theme["radiusSurface"] = json!(value + 8.0);
                            }
                            state.preview_theme(theme);
                        },
                        |state: &mut S, tx: &mut Tx| {
                            let theme = state.theme_record();
                            commit(state, &theme, tx);
                        },
                    )),
            );
        }
        // The last field's bottom margin.
        body = body.child(el().h(10.0));
        if jelly {
            body = body.child(controls::field_heading(look, "Палитра оттенков"));
            let mut wells = col().gap(6.0);
            for pair in SLOT_LABELS.chunks(2) {
                let mut line = row().gap(6.0);
                for (slot, label) in pair {
                    let slot = *slot;
                    line = line.child(color_well(look, format!("hit:slot:{slot}"), hex_color(&active["palette"][slot]), label, move |state: &mut S, rgb: [u8; 3], tx: &mut Tx| {
                        let mut theme = state.theme_record();
                        theme["palette"][slot] = json!(format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]));
                        commit(state, &theme, tx);
                    }));
                }
                wells = wells.child(line);
            }
            body = body.child(wells).child(controls::field_heading(look, "Каким цветом окрашено"));
            // The panels, the folders and the modules the desk draws; a module with no native half
            // yet has nothing a tint would show on.
            let targets = record::tint_targets(crate::catalog::desk_modules().filter(|m| state.draws(&m.id)).map(|m| &m.meta));
            let labels = tint_labels();
            let theme_now = crate::theme::Theme::normalize(&active);
            let mut tints = col().gap(4.0);
            for (target, default, role) in targets {
                let current = theme_now.tint_slot(&target, &role, &default);
                let label = labels.iter().find(|(id, _)| *id == target).map_or(target.clone(), |(_, l)| l.clone());
                let mut dots = row().gap(4.0);
                for slot in SLOTS {
                    let target = target.clone();
                    dots = dots.child(
                        el().key(format!("hit:tint:{target}:{slot}")).size(24.0, 24.0).radius(12.0)
                            .fill(hex_color(&active["palette"][slot]))
                            .border(2.0, if current == slot { look.fg } else { Color::TRANSPARENT })
                            .on_press_tx(move |state: &mut S, tx: &mut Tx| {
                                let mut theme = state.theme_record();
                                theme["tintMap"][target.as_str()] = json!(slot);
                                commit(state, &theme, tx);
                            }),
                    );
                }
                tints = tints.child(
                    row().w_full().justify_between().items_center().gap(8.0)
                        .child(text(label).font(look.font(12.0, 400.0)).color(look.fg).single_line())
                        .child(dots),
                );
            }
            body = body.child(tints);
        }
        body = body.child(controls::field_heading(look, "Текст, фоны и статусы"));
        let fields: Vec<_> = COLOR_FIELDS.iter().filter(|(.., m)| for_material(*m)).collect();
        let mut wells = col().gap(6.0);
        for pair in fields.chunks(2) {
            let mut line = row().gap(6.0);
            for (field, label, is_hex, _) in pair {
                let (field, is_hex) = (*field, *is_hex);
                let colour = if is_hex { hex_color(&active[field]) } else { rgb_color(&active[field]) };
                line = line.child(color_well(look, format!("hit:color:{field}"), colour, label, move |state: &mut S, rgb: [u8; 3], tx: &mut Tx| {
                    let value = if is_hex { format!("#{:02x}{:02x}{:02x}", rgb[0], rgb[1], rgb[2]) } else { format!("{}, {}, {}", rgb[0], rgb[1], rgb[2]) };
                    set_field(state, field, json!(value), tx);
                }));
            }
            if pair.len() == 1 {
                line = line.child(el().grow(1.0));
            }
            wells = wells.child(line);
        }
        body = body.child(wells).child(controls::field_heading(look, "Свои темы"));
        let name_field = text_input(&state.menu_ref().preset_name, look.font(13.0, 400.0))
            .single_line()
            .key("hit:preset-name")
            .label("Название темы")
            .placeholder("Название темы", look.fg.alpha(0.55))
            .on_edit(|state: &mut S| &mut state.menu().preset_name)
            .on_submit(|state: &mut S, tx: &mut Tx| save_custom(state, tx));
        body = body.child(
            row().gap(8.0).pt(4.0).pb(8.0)
                .child(
                    el().grow(1.0).min_h(32.0).px(10.0).py(6.0).radius(9.0)
                        .border(1.0, look.elev.alpha(0.16)).border_layout().fill(look.well.alpha(0.25))
                        .child(name_field),
                )
                .child(controls::button(look, "hit:preset-save".into(), "Сохранить текущую", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| save_custom(state, tx))),
        );
        let cross = [([18.0, 6.0], [6.0, 18.0]), ([6.0, 6.0], [18.0, 18.0])];
        for preset in presets(state).into_iter().filter(|p| p["isBuiltin"] != true) {
            let id = preset["id"].as_str().unwrap_or_default().to_string();
            body = body.child(
                row().w_full().min_h(44.0).px(8.0).py(6.0).gap(10.0).radius(12.0).items_center().hover(look.elev.alpha(0.07))
                    .child(el().grow(1.0).child(text(preset["name"].as_str().unwrap_or_default().to_string()).font(look.font(13.0, 400.0)).color(look.fg).single_line()))
                    .child(controls::icon_button(look, format!("hit:preset-delete:{id}"), "Удалить тему", &cross, 14.0, move |state: &mut S, tx: &mut Tx| delete_custom(state, &id, tx))),
            );
        }
        body = body.child(row().py(8.0).child(controls::button(look, "hit:reset-sizes".into(), "Размеры виджетов как в теме", ButtonKind::Normal, |state: &mut S, tx: &mut Tx| reset_sizes(state, tx))));
        fine = fine.child(body);
    }
    pane.child(fine)
}

/// The tint targets' labels: the core's two, then each module's `tints` labels.
fn tint_labels() -> Vec<(String, String)> {
    let mut labels = vec![("panel".to_string(), "Панели".to_string()), ("folder".to_string(), "Папки".to_string())];
    for module in crate::catalog::desk_modules() {
        for tint in module.meta.get("tints").and_then(Value::as_array).into_iter().flatten() {
            if let (Some(id), Some(label)) = (tint["id"].as_str(), tint["label"].as_str()) {
                labels.push((id.to_string(), label.to_string()));
            }
        }
    }
    labels
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn slider_values_read_as_the_page_writes_them() {
        assert_eq!(format_value("glassOpacity", 0.9), "90%");
        assert_eq!(format_value("blur", 16.0), "16 пикс.");
        assert_eq!(format_value("motionScale", 0.0), "без анимаций");
        assert_eq!(format_value("motionScale", 1.25), "×1.25");
        assert_eq!(format_value("inkDepth", 0.3), "0.30");
        assert_eq!(format_value("radiusStrip", 64.0), "пилюля");
        assert_eq!((to_ui("radiusStrip", 999.0), from_ui("radiusStrip", 64.0), from_ui("radiusStrip", 30.0)), (64.0, 999.0, 30.0));
    }

    #[test]
    fn the_choices_include_the_look_options_of_the_modules_drawn() {
        let fields: Vec<String> = choices(|_| true).into_iter().map(|(field, ..)| field).collect();
        assert_eq!(&fields[..3], ["motionProfile", "fontBody", "material"]);
        assert!(fields.iter().any(|f| f == "mgLayout") && fields.iter().any(|f| f == "avatarColor"));
        let drawn: Vec<String> = choices(|id| id == "clock").into_iter().map(|(field, ..)| field).collect();
        assert!(!drawn.iter().any(|f| f == "mgLayout" || f == "avatarColor"), "{drawn:?}");
    }
}
