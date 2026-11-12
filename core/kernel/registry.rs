//! UniDTK Kernel Module Registry.
//!
//! Provides canonical instantiation and inventory of all registered modules.
//! Each vertical slice module exports `create_module() -> Box<dyn unidtk_api::Module>`.
//! Compile-time integrity between disk modules and this registry is enforced by
//! `toolchain/build.rs`.
//!
//! [`Registry`] holds the modules of one run and is the only place that starts,
//! switches and stops them: the start order comes from each module's
//! `starts_after`, the stop runs in reverse after every exit latch is raised. It
//! also maps every module command to the module that serves it.

use std::sync::Arc;
use std::time::Duration;

use unidtk_api::commands::CommandSpec;
use unidtk_api::Module;

/// Factory list of all modules in the application.
pub fn all_modules() -> Vec<Box<dyn Module>> {
    vec![
        unidtk_mod_glyph_bar::create_module(),
        unidtk_mod_explorer::create_module(),
        unidtk_mod_vpn::create_module(),
        unidtk_mod_telegram::create_module(),
        unidtk_mod_mp3_player::create_module(),
        unidtk_mod_micrographics::create_module(),
    ]
}

/// Static list of module IDs for inventory checks and diagnostic assertions.
#[allow(dead_code)]
pub const ALL_MODULE_IDS: &[&str] = &[
    "glyph-bar",
    "explorer",
    "vpn",
    "telegram",
    "mp3-player",
    "micrographics",
];

/// Every module command with the id of its module, as the manifest lists them.
pub fn all_module_commands() -> Vec<(&'static str, &'static CommandSpec)> {
    all_modules()
        .iter()
        .flat_map(|module| module.commands().iter().map(move |spec| (module.id(), spec)))
        .collect()
}

/// A command name that a module may not take: one the kernel serves itself, or one
/// an earlier module took. Either would leave a door serving one of two handlers.
fn claimed_twice(modules: &[Arc<dyn Module>]) -> Option<String> {
    for (index, module) in modules.iter().enumerate() {
        for spec in module.commands() {
            if crate::command_inventory::find(spec.name).is_some() {
                return Some(format!("module {} declares {}, a kernel command", module.id(), spec.name));
            }
            let so_far = modules[..=index]
                .iter()
                .flat_map(|m| m.commands())
                .filter(|s| s.name == spec.name)
                .count();
            if so_far > 1 {
                return Some(format!("command {} is declared twice (last by module {})", spec.name, module.id()));
            }
        }
    }
    None
}

/// The modules of this run, in start order. Built once in `setup`, read-only after.
pub struct Registry {
    modules: Vec<Arc<dyn Module>>,
}

impl Registry {
    /// Orders `modules` so that each one starts after every id it names in
    /// `starts_after`, keeping the given order where nothing constrains it.
    /// A command name claimed twice is an error too.
    pub fn new(modules: Vec<Arc<dyn Module>>) -> Result<Self, String> {
        if let Some(error) = claimed_twice(&modules) {
            return Err(error);
        }
        for (index, module) in modules.iter().enumerate() {
            if modules[..index].iter().any(|earlier| earlier.id() == module.id()) {
                return Err(format!("module {} is registered twice", module.id()));
            }
            if let Some(unknown) = module.starts_after().iter().find(|id| !modules.iter().any(|m| m.id() == **id)) {
                return Err(format!("module {} starts after an unknown module {unknown}", module.id()));
            }
        }
        let mut pending = modules;
        let mut ordered: Vec<Arc<dyn Module>> = Vec::with_capacity(pending.len());
        while !pending.is_empty() {
            let ready = pending
                .iter()
                .position(|module| module.starts_after().iter().all(|id| ordered.iter().any(|m| m.id() == *id)));
            match ready {
                Some(index) => ordered.push(pending.remove(index)),
                None => {
                    let ids: Vec<&str> = pending.iter().map(|module| module.id()).collect();
                    return Err(format!("modules start after each other in a cycle: {}", ids.join(", ")));
                }
            }
        }
        Ok(Self { modules: ordered })
    }

    pub fn get(&self, id: &str) -> Option<&Arc<dyn Module>> {
        self.modules.iter().find(|module| module.id() == id)
    }

    pub fn in_start_order(&self) -> &[Arc<dyn Module>] {
        &self.modules
    }

    /// The module that serves `name`, with the command's spec.
    pub fn command(&self, name: &str) -> Option<(&Arc<dyn Module>, &'static CommandSpec)> {
        self.modules
            .iter()
            .find_map(|module| module.commands().iter().find(|spec| spec.name == name).map(|spec| (module, spec)))
    }

    /// Stops every module: first every exit latch, then each module in reverse
    /// start order, so a module stops before the ones it started after. Returns
    /// the modules whose stop failed, with the reason.
    pub fn shutdown(&self, budget: Duration) -> Vec<(&'static str, String)> {
        for module in &self.modules {
            module.begin_shutdown();
        }
        self.modules
            .iter()
            .rev()
            .filter_map(|module| module.shutdown(budget).err().map(|error| (module.id(), error)))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    /// A module that writes what the registry asked of it into a shared journal.
    struct Probe {
        id: &'static str,
        after: &'static [&'static str],
        journal: Arc<Mutex<Vec<String>>>,
        commands: &'static [CommandSpec],
    }

