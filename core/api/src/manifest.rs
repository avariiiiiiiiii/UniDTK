//! Declarative dependencies manifest for UniDTK modules.
//!
//! Modules declare their external dependencies (binaries, DLLs, model weights,
//! scheduled tasks) statically via `Module::dependencies()`. The kernel and
//! CLI diagnostic tools inspect this manifest without invoking module code.

/// Severity of a module dependency.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DependencySeverity {
    /// Mandatory dependency: without this, the module cannot operate (causes FAIL).
    Mandatory,
    /// Optional dependency: module operates, but specific features are disabled (causes WARN).
    Optional,
}

/// Type and characteristics of an external system dependency.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DependencyKind {
    /// Dynamic link library (.dll).
    DynamicLibrary {
        /// Primary DLL filename (e.g. "tdjson.dll" or "libmpv-2.dll").
        file_name: &'static str,
        /// Required companion DLLs that must be co-located (e.g. OpenSSL companions).
        companions: &'static [&'static str],
        /// Minimum expected file size in bytes (filters 0-byte files and Git LFS stubs).
        min_bytes: u64,
    },
    /// Executable daemon or tool (.exe).
    Executable {
        /// Executable filename (e.g. "sing-box.exe" or "vpn-helper.exe").
        file_name: &'static str,
        /// Optional arguments for a non-blocking smoke probe (e.g. &["version"]).
        probe_args: Option<&'static [&'static str]>,
    },
    /// Model graph or heavy weights asset file.
    ModelAsset {
        /// Relative path within models directory (e.g. "discogs-maest-129e/model.onnx").
        relative_path: &'static str,
        /// Minimum expected file size of the primary file in bytes.
        min_bytes: u64,
        /// Companion asset files with minimum sizes (e.g. external ONNX weights .data).
        companion_files: &'static [(&'static str, u64)],
    },
    /// Windows Task Scheduler elevated background task.
    WindowsTask {
        /// Registered task name in Task Scheduler.
        task_name: &'static str,
        /// Whether the task requires HighestAvailable privileges.
        require_elevated: bool,
    },
}

/// Single declarative dependency entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ModuleDependency {
    pub id: &'static str,
    pub description: &'static str,
    pub severity: DependencySeverity,
    pub kind: DependencyKind,
}

/// One render entry point of a UI module, for the `doctor -simulate` probe.
///
/// Lives in `modules/<id>/ui/metadata.json` as `simulate: {module, function,
/// sandbox}` — the CLI reads it at runtime from the staged `dist/` tree, so
/// adding or renaming an entry point never requires touching the CLI. The
/// sandbox is the offscreen HTML the probe renders into; the probe stays
/// offscreen and never touches the live layout.
///
/// The CLI binary (`udtk`) includes this file with `#[path]` transitively and
/// reads this type; the kernel binary does not, hence the `dead_code`
/// allowance.
#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SimulateEntry {
    pub module_id: &'static str,
    pub module_file: &'static str,
    pub function: &'static str,
    pub sandbox: &'static str,
}
