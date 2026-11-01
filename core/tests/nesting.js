/** Run through `udtk gui js` in the real WebView; no external JS runtime. */
import { canNest, effectiveNested, moveNested, isContainer } from "../ui/nesting.js";

const clock = { id: "clock", name: "Часы", container: true };
const folder = { id: "f1", name: "Папка", isFolder: true };
const vpn = { id: "vpn", name: "VPN", embeddable: { module: "vpn-panel.js", render: "renderVpnPanel" } };
const player = { id: "mp3-player", name: "Плеер" };

export function runNestingDiagnostics() {
  let checks = 0;
  const assert = (ok, message) => { checks++; if (!ok) throw new Error(message); };

  assert(isContainer(clock) && isContainer(folder) && !isContainer(vpn), "containers");
  assert(canNest(vpn, clock).ok && canNest(vpn, folder).ok, "embeddable enters both hosts");
  assert(!canNest(vpn, player).ok && canNest(vpn, player).reason.includes("Плеер"), "host without contract refuses, by name");
  assert(!canNest(player, clock).ok && canNest(player, clock).reason.includes("компактного"), "child without contract refused");
  assert(!canNest(folder, clock).ok && !canNest(clock, folder).ok, "containers never nest: no cycles");
  assert(!canNest(clock, clock).ok, "not into itself");
  assert(!canNest(undefined, clock).ok, "unknown widget refused");

  const widgets = [clock, folder, vpn, player];
  assert(effectiveNested({ clock: ["vpn"] }, widgets).clock[0] === "vpn", "valid nesting applies");
  assert(!effectiveNested({ clock: ["vpn"] }, [folder, vpn]).clock, "missing host: child goes to the desk");
  assert(!effectiveNested({ clock: ["vpn"] }, [clock, folder]).clock, "missing child: nothing to show");
  assert(!effectiveNested({ clock: ["mp3-player"] }, widgets).clock, "contract rechecked on load");
  const twice = effectiveNested({ clock: ["vpn"], f1: ["vpn"] }, widgets);
  assert(twice.clock && !twice.f1, "one child lives in one host");
  assert(Object.keys(effectiveNested(null, widgets)).length === 0, "old configs have no key");
  assert(Object.keys(effectiveNested({ clock: "junk" }, widgets)).length === 0, "malformed entry ignored");
  const stored = { clock: ["vpn"] };
  effectiveNested(stored, [folder]);
  assert(stored.clock[0] === "vpn", "absent host never trims the stored intent");

  const many = Array.from({ length: 200 }, (_, i) => ({ id: `w${i}`, name: "w", embeddable: {} }));
  assert(effectiveNested({ clock: many.map((w) => w.id) }, [clock, ...many]).clock.length === 200, "no count limit");

  assert(moveNested({ clock: ["vpn"] }, "vpn", "f1").f1[0] === "vpn", "moves to the new host");
  assert(!moveNested({ clock: ["vpn"] }, "vpn", "f1").clock, "leaves the old host");
  assert(Object.keys(moveNested({ clock: ["vpn"] }, "vpn", null)).length === 0, "ejected: nothing left");
  return { checks };
}