    impl Module for Probe {
        fn id(&self) -> &'static str {
            self.id
        }
        fn starts_after(&self) -> &'static [&'static str] {
            self.after
        }
        fn commands(&self) -> &'static [CommandSpec] {
            self.commands
        }
        fn init(&self, _ctx: unidtk_api::ModuleContext) -> Result<(), String> {
            Ok(())
        }
        fn begin_shutdown(&self) {
            self.journal.lock().unwrap().push(format!("latch {}", self.id));
        }
        fn shutdown(&self, budget: Duration) -> Result<(), String> {
            self.journal.lock().unwrap().push(format!("stop {} {}ms", self.id, budget.as_millis()));
            Ok(())
        }
    }

    fn probes(spec: &[(&'static str, &'static [&'static str])]) -> (Vec<Arc<dyn Module>>, Arc<Mutex<Vec<String>>>) {
        let journal = Arc::new(Mutex::new(Vec::new()));
        let modules = spec
            .iter()
            .map(|&(id, after)| Arc::new(Probe { id, after, journal: journal.clone(), commands: &[] }) as Arc<dyn Module>)
            .collect();
        (modules, journal)
    }

    fn ids(registry: &Registry) -> Vec<&'static str> {
        registry.in_start_order().iter().map(|module| module.id()).collect()
    }

    #[test]
    fn a_module_starts_after_the_ones_it_names_and_the_rest_keep_their_order() {
        let (modules, _) = probes(&[("telegram", &["vpn"]), ("glyph-bar", &[]), ("vpn", &[]), ("explorer", &[])]);
        let registry = Registry::new(modules).unwrap();
        assert_eq!(ids(&registry), ["glyph-bar", "vpn", "telegram", "explorer"]);
    }

    #[test]
    fn every_latch_is_raised_before_any_module_stops_and_stops_run_in_reverse() {
        // Stopping the VPN fires its state event; Telegram must already be latched
        // and stopped by then, or the event brings its proxy back.
        let (modules, journal) = probes(&[("vpn", &[]), ("telegram", &["vpn"]), ("micrographics", &[])]);
        let registry = Registry::new(modules).unwrap();
        assert!(registry.shutdown(Duration::ZERO).is_empty());
        assert_eq!(
            *journal.lock().unwrap(),
            [
                "latch vpn",
                "latch telegram",
                "latch micrographics",
                "stop micrographics 0ms",
                "stop telegram 0ms",
                "stop vpn 0ms",
            ]
        );
    }

    #[test]
    fn an_unknown_id_a_cycle_or_a_twin_is_a_startup_error() {
        let (unknown, _) = probes(&[("telegram", &["vnp"])]);
        assert!(Registry::new(unknown).err().unwrap().contains("unknown module vnp"));
        let (cycle, _) = probes(&[("a", &["b"]), ("b", &["a"]), ("c", &[])]);
        assert!(Registry::new(cycle).err().unwrap().contains("cycle: a, b"));
        let (twins, _) = probes(&[("vpn", &[]), ("vpn", &[])]);
        assert!(Registry::new(twins).err().unwrap().contains("registered twice"));
    }

    fn with_commands(id: &'static str, commands: &'static [CommandSpec]) -> Arc<dyn Module> {
        Arc::new(Probe { id, after: &[], journal: Arc::new(Mutex::new(Vec::new())), commands })
    }

    #[test]
    fn a_command_maps_to_its_module_and_a_name_is_claimed_once() {
        const VOLUME: &[CommandSpec] = &[CommandSpec::read_only("get_volume", "", "")];
        const STATUS: &[CommandSpec] = &[CommandSpec::write("set_status", "", "")];
        const TWIN: &[CommandSpec] = &[CommandSpec::read_ui("get_volume", "", "")];
        const KERNEL: &[CommandSpec] = &[CommandSpec::read_only("ping", "", "")];

        let registry = Registry::new(vec![with_commands("a", VOLUME), with_commands("b", STATUS)]).unwrap();
        let (module, spec) = registry.command("set_status").unwrap();
        assert_eq!((module.id(), spec.is_write()), ("b", true));
        assert!(registry.command("ping").is_none(), "a kernel command is not a module's");

        let twin = Registry::new(vec![with_commands("a", VOLUME), with_commands("b", TWIN)]);
        assert!(twin.err().unwrap().contains("get_volume is declared twice"));
        let kernel = Registry::new(vec![with_commands("a", KERNEL)]);
        assert!(kernel.err().unwrap().contains("ping, a kernel command"));
    }

    #[test]
    fn the_real_modules_claim_distinct_commands() {
        let modules: Vec<Arc<dyn Module>> = all_modules().into_iter().map(Arc::from).collect();
        let registry = Registry::new(modules).unwrap();
        let commands = all_module_commands();
        assert!(commands.len() > 100, "the modules declare their commands ({} found)", commands.len());
        for (id, spec) in commands {
            assert_eq!(registry.command(spec.name).unwrap().0.id(), id);
        }
    }

    #[test]
    fn the_real_modules_start_in_a_valid_order() {
        let modules = all_modules().into_iter().map(Arc::from).collect();
        let registry = Registry::new(modules).unwrap();
        let order = ids(&registry);
        assert_eq!(order.len(), ALL_MODULE_IDS.len());
        let at = |id| order.iter().position(|m| *m == id).unwrap();
        assert!(at("vpn") < at("telegram"));
    }
}
