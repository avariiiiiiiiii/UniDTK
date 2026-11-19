//! The clock's dashboard (`renderClockDashboard`, `.clock-dash-layout` in `clock.css`, the
//! calendar and connection badge styles in `micrographics.css`): the month, Sunday first, and
//! the connection badge `navigator.onLine` fills.

use rune::el::{col, el, row, text, Align, El};
use rune::{Color, LineCap, Shape, TextStyle};
use unidtk_shell::widget::{LocalTime as ClockTime, Theme};

/// `new Date(year, month, 1).getDay()`: the column of the 1st, Sunday first.
fn first_weekday(time: ClockTime) -> usize {
    chrono::NaiveDate::from_ymd_opt(time.year, time.month, 1).map_or(0, |date| {
        use chrono::Datelike;
        date.weekday().num_days_from_sunday() as usize
    })
}

fn days_in_month(time: ClockTime) -> u32 {
    let (year, month) = if time.month == 12 { (time.year + 1, 1) } else { (time.year, time.month + 1) };
    chrono::NaiveDate::from_ymd_opt(year, month, 1)
        .and_then(|next| next.pred_opt())
        .map_or(30, |last| {
            use chrono::Datelike;
            last.day()
        })
}

fn style(theme_stack: &str, size: f32, weight: f32) -> TextStyle {
    TextStyle::new(size, weight).family(Theme::family(theme_stack)).lh_normal()
}

/// The dashboard's month: `.dash-calendar` as `renderClockDashboard` fills it.
pub fn calendar(time: ClockTime, theme: &Theme, ink: Color, width: f32) -> (El, f32) {
    const PAD: f32 = 12.0;
    const GAP: f32 = 4.0;
    const NAMES: [&str; 7] = ["В", "П", "В", "С", "Ч", "П", "С"];
    let inner = width - 2.0 * PAD - 2.0;
    let cell = (inner - 6.0 * GAP) / 7.0;
    let body = |size: f32, weight: f32| style(theme.font_body, size, weight);
    let line = 9.6 * Theme::normal_line(theme.font_body);
    let cells: Vec<El> = (0..theme_cells(time))
        .map(|index| {
            let day = index as i64 - first_weekday(time) as i64 + 1;
            if day < 1 || day > i64::from(days_in_month(time)) {
                return el().w(cell).h(line + 4.0);
            }
            let today = day as u32 == time.day;
            let label = text(day.to_string()).font(body(9.6, if today { 700.0 } else { 400.0 })).align(Align::Center);
            let mut cell_el = col().w(cell).h(line + 4.0).items_center().justify_center();
            if today {
                cell_el = cell_el.fill(theme.bg_elevated).radius(4.0);
                cell_el.child(label.color(theme.scrim))
            } else {
                cell_el.child(label.color(ink.alpha(0.6)))
            }
        })
        .collect();
    let weeks = cells.len() / 7;
    let mut rows = col().gap(GAP).child(
        row().gap(GAP).children(NAMES.iter().map(|name| {
            col().w(cell).h(line + 4.0).items_center().child(text(*name).font(body(9.6, 600.0)).color(ink.alpha(0.3)))
        })),
    );
    let mut cells = cells.into_iter();
    for _ in 0..weeks {
        rows = rows.child(row().gap(GAP).children(cells.by_ref().take(7)));
    }
    // `toLocaleString('default', { month: 'long' })`, upper-cased as `.cal-header` shows it.
    let header = text(time.month_name().to_uppercase()).font(style(theme.font_display, 11.2, 400.0)).color(theme.accent).align(Align::Center);
    let height = 2.0 + 2.0 * PAD + 11.2 * Theme::normal_line(theme.font_display) + 8.0 + (weeks as f32 + 1.0) * (line + 4.0) + weeks as f32 * GAP;
    let out = col()
        .w(width)
        .h(height)
        .padding(PAD)
        .radius(16.0)
        .fill(theme.bg_elevated.alpha(0.03))
        .border(1.0, theme.bg_elevated.alpha(0.05)).border_layout()
        .child(col().w_full().items_center().child(header))
        .child(el().h(8.0))
        .child(rows);
    (out, height)
}

