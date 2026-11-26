//! The music player's native half (`r[modules.vertical-slice]`): the page's card, library,
//! settings and similarity graph (`modules/mp3-player/ui`) as a widget of the shell's
//! contract. Built only with the application's `mp3-native` feature until the port is whole
//! (session 152); nothing of it reaches the desk before then.

pub mod logic;

use rune::el::El;
use unidtk_shell::widget::{Init, View, Widget, WidgetType};

/// The kernel's player events: the bands for the arcs, the play state, the sync tasks, the
/// AI DJ's analysis and model download.
pub static WIDGET: WidgetType = WidgetType {
    id: "mp3-player",
    events: &["audio-bands", "player-state", "sync-tasks", "ai-dj-progress", "ai-dj-model"],
    make,
    settings: None,
};

fn make(_: &Init<'_>) -> Box<dyn Widget> {
    Box::new(Player::default())
}

#[derive(Default)]
struct Player {}

impl Widget for Player {
    fn content(&self, _v: &View<'_>) -> Option<El> {
        None
    }
}
