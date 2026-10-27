use std::sync::Arc;

use gpui::{
    div, prelude::*, px, rgb, rgba, Context, FontWeight, IntoElement, Render, Window,
};
use unidtk_api::{InMemoryModuleDb, Module, ModuleContext, ModuleDb};
use unidtk_mod_micrographics::{MicrographicsModule, WeatherSnapshot};

use crate::theme::{self, font_dot};
use crate::widgets::DesktopWidgetContent;

pub struct MicrographicsWidget {
    pub location: String,
    pub temp_c: Option<i32>,
    pub feels_c: Option<i32>,
    pub condition_label: String,
    pub wind_kmh: Option<u32>,
    pub sunrise_str: String,
    pub sunset_str: String,
    pub daylight_str: String,
    pub is_day: bool,
    pub stale: bool,
    module: Arc<MicrographicsModule>,
}

impl MicrographicsWidget {
    pub fn new(_cx: &mut Context<Self>) -> Self {
        let module = Arc::new(MicrographicsModule::new());

        // Initialize micrographics with in-memory context and grant location access to start background polling
        let db = Arc::new(InMemoryModuleDb::new());
        let _ = db.kv_set("windows/access-allowed-v1", b"allowed");
        let ctx = ModuleContext::new("micrographics", db);
        let _ = module.init(ctx);

        let mut widget = Self {
            location: "DEVICE FIX".to_string(),
            temp_c: None,
            feels_c: None,
            condition_label: "CLEAR".to_string(),
            wind_kmh: None,
            sunrise_str: "06:45".to_string(),
            sunset_str: "18:30".to_string(),
            daylight_str: "11H 45M".to_string(),
            is_day: true,
            stale: false,
            module,
        };

        widget.update_from_snapshot();
        widget
    }

    pub fn update_from_snapshot(&mut self) -> bool {
        if let Some(snap) = self.module.runtime().snapshot() {
            let changed = self.temp_c != snap.temp_c || self.condition_label != wmo_condition_label(snap.code);
            self.temp_c = snap.temp_c;
            self.feels_c = snap.feels_c;
            self.wind_kmh = snap.wind_kmh;
            self.condition_label = wmo_condition_label(snap.code).to_string();
            self.is_day = snap.is_day.unwrap_or(true);
            self.stale = snap.stale;

            if !snap.location.is_empty() {
                self.location = snap.location.to_uppercase();
            }

            if let (Some(rise), Some(set)) = (snap.sunrise_at, snap.sunset_at) {
                self.sunrise_str = format_unix_time(rise);
                self.sunset_str = format_unix_time(set);
            }

            if let Some(secs) = snap.daylight_seconds {
                let h = secs / 3600;
                let m = (secs % 3600) / 60;
                self.daylight_str = format!("{h}H {m:02}M");
            }

            return changed;
        }
        false
    }
}

fn wmo_condition_label(code: Option<u8>) -> &'static str {
    match code {
        Some(0) => "CLEAR SKY",
        Some(1) => "MAINLY CLEAR",
        Some(2) => "PARTLY CLOUDY",
        Some(3) => "OVERCAST",
        Some(45) | Some(48) => "FOG",
        Some(51..=55) => "LIGHT DRIZZLE",
        Some(61..=65) => "RAIN",
        Some(71..=77) => "SNOW",
        Some(80..=82) => "SHOWERS",
        Some(95..=99) => "THUNDERSTORM",
        _ => "AMBIENT WEATHER",
    }
}

fn format_unix_time(ts: u64) -> String {
    let dt = chrono::DateTime::from_timestamp(ts as i64, 0)
        .unwrap_or_else(|| chrono::Utc::now());
    let local = dt.with_timezone(&chrono::Local);
    local.format("%H:%M").to_string()
}

impl DesktopWidgetContent for MicrographicsWidget {
    fn id(&self) -> &'static str {
        "micrographics"
    }
}

impl Render for MicrographicsWidget {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let temp_str = match self.temp_c {
            Some(t) if t > 0 => format!("+{t}°"),
            Some(t) => format!("{t}°"),
            None => "--°".to_string(),
        };

        let feels_str = match self.feels_c {
            Some(t) if t > 0 => format!("FEELS +{t}°"),
            Some(t) => format!("FEELS {t}°"),
            None => "FEELS --°".to_string(),
        };

        let wind_str = match self.wind_kmh {
            Some(w) => format!("WIND {w} KM/H"),
            None => "WIND CALM".to_string(),
        };

        div()
            .flex()
            .flex_col()
            .w(px(320.0))
            .p_6()
            .rounded_3xl()
            .bg(theme::bg_glass())
            .border_1()
            .border_color(theme::border_glass())
            .shadow_lg()
            .child(
                // Header: Astronomy / Condition dot, title, and Location pill
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mb_3()
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_2()
                            .child(
                                // Dot: Amber if day, Indigo if night
                                div()
                                    .size_2()
                                    .rounded_full()
                                    .bg(if self.is_day {
                                        rgb(0xffa726)
                                    } else {
                                        rgb(0x7e57c2)
                                    }),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_sm()
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child("WEATHER & SUN"),
                            ),
                    )
                    .child(
                        // Location pill
                        div()
                            .px_2()
                            .py_0p5()
                            .rounded_full()
                            .bg(rgba(0xffffff15))
                            .border_1()
                            .border_color(theme::border_glass())
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(self.location.clone()),
                            ),
                    ),
            )
            .child(
                // Middle row: Temperature and Weather condition
                div()
                    .flex()
                    .items_baseline()
                    .justify_between()
                    .w_full()
                    .my_1()
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_size(px(40.0))
                                    .font_weight(FontWeight::BOLD)
                                    .text_color(theme::text_primary())
                                    .child(temp_str),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .font_weight(FontWeight::SEMIBOLD)
                                    .text_color(if self.is_day {
                                        rgb(0xffa726)
                                    } else {
                                        theme::text_secondary()
                                    })
                                    .child(self.condition_label.clone()),
                            ),
                    )
                    .child(
                        div()
                            .flex()
                            .flex_col()
                            .items_end()
                            .gap_1()
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_secondary())
                                    .child(feels_str),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(wind_str),
                            ),
                    ),
            )
            .child(
                // Bottom row: Sun astronomy (Sunrise / Sunset / Daylight)
                div()
                    .flex()
                    .items_center()
                    .justify_between()
                    .w_full()
                    .mt_4()
                    .pt_2()
                    .border_t_1()
                    .border_color(theme::border_glass())
                    .child(
                        div()
                            .flex()
                            .items_center()
                            .gap_3()
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format!("RISE {}", self.sunrise_str)),
                            )
                            .child(
                                div()
                                    .font(font_dot())
                                    .text_xs()
                                    .text_color(theme::text_muted())
                                    .child(format!("SET {}", self.sunset_str)),
                            ),
                    )
                    .child(
                        div()
                            .font(font_dot())
                            .text_xs()
                            .font_weight(FontWeight::MEDIUM)
                            .text_color(theme::accent_green())
                            .child(self.daylight_str.clone()),
                    ),
            )
    }
}
