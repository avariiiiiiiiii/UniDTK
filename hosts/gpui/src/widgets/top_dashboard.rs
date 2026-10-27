use gpui::{
    div, prelude::*, px, rgb, rgba, Context, Entity, FontWeight, IntoElement, Render, Window,
};

use crate::db::FolderItem;
use crate::theme;
use crate::widgets::clock::ClockWidget;
use crate::widgets::folder_widget::{FolderIconKind, FolderWidget};
use crate::widgets::DesktopWidgetContent;

pub struct TopDashboardWidget {
    pub time_str: String,
    pub date_day: String,
    pub date_weekday: String,
    pub date_month: String,
    pub temp_str: String,
    pub feels_str: String,
    pub wind_str: String,
    pub precip_amount_str: String,
    pub precip_window_str: String,
    pub precip_chance_str: String,
    pub location_str: String,
    pub condition_str: String,
    pub sunrise_str: String,
    pub sunset_str: String,
    pub daylight_str: String,
    pub daylight_progress_pct: f32,
    pub ip_location_str: String,
    pub second: u32,

    // Embedded child widget entities
    pub clock: Entity<ClockWidget>,
    pub folder_games: Entity<FolderWidget>,
    pub folder_notes: Entity<FolderWidget>,
    pub folder_launcher: Entity<FolderWidget>,
    pub folder_tools: Entity<FolderWidget>,
    pub folder_network: Entity<FolderWidget>,
}

impl TopDashboardWidget {
    pub fn new(cx: &mut Context<Self>) -> Self {
        let clock = cx.new(|cx| ClockWidget::new(cx));

        // Load real folders from unidtk.redb
        let records = crate::db::list_folders().unwrap_or_default();

        let find_or_fallback = |id: &str, default_name: &str, kind: FolderIconKind, default_items: Vec<FolderItem>| {
            if let Some(r) = records.iter().find(|r| r.id == id) {
                FolderWidget::from_record(r)
            } else {
                FolderWidget::new(id, default_name, kind, default_items)
            }
        };

        let folder_games = cx.new(|_cx| {
            find_or_fallback(
                "games",
                "Игры",
                FolderIconKind::Gamepad,
                vec![
                    FolderItem { id: "1".into(), name: "Steam".into(), target: "steam://".into(), kind: "app".into() },
                    FolderItem { id: "2".into(), name: "Discord".into(), target: "discord:".into(), kind: "app".into() },
                ],
            )
        });

        let folder_notes = cx.new(|_cx| {
            find_or_fallback(
                "notes",
                "Заметки",
                FolderIconKind::Notes,
                vec![
                    FolderItem { id: "1".into(), name: "Notepad".into(), target: "notepad.exe".into(), kind: "app".into() },
                    FolderItem { id: "2".into(), name: "Документы".into(), target: "shell:Personal".into(), kind: "folder".into() },
                ],
            )
        });

        let folder_launcher = cx.new(|_cx| {
            find_or_fallback(
                "launcher",
                "Лаунчер",
                FolderIconKind::Launcher,
                vec![
                    FolderItem { id: "1".into(), name: "Терминал".into(), target: "wt.exe".into(), kind: "app".into() },
                    FolderItem { id: "2".into(), name: "VS Code".into(), target: "code".into(), kind: "app".into() },
                ],
            )
        });

        let folder_tools = cx.new(|_cx| {
            find_or_fallback(
                "tools",
                "Утилиты",
                FolderIconKind::Tools,
                vec![
                    FolderItem { id: "1".into(), name: "Проводник".into(), target: "explorer.exe".into(), kind: "folder".into() },
                    FolderItem { id: "2".into(), name: "Диспетчер".into(), target: "taskmgr.exe".into(), kind: "app".into() },
                    FolderItem { id: "3".into(), name: "Калькулятор".into(), target: "calc.exe".into(), kind: "app".into() },
                ],
            )
        });

        let folder_network = cx.new(|_cx| {
            find_or_fallback(
                "network",
                "Сеть",
                FolderIconKind::Network,
                vec![
                    FolderItem { id: "1".into(), name: "Браузер".into(), target: "https://google.com".into(), kind: "url".into() },
                    FolderItem { id: "2".into(), name: "Telegram".into(), target: "tg:".into(), kind: "app".into() },
                ],
            )
        });

        let mut widget = Self {
            time_str: "15:53".to_string(),
            date_day: "10".to_string(),
            date_weekday: "СБ".to_string(),
            date_month: "ОКТ".to_string(),
            temp_str: "+9°C".to_string(),
            feels_str: "+3°C".to_string(),
            wind_str: "31 км/ч".to_string(),
            precip_amount_str: "≈0,2 мм".to_string(),
            precip_window_str: "15:00–18:00".to_string(),
            precip_chance_str: "81%".to_string(),
            location_str: "Санкт-Петербург".to_string(),
            condition_str: "ПАСМУРНО".to_string(),
            sunrise_str: "07:25".to_string(),
            sunset_str: "18:05".to_string(),
            daylight_str: "10ч 40м".to_string(),
            daylight_progress_pct: 0.78,
            ip_location_str: "Manassas".to_string(),
            second: 30,
            clock,
            folder_games,
            folder_notes,
            folder_launcher,
            folder_tools,
            folder_network,
        };
        widget.sync_time();
        widget.sync_weather();
        widget
    }

