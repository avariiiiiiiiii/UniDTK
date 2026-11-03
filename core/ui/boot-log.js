import { invoke } from "@unidtk/bridge";

function log(msg, level = "INFO") {
  invoke("frontend_log", { message: msg, level }).catch(() => {});
}

window.__wallpaper_log = log;

window.addEventListener("error", (e) => {
  log(`BOOT ERROR: ${e.message} at ${e.filename}:${e.lineno}:${e.colno} :: ${e.error?.stack || ""}`, "ERROR");
});

window.addEventListener("unhandledrejection", (e) => {
  const r = e.reason;
  log(`BOOT UNHANDLED: ${r?.message || r?.stack || String(r)}`, "ERROR");
});

// Catch module load failures (import errors fire as error events)
log("boot-log: handler installed");
