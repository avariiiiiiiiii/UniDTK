//! A module's settings pane inside the shell menu (r[shell.widget-contract]).

use std::any::Any;

use rune::{el::El, TextEdit, Tx};
use serde_json::Value;

use crate::controls::Look;
use crate::menu::MenuHost;
use crate::Host;

pub type Factory = fn() -> Box<dyn Settings>;

/// State for one opening of a module's settings. It lives independently of any widget tile.
pub trait Settings: Any {
    fn load(&mut self, act: &mut Act);
    fn pane(&self, view: &View<'_>) -> El;
}

pub(crate) struct Session {
    pub generation: u64,
    pub settings: Box<dyn Settings>,
}

/// The menu's appearance and the bindings for its current pane.
pub struct View<'a> {
    pub look: &'a Look,
    generation: u64,
}

impl<'a> View<'a> {
    pub(crate) fn new(look: &'a Look, generation: u64) -> Self {
        Self { look, generation }
    }

    pub fn on<P: Settings>(
        &self,
        handler: impl Fn(&mut P, &mut Act) + 'static,
    ) -> impl Fn(&mut Host, &mut Tx) + 'static {
        let generation = self.generation;
        move |host, tx| {
            run(host, generation, tx, |settings, act| {
                if let Some(pane) = downcast::<P>(settings) {
                    handler(pane, act);
                }
            });
        }
    }

    pub fn edit<P: Settings>(
        &self,
        field: impl Fn(&mut P) -> &mut TextEdit + 'static,
    ) -> impl Fn(&mut Host) -> &mut TextEdit + 'static {
        let generation = self.generation;
        move |host: &mut Host| match host
            .menu
            .active_settings(generation)
            .and_then(downcast::<P>)
        {
            Some(pane) => field(pane),
            None => &mut host.scratch,
        }
    }
}

fn downcast<P: Settings>(settings: &mut dyn Settings) -> Option<&mut P> {
    (settings as &mut dyn Any).downcast_mut()
}

type Reply = Box<dyn FnOnce(&mut dyn Settings, Result<Value, String>, &mut Act) + Send>;

enum Op {
    Call {
        cmd: &'static str,
        args: Value,
        reply: Reply,
    },
    Toast {
        text: String,
        error: bool,
    },
}

/// A pane's requests to the kernel and menu. The shell applies them after its handler returns.
#[derive(Default)]
pub struct Act {
    ops: Vec<Op>,
}

impl Act {
    /// Uses the same UI door as the desk. Replies belong to this opening of the pane.
    pub fn call<P: Settings>(
        &mut self,
        cmd: &'static str,
        args: Value,
        then: impl FnOnce(&mut P, Result<Value, String>, &mut Act) + Send + 'static,
    ) {
        let reply = Box::new(move |pane: &mut dyn Settings, result, act: &mut Act| {
            if let Some(pane) = downcast::<P>(pane) {
                then(pane, result, act);
            }
        });
        self.ops.push(Op::Call { cmd, args, reply });
    }

    pub fn toast(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Toast {
            text: text.into(),
            error: false,
        });
    }

    pub fn error(&mut self, text: impl Into<String>) {
        self.ops.push(Op::Toast {
            text: text.into(),
            error: true,
        });
    }
}

pub(crate) fn run<S: MenuHost>(
    host: &mut S,
    generation: u64,
    tx: &mut Tx,
    handler: impl FnOnce(&mut dyn Settings, &mut Act),
) {
    let Some(pane) = host.menu().active_settings(generation) else {
        return;
    };
    let mut act = Act::default();
    handler(pane, &mut act);
    run_ops(host, generation, tx, act.ops);
}

fn run_ops<S: MenuHost>(host: &mut S, generation: u64, tx: &mut Tx, ops: Vec<Op>) {
    for op in ops {
        match op {
            Op::Call { cmd, args, reply } => {
                host.call(
                    cmd,
                    args,
                    Box::new(move |host, result, tx| {
                        run(host, generation, tx, |pane, act| reply(pane, result, act));
                    }),
                );
            }
            Op::Toast { text, error } => {
                host.toasts().show(text, error, None);
                crate::surface::keep_frames(tx);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::menu::{self, Sheet};
    use crate::widget::{Init, Widget, WidgetType};

    #[derive(Default)]
    struct ExampleSettings {
        loaded: bool,
        input: TextEdit,
    }

    impl Settings for ExampleSettings {
        fn load(&mut self, _: &mut Act) {
            self.loaded = true;
        }

        fn pane(&self, _: &View<'_>) -> El {
            rune::el::el()
        }
    }

    struct ExampleWidget;

    impl Widget for ExampleWidget {
        fn content(&self, _: &crate::widget::View<'_>) -> Option<El> {
            None
        }
    }

    static EXAMPLE: WidgetType = WidgetType {
        id: "example",
        events: &[],
        make: |_: &Init<'_>| Box::new(ExampleWidget),
        settings: Some(|| Box::<ExampleSettings>::default()),
    };

    #[test]
    fn a_pane_can_open_without_a_desk_widget_and_old_bindings_expire() {
        let mut host = Host::new(
            crate::inspector::Inspector::new().unwrap(),
            None,
            (800.0, 600.0),
            1.0,
            Default::default(),
            &[&EXAMPLE],
        );
        let mut tx = Tx::default();
        host.menu.open = true;
        host.menu.sheet = Some(Sheet::Widget(EXAMPLE.id.into()));
        menu::open_module_settings(&mut host, EXAMPLE.id, &mut tx);
        assert!(host.widgets.is_empty());

        let menu::View::ModuleSettings(session) = &host.menu.view else {
            panic!("expected module settings")
        };
        let first = session.generation;
        assert!(
            host.menu
                .active_settings(first)
                .and_then(downcast::<ExampleSettings>)
                .unwrap()
                .loaded
        );
        let look = Look::panel(&host.theme());
        let first_view = View::new(&look, first);
        let first_edit = first_view.edit(|pane: &mut ExampleSettings| &mut pane.input);
        let first_press = first_view.on(|pane: &mut ExampleSettings, _| pane.loaded = false);
        first_edit(&mut host).text = "first".into();

        menu::close(&mut host, &mut tx);
        assert!(matches!(&host.menu.view, menu::View::ModuleSettings(_)));
        assert!(host.menu.active_settings(first).is_none());

        menu::toggle(&mut host, &mut tx);
        host.menu.sheet = Some(Sheet::Widget(EXAMPLE.id.into()));
        menu::open_module_settings(&mut host, EXAMPLE.id, &mut tx);
        let menu::View::ModuleSettings(session) = &host.menu.view else {
            panic!("expected reopened settings")
        };
        let second = session.generation;
        assert_ne!(first, second);

        first_edit(&mut host).text = "stale".into();
        first_press(&mut host, &mut tx);
        let opened = host
            .menu
            .active_settings(second)
            .and_then(downcast::<ExampleSettings>)
            .unwrap();
        assert!(opened.loaded);
        assert!(opened.input.text.is_empty());
    }
}