    pub fn sync_weather(&mut self) {
        if let Some(w) = fetch_live_weather() {
            if let Some(loc) = w.get("location").and_then(|s| s.as_str()) {
                if !loc.is_empty() {
                    self.location_str = loc.to_string();
                }
            }
            if let Some(city) = w.pointer("/ipLocation/city").and_then(|s| s.as_str()) {
                if !city.is_empty() {
                    self.ip_location_str = city.to_string();
                }
            }
            if let Some(temp) = w.get("tempC").and_then(|n| n.as_i64()) {
                self.temp_str = format!("{:+2}°C", temp);
            }
            if let Some(feels) = w.get("feelsC").and_then(|n| n.as_i64()) {
                self.feels_str = format!("{:+2}°C", feels);
            }
            if let Some(wind) = w.get("windKmh").and_then(|n| n.as_i64()) {
                self.wind_str = format!("{} км/ч", wind);
            }
            if let Some(code) = w.get("code").and_then(|n| n.as_i64()) {
                self.condition_str = match code {
                    0 => "ЯСНО",
                    1 => "МАЛООБЛАЧНО",
                    2 => "ПЕРЕМЕННАЯ ОБЛАЧНОСТЬ",
                    3 => "ПАСМУРНО",
                    45 | 48 => "ТУМАН",
                    51 | 53 | 55 => "МОРОСЬ",
                    61 | 63 | 65 => "ДОЖДЬ",
                    71 | 73 | 75 => "СНЕГ",
                    80 | 81 | 82 => "ЛИВЕНЬ",
                    95 | 96 | 99 => "ГРОЗА",
                    _ => "ПАСМУРНО",
                }.to_string();
            }
            if let Some(daylight) = w.get("daylightSeconds").and_then(|n| n.as_i64()) {
                let hours = daylight / 3600;
                let mins = (daylight % 3600) / 60;
                self.daylight_str = format!("{}ч {}м", hours, mins);
            }
            if let Some(rise) = w.get("sunriseAt").and_then(|n| n.as_i64()) {
                if let Some(dt) = chrono::DateTime::from_timestamp(rise, 0) {
                    let local: chrono::DateTime<chrono::Local> = dt.into();
                    self.sunrise_str = local.format("%H:%M").to_string();
                }
            }
            if let Some(set) = w.get("sunsetAt").and_then(|n| n.as_i64()) {
                if let Some(dt) = chrono::DateTime::from_timestamp(set, 0) {
                    let local: chrono::DateTime<chrono::Local> = dt.into();
                    self.sunset_str = local.format("%H:%M").to_string();
                }
            }
            if let Some(forecast) = w.get("precipitationForecast").and_then(|a| a.as_array()) {
                let max_prob = forecast.iter()
                    .filter_map(|h| h.get("probability").and_then(|p| p.as_i64()))
                    .max()
                    .unwrap_or(0);
                self.precip_chance_str = format!("{}%", max_prob);

                let total_mm: f64 = forecast.iter()
                    .filter_map(|h| h.get("amountMm").and_then(|p| p.as_f64()))
                    .sum();
                self.precip_amount_str = format!("≈{:.0} ММ", total_mm);
            }
        }
    }

    pub fn sync_time(&mut self) {
        let now = chrono::Local::now();
        self.time_str = now.format("%H:%M").to_string();
        self.second = now.format("%S").to_string().parse().unwrap_or(0);
        self.date_day = now.format("%d").to_string();

        self.date_weekday = match now.format("%u").to_string().as_str() {
            "1" => "ПН",
            "2" => "ВТ",
            "3" => "СР",
            "4" => "ЧТ",
            "5" => "ПТ",
            "6" => "СБ",
            _ => "ВС",
        }
        .to_string();

        self.date_month = match now.format("%m").to_string().as_str() {
            "01" => "ЯНВ",
            "02" => "ФЕВ",
            "03" => "МАР",
            "04" => "АПР",
            "05" => "МАЙ",
            "06" => "ИЮН",
            "07" => "ИЮЛ",
            "08" => "АВГ",
            "09" => "СЕН",
            "10" => "ОКТ",
            "11" => "НОЯ",
            _ => "ДЕК",
        }
        .to_string();

        if self.second == 0 {
            self.sync_weather();
        }
    }
}

fn fetch_live_weather() -> Option<serde_json::Value> {
    use std::io::{BufRead, BufReader, Write};
    use std::net::TcpStream;
    use std::time::Duration;

    let stream = TcpStream::connect_timeout(
        &"127.0.0.1:26540".parse().ok()?,
        Duration::from_millis(200),
    ).ok()?;
    let _ = stream.set_read_timeout(Some(Duration::from_millis(400)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(200)));

    let mut writer = stream.try_clone().ok()?;
    let _ = writer.write_all(b"{\"cmd\":\"weather_current\"}\n");
    let _ = writer.flush();

    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;

    let v: serde_json::Value = serde_json::from_str(&line).ok()?;
    if v.get("ok").and_then(|b| b.as_bool()).unwrap_or(false) {
        v.get("result").cloned()
    } else {
        None
    }
}

