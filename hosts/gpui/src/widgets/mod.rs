#![allow(dead_code, unused_imports)]

pub mod clock;
pub mod control_bar;
pub mod explorer;
pub mod folder_widget;
pub mod glyph_bar;
pub mod micrographics;
pub mod music_player;
pub mod player;
pub mod telegram;
pub mod telegram_rail;
pub mod top_dashboard;
pub mod vpn;

use gpui::{AnyView, Entity, Render};

pub use clock::ClockWidget;
pub use control_bar::ControlBarWidget;
pub use explorer::ExplorerWidget;
pub use glyph_bar::GlyphBarWidget;
pub use micrographics::MicrographicsWidget;
pub use music_player::MusicPlayerWidget;
pub use player::PlayerWidget;
pub use telegram::TelegramWidget;
pub use telegram_rail::TelegramRailWidget;
pub use top_dashboard::TopDashboardWidget;
pub use vpn::VpnWidget;

/// Trait implemented by concrete widget view models.
pub trait DesktopWidgetContent: Render + Sized + 'static {
    fn id(&self) -> &'static str;
}

/// Object-safe trait for managing heterogeneous desktop widgets in workspace collections.
pub trait DesktopWidget: 'static {
    fn id(&self) -> &'static str;
    fn to_any_view(&self) -> AnyView;
}

/// Generic wrapper holding a strongly-typed GPUI Entity and implementing DesktopWidget.
pub struct DesktopWidgetHandle<W: DesktopWidgetContent> {
    entity: Entity<W>,
}

impl<W: DesktopWidgetContent> DesktopWidgetHandle<W> {
    pub fn new(entity: Entity<W>) -> Self {
        Self { entity }
    }

    pub fn entity(&self) -> &Entity<W> {
        &self.entity
    }
}

impl<W: DesktopWidgetContent> DesktopWidget for DesktopWidgetHandle<W> {
    fn id(&self) -> &'static str {
        std::any::type_name::<W>()
    }

    fn to_any_view(&self) -> AnyView {
        self.entity.clone().into()
    }
}