/// The grid's cell count: the blanks before the 1st and the days, whole weeks.
fn theme_cells(time: ClockTime) -> usize {
    let used = first_weekday(time) + days_in_month(time) as usize;
    used.div_ceil(7) * 7
}

/// Points on a circle of `radius` around `centre`, from `from` to `to` degrees clockwise from
/// twelve o'clock.
fn circle_points(centre: [f32; 2], radius: f32, from: f32, to: f32, steps: usize) -> Vec<[f32; 2]> {
    (0..=steps)
        .map(|step| {
            let angle = (from + (to - from) * step as f32 / steps as f32).to_radians();
            [centre[0] + radius * angle.sin(), centre[1] - radius * angle.cos()]
        })
        .collect()
}

/// The Wi-Fi glyph of `.net-icon` (a 24-unit SVG: three arcs and a dot), as polylines.
fn wifi_icon(colour: Color) -> Shape {
    // Each `a r r 0 0 1` arc runs clockwise over the top of its circle between two points
    // on the same height; the centre lies below the chord.
    let arc = |from: [f32; 2], to: [f32; 2], radius: f32| {
        let half = (to[0] - from[0]) / 2.0;
        let centre = [from[0] + half, from[1] + (radius * radius - half * half).max(0.0).sqrt()];
        let sweep = (half / radius).clamp(-1.0, 1.0).asin().to_degrees();
        circle_points(centre, radius, -sweep, sweep, 24)
    };
    Shape::new()
        .view_box(24.0, 24.0)
        .stroke_with(arc([5.0, 12.55], [19.08, 12.55], 11.0), false, 2.0, LineCap::Round, colour)
        .stroke_with(arc([1.42, 9.0], [22.58, 9.0], 16.0), false, 2.0, LineCap::Round, colour)
        .stroke_with(arc([8.53, 16.11], [15.48, 16.11], 6.0), false, 2.0, LineCap::Round, colour)
        .line([12.0, 20.0], [12.01, 20.0], 2.0, LineCap::Round, colour)
}

/// `.dash-internet`: the connection badge `navigator.onLine` fills.
pub fn internet(online: bool, theme: &Theme, ink: Color, width: f32) -> El {
    let status = if online { theme.status_ok } else { theme.status_error };
    row()
        .w(width)
        .h(50.0)
        .py(10.0)
        .px(14.0)
        .gap(10.0)
        .items_center()
        .radius(14.0)
        .fill(theme.bg_elevated.alpha(0.03))
        .border(1.0, theme.bg_elevated.alpha(0.05)).border_layout()
        .child(
            col().size(28.0, 28.0).radius(14.0).fill(status.alpha(0.1)).items_center().justify_center()
                .child(el().size(16.0, 16.0).shape(wifi_icon(status))),
        )
        .child(text(if online { "В сети" } else { "Нет сети" }).font(style(theme.font_body, 11.2, 500.0)).color(ink.alpha(0.8)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_calendar_month_is_laid_out_sunday_first() {
        // 1 November 2026 is a Sunday; February 2028 has 29 days and starts on a Tuesday.
        let november = ClockTime { year: 2026, month: 11, day: 15, ..Default::default() };
        assert_eq!((first_weekday(november), days_in_month(november), theme_cells(november)), (0, 30, 35));
        let february = ClockTime { year: 2028, month: 2, day: 1, ..Default::default() };
        assert_eq!((first_weekday(february), days_in_month(february), theme_cells(february)), (2, 29, 35));
        assert_eq!(days_in_month(ClockTime { month: 12, year: 2026, ..Default::default() }), 31);
        // The language is the machine's: twelve different names, whatever it is.
        let names: std::collections::HashSet<String> = (1..=12).map(|month| ClockTime { month, ..november }.month_name()).collect();
        assert_eq!(names.len(), 12, "{names:?}");
    }
}