impl DesktopWidgetContent for TopDashboardWidget {
    fn id(&self) -> &'static str {
        "top-dashboard"
    }
}

impl Render for TopDashboardWidget {
    fn render(&mut self, _window: &mut Window, _cx: &mut Context<Self>) -> impl IntoElement {
        let red = theme::nothing_red();
        let font_mono = "Consolas";

        div()
            .flex()
            .items_start()
            .justify_center()
            .gap(px(16.0))
            .child(
                // ── LEFT BLOCK: Micrographics Weather & Date ──
                div()
                    .flex()
                    .flex_col()
                    .items_end()
                    .gap(px(6.0))
                    .pt(px(4.0))
                    .font_family(font_mono)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(
                        // Line 1: СБ (10) [ОКТ] +19°C ≈0 ММ
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().text_color(red).child(self.date_weekday.clone()))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(red)
                                    .text_color(rgb(0xffffff))
                                    .child(self.date_day.clone()),
                            )
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(red)
                                    .text_color(red)
                                    .child(self.date_month.clone()),
                            )
                            .child(div().text_color(red).child(format!("{} {}", self.temp_str, self.precip_amount_str))),
                    )
                    .child(
                        // Line 2: +19°C 10 км/ч
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().text_color(red).child(self.feels_str.clone()))
                            .child(div().text_color(red).child(self.wind_str.clone())),
                    )
                    .child(
                        // Line 3: (11:00-14:00) ⭕ 6%
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(red)
                                    .text_color(red)
                                    .child(self.precip_window_str.clone()),
                            )
                            .child(div().text_color(red).child(format!("⭕ {}", self.precip_chance_str))),
                    )
                    .child(
                        // Line 4: ≈ Вашингтон ::: ::: Manassas [IP]
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(div().text_color(red).child(format!("≈ {}", self.location_str)))
                            .child(div().text_color(red).child("::: :::"))
                            .child(div().text_color(red).child(self.ip_location_str.clone()))
                            .child(
                                div()
                                    .px_1p5()
                                    .py_0p5()
                                    .rounded_full()
                                    .border_1()
                                    .border_color(red)
                                    .text_color(red)
                                    .child("IP"),
                            ),
                    ),
            )
            .child(
                // ── LEFT FOLDERS COLUMN 1: Games (64x64) ──
                div().child(self.folder_games.clone()),
            )
            .child(
                // ── LEFT FOLDERS COLUMN 2: Notes (64x64) + Launcher (64x64) stacked ──
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(self.folder_notes.clone())
                    .child(self.folder_launcher.clone()),
            )
            .child(
                // ── CENTER CLOCK WIDGET (210x190 Nothing OS Concentric Rings .clock-rings) ──
                div().child(self.clock.clone()),
            )
            .child(
                // ── RIGHT FOLDERS COLUMN: Tools (64x64) + Network (64x64) stacked ──
                div()
                    .flex()
                    .flex_col()
                    .gap(px(6.0))
                    .child(self.folder_tools.clone())
                    .child(self.folder_network.clone()),
            )
            .child(
                // ── RIGHT BLOCK: Weather Condition, Sun Astronomy, Daylight ──
                div()
                    .flex()
                    .flex_col()
                    .items_start()
                    .gap(px(6.0))
                    .pt(px(4.0))
                    .font_family(font_mono)
                    .text_sm()
                    .font_weight(FontWeight::BOLD)
                    .child(
                        // Line 1: [ПАСМУРНО]
                        div()
                            .flex()
                            .items_center()
                            .gap(px(6.0))
                            .child(
                                div()
                                    .px_2()
                                    .py_0p5()
                                    .rounded_full()
                                    .bg(red)
                                    .text_color(rgb(0xffffff))
                                    .child(self.condition_str.clone()),
                            ),
                    )
                    .child(
                        // Line 2: 07:14 18:38
                        div()
                            .flex()
                            .items_center()
                            .gap(px(10.0))
                            .child(div().text_color(red).child(self.sunrise_str.clone()))
                            .child(div().text_color(red).child(self.sunset_str.clone())),
                    )
                    .child(
                        // Line 3: 11ч 25м [======---]
                        div()
                            .flex()
                            .items_center()
                            .gap(px(8.0))
                            .child(div().text_color(red).child(self.daylight_str.clone()))
                            .child(
                                div()
                                    .w(px(70.0))
                                    .h(px(8.0))
                                    .border_1()
                                    .border_color(red)
                                    .child(
                                        div()
                                            .h_full()
                                            .w(px(self.daylight_progress_pct * 70.0))
                                            .bg(red),
                                    ),
                            ),
                    ),
            )
    }
}
