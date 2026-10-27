/**
 * unidtk-bridge.js — the single import point for everything the UI needs from the host.
 *
 * Why this file exists: the interface used to talk to Tauri from ~13 files, each
 * importing `@tauri-apps/api/core` or a plugin directly. That made the transport
 * impossible to change in one place — which is exactly what the frontend migration
 * (`docs/cliudtk.md` §10.22) needs. The UI imports *this* module; only this module knows
 * how a call reaches the kernel.
 *
 * Rules this file keeps:
 * - the real Tauri functions stay internal (`tauriInvoke`, `tauriListen`), so the
 *   WebSocket path cannot recurse into itself;
 * - `session_info` always goes through Tauri, never through the public `invoke`;
 * - the transport is decided **once** per page load: mixing transports inside one page
 *   would leave half the subscriptions and half the calls on the old door;
 * - the per-run token lives only in this module's memory — never in sessionStorage,
 *   localStorage, the URL or the log;
 * - a command travels over WebSocket only if it is listed in `WS_COMMANDS`, and it gets
 *   listed only after a clean `shadow` run.
 */

import { invoke as tauriInvoke, convertFileSrc } from "@tauri-apps/api/core";
import { listen as tauriListen } from "@tauri-apps/api/event";
import { appDataDir, resolveResource } from "@tauri-apps/api/path";
import {
  BaseDirectory,
  mkdir,
  readDir,
  readTextFile,
  writeTextFile,
} from "@tauri-apps/plugin-fs";
import { openPath } from "@tauri-apps/plugin-opener";

// Re-exported untouched: plugins and asset URLs stay on Tauri for the whole migration
// (moving them is a separate stage, §10.22 «что не делать»).
export {
  convertFileSrc,
  appDataDir,
  resolveResource,
  BaseDirectory,
  mkdir,
  readDir,
  readTextFile,
  writeTextFile,
  openPath,
};

/** Wire version this page speaks. Compared with the app's `ping`/`session` answer. */
const PROTOCOL = 1;
const HANDSHAKE_TIMEOUT_MS = 8000;
// Generous on purpose: the kernel serves one socket connection in order, and a busy
// backend (library scan, VPN probe) can legitimately take seconds. A tight timeout here
// produced 14 phantom "failures" that were load, not divergence.
const COMMAND_TIMEOUT_MS = 15000;

/**
 * Commands allowed to travel over the WebSocket door. A command is added here only
 * after a clean `shadow` run (both doors called, answers compared structurally), so
 * nothing reaches the new door unverified.
 *
 * Every read-only command the interface uses now travels over the socket; the volume
 * read has no UI caller by name (volume arrives as the `system-volume` event).
 */
export const WS_COMMANDS = [
  "player_status",
  "vpn_status",
  "vpn_list_servers",
  "vpn_settings",
  "telegram_status",
  "telegram_chats",
  "ai_dj_status",
  "get_widget_config",
  "list_tracks",
  "list_playlists",
  "get_submodules",
  "get_drives",
  "get_user_places",
  "weather_current",
];

/**
 * Write commands that travel over the socket.
 *
 * Separate from [`WS_COMMANDS`] because a write must **never** be compared by calling
 * both doors: `shadow` asks two transports the same question, and for a command that
 * changes something that is not a comparison — it is the action, performed twice. So a
 * write is migrated by flipping its door in `ws` mode and checking the *effect* with a
 * follow-up read; in `shadow` it deliberately keeps using the old door.
 *
 * `frontend_log` was the pilot: its effect is a journal record, harmless and directly
 * observable (`udtk logs --source frontend`). The volume writes followed, because their
 * effect is real state and equally readable back — a write is verified by what it changed.
 */
export const WS_WRITES = [
  "weather_location_request",
  "frontend_log",
  "set_system_volume",
  "set_system_mute",
  "vpn_connect",
  "vpn_disconnect",
  "vpn_ping_all",
  "vpn_set_auto_connect",
];

/**
 * Events that travel over the socket instead of Tauri. This is the whole set the
 * interface subscribes to; the rule is unchanged — an event is added only after a clean
 * `shadow` run, so a divergence is caught before a widget goes silent.
 */
export const WS_EVENTS = [
  "system-volume",
  "audio-bands",
  "sync-tasks",
  "weather-update",
  "wallpaper-wheel",
  "explorer-open",
  "display-changed",
  "ai-dj-progress",
  "telegram-chat-update",
  "telegram-connection-state",
  "telegram-user-status",
  "telegram-attachment-ready",
  "telegram-message-new",
  "telegram-message-edited",
  "telegram-message-deleted",
  "telegram-message-sent",
  "telegram-message-send-failed",
  "telegram-message-reactions",
];

/** Transport of this page load. `?transport=` wins for a one-off probe; the durable
 *  switch is `sessionStorage`, which dies with the WebView session — a forgotten flag
 *  cannot survive a restart the way a `localStorage` one would. */
const TRANSPORT = (() => {
  try {
    const fromUrl = new URLSearchParams(window.location.search).get("transport");
    if (fromUrl) {
      return fromUrl;
    }
    return window.sessionStorage.getItem("unidtk.transport") || "tauri";
  } catch (_) {
    return "tauri";
  }
})();

const state = {
  transport: TRANSPORT,
  sessionInfoCalls: 0,
  wsState: "off", // off | connecting | ready | closed | failed
  wsReconnects: 0,
  pendingCommands: 0,
  /** Commands actually answered over the WebSocket door — proof the path is live. */
  wsCalls: 0,
  /** Per-command tally, so "the migration was verified" names the commands it covered. */
  perCommand: { tauri: {}, ws: {}, shadow: {} },
  /** Last `seq` seen per event, so a reconnect can ask for a replay instead of a guess. */
  eventCursor: {},
  shadowChecks: 0,
  shadowDivergences: 0,
  shadowFailures: 0,
  /** Last few mismatches, so the CLI can show what actually differs. */
  shadowSamples: [],
  shadowFailureSamples: [],
};

function remember(list, entry) {
  list.push(entry);
  if (list.length > 3) {
    list.shift();
  }
}

/** Short, readable rendering of a value for diagnostics. */
function brief(value) {
  try {
    const text = JSON.stringify(value);
    return text && text.length > 400 ? `${text.slice(0, 400)}…` : text;
  } catch (_) {
    return String(value);
  }
}

/**
 * Structural equality, deliberately ignoring key order.
 *
 * The two doors serialise the same struct differently: Tauri writes the fields in
 * declaration order, while the socket path goes through `serde_json::Value`, whose map
 * is sorted. Comparing `JSON.stringify` output therefore reported 16 "divergences" that
 * were the same data with a different key order — noise that would have hidden a real
 * mismatch.
 */
function sameShape(a, b) {
  if (a === b) {
    return true;
  }
  if (a === null || b === null || typeof a !== typeof b) {
    return false;
  }
  if (Array.isArray(a) || Array.isArray(b)) {
    return (
      Array.isArray(a) &&
      Array.isArray(b) &&
      a.length === b.length &&
      a.every((item, index) => sameShape(item, b[index]))
    );
  }
  if (typeof a === "object") {
    const left = Object.keys(a).sort();
    const right = Object.keys(b).sort();
    return (
      left.length === right.length &&
      left.every((key, index) => key === right[index]) &&
      left.every((key) => sameShape(a[key], b[key]))
    );
  }
  return false;
}

// ── Session ────────────────────────────────────────────────────────────────────

let sessionPromise = null;

/** The per-run port and token: fetched at most once per page, never stored. */
function session() {
  if (!sessionPromise) {
    state.sessionInfoCalls += 1;
    sessionPromise = tauriInvoke("session_info")
      .then((info) => {
        if (!info || typeof info.wsPort !== "number" || !info.token) {
          throw new Error("session_info did not return a usable port and token");
        }
        return info;
      })
      .catch((error) => {
        sessionPromise = null; // let a later attempt try again
        throw error;
      });
  }
  return sessionPromise;
}

// ── WebSocket client ───────────────────────────────────────────────────────────

let socket = null;
let socketPromise = null;
let nextId = 1;
const pending = new Map();
const eventHandlers = new Map();
/**
 * The last few **live** payloads per event, for the `shadow` delivery check.
 *
 * A ring rather than the newest value, because the doors are compared by *delivery*, not
 * by order or by last value. Telegram emits bursts (several user-status changes, several
 * messages within a second); pairing "the Tauri payload" with "the newest socket payload"
 * compares two different moments of a stream and reports a divergence that never
 * happened — the socket did deliver that event, just not as the latest one.
 *
 * A snapshot frame (a subscribe catch-up) is not a delivery: it has no Tauri twin, so it
 * never enters the ring. Handlers still receive it — that catch-up is its whole purpose.
 */
const liveEventPayloads = {};
const LIVE_PAYLOAD_RING = 8;
/** Events that already have a shadow comparator registered (one per event is enough). */
const shadowComparators = new Set();

function settle(id, outcome) {
  const entry = pending.get(id);
  if (!entry) {
    return;
  }
  pending.delete(id);
  state.pendingCommands = pending.size;
  clearTimeout(entry.timer);
  if (outcome.error) {
    entry.reject(new Error(outcome.error));
  } else {
    entry.resolve(outcome.result);
  }
}

function dispatchEvent(message) {
  const handlers = eventHandlers.get(message.event);
  if (!handlers) {
    return;
  }
  handlers.forEach((handler) => {
    try {
      handler(message);
    } catch (_) {
      // A broken listener must not take the socket down with it.
    }
  });
}

function onMessage(raw) {
  let message = null;
  try {
    message = JSON.parse(raw);
  } catch (_) {
    return;
  }
  if (!message) {
    return;
  }
  if (message.type === "event") {
    if (typeof message.seq === "number") {
      state.eventCursor[message.event] = message.seq;
    }
    if (message.snapshot !== true) {
      const ring = liveEventPayloads[message.event] || (liveEventPayloads[message.event] = []);
      ring.push(message.payload);
      if (ring.length > LIVE_PAYLOAD_RING) {
        ring.shift();
      }
    }
    dispatchEvent(message);
    return;
  }
  if (message.type === "gap") {
    // The buffer no longer covers our cursor. Re-subscribing without a cursor makes the
    // server answer with the newest value: an explicit resync beats a silent hole.
    console.warn(`[bridge] event gap for '${message.event}'; resyncing`);
    delete state.eventCursor[message.event];
    ensureSubscription(message.event).catch((error) => {
      console.warn(`[bridge] resync failed for '${message.event}': ${error}`);
    });
    return;
  }
  if (typeof message.id !== "number") {
    return;
  }
  if (message.v !== PROTOCOL) {
    settle(message.id, {
      error: `protocol mismatch: the app speaks v${message.v}, this page speaks v${PROTOCOL}`,
    });
    return;
  }
  settle(
    message.id,
    message.ok
      ? { result: message.result }
      : { error: message.error || "command failed" },
  );
}

/**
 * Opens the door once and completes the handshake. The token goes in the first frame;
 * nothing is sent as a command before the app answers `ready`.
 */
async function connect() {
  const info = await session();
  state.wsState = "connecting";

  const opened = await new Promise((resolve, reject) => {
    let ws = null;
    try {
      ws = new WebSocket(`ws://127.0.0.1:${info.wsPort}/`);
    } catch (error) {
      state.wsState = "failed";
      reject(error);
      return;
    }

    const timer = setTimeout(() => {
      state.wsState = "failed";
      try {
        ws.close();
      } catch (_) {}
      reject(new Error("ws handshake timed out"));
    }, HANDSHAKE_TIMEOUT_MS);

    ws.onopen = () => {
      ws.send(JSON.stringify({ hello: { token: info.token } }));
    };

    ws.onmessage = (event) => {
      if (state.wsState === "connecting") {
        let message = null;
        try {
          message = JSON.parse(event.data);
        } catch (_) {}
        if (message && message.ok === true && message.result && message.result.ready) {
          clearTimeout(timer);
          state.wsState = "ready";
          resolve(ws);
          return;
        }
        if (message && message.ok === false) {
          clearTimeout(timer);
          state.wsState = "failed";
          reject(new Error(message.error || "ws handshake rejected"));
          return;
        }
      }
      onMessage(event.data);
    };

    ws.onerror = () => {
      if (state.wsState === "connecting") {
        clearTimeout(timer);
        state.wsState = "failed";
        reject(new Error("ws connection failed"));
      }
    };

    ws.onclose = () => {
      // A close must fail every in-flight request rather than leave it hanging.
      [...pending.keys()].forEach((id) => settle(id, { error: "ws closed before the answer" }));
      if (state.wsState === "ready") {
        state.wsState = "closed";
        state.wsReconnects += 1;
      }
      socket = null;
      socketPromise = null;
    };
  });

  // If the socket died while we were still handshaking, do not publish it.
  if (state.wsState === "ready") {
    socket = opened;
  }
  return opened;
}

/**
 * Returns the single connection of this page.
 *
 * The **promise** is memoized, not just the socket: caching only the socket meant two
 * callers arriving during the handshake each opened their own connection. That leak
 * surfaced as "ws handshake timed out" under load, and it was a bug in this file.
 */
function ensureSocket() {
  if (socket && socket.readyState === WebSocket.OPEN && state.wsState === "ready") {
    return Promise.resolve(socket);
  }
  if (!socketPromise) {
    socketPromise = connect().catch((error) => {
      socketPromise = null;
      throw error;
    });
  }
  return socketPromise;
}

function tally(bucket, command) {
  const counter = state.perCommand[bucket];
  counter[command] = (counter[command] || 0) + 1;
}

function wsCall(command, args) {
  return ensureSocket().then(
    (ws) =>
      new Promise((resolve, reject) => {
        const id = nextId++;
        const timer = setTimeout(() => {
          settle(id, { error: `ws timeout for '${command}'` });
        }, COMMAND_TIMEOUT_MS);
        state.wsCalls += 1;
        tally("ws", command);
        pending.set(id, { resolve, reject, timer });
        state.pendingCommands = pending.size;
        try {
          ws.send(JSON.stringify({ v: PROTOCOL, id, cmd: command, args: args || {} }));
        } catch (error) {
          settle(id, { error: String((error && error.message) || error) });
        }
      }),
  );
}

// ── Public surface ─────────────────────────────────────────────────────────────

/** Diagnostics for `udtk gui js:…` and the harness. Deliberately returns no token. */
export function getTransportState() {
  return {
    transport: state.transport,
    protocol: PROTOCOL,
    migratedCommands: [...WS_COMMANDS],
    migratedWrites: [...WS_WRITES],
    sessionInfoCalls: state.sessionInfoCalls,
    wsState: state.wsState,
    wsReconnects: state.wsReconnects,
    pendingCommands: state.pendingCommands,
    wsCalls: state.wsCalls,
    perCommand: state.perCommand,
    eventCursor: state.eventCursor,
    shadowChecks: state.shadowChecks,
    shadowDivergences: state.shadowDivergences,
    shadowFailures: state.shadowFailures,
    shadowSamples: state.shadowSamples,
    shadowFailureSamples: state.shadowFailureSamples,
  };
}

/**
 * Calls a kernel or module command.
 *
 * `tauri` — everything goes to Tauri (default, and the rollback state).
 * `shadow` — the migrated *reads* are called through *both* doors, the caller gets the
 *            Tauri answer, and a divergence is reported instead of silently trusted.
 *            Writes stay on Tauri here on purpose: see `WS_WRITES`.
 * `ws` — the migrated commands go over WebSocket; everything else stays on Tauri.
 */
export async function invoke(command, args) {
  const migratedRead = WS_COMMANDS.includes(command);
  const migratedWrite = WS_WRITES.includes(command);
  // A write only ever travels on the socket in `ws` mode. In `shadow` it stays on the
  // old door: asking both transports to change something asks for the change twice, and
  // the "comparison" would be two identical records — a green result proving nothing.
  if (migratedWrite && state.transport === "ws") {
    tally("ws", command);
    return wsCall(command, args);
  }
  const wantsWs = state.transport === "ws" || state.transport === "shadow";
  if (!migratedRead || !wantsWs) {
    tally("tauri", command);
    return tauriInvoke(command, args);
  }

  if (state.transport === "shadow") {
    tally("shadow", command);
    const [viaTauri, viaWs] = await Promise.allSettled([
      tauriInvoke(command, args),
      wsCall(command, args),
    ]);
    state.shadowChecks += 1;
    if (viaWs.status === "rejected") {
      // Counted, not merely logged: `getTransportState()` has to be able to prove a
      // clean shadow run before the command is allowed onto the new door.
      state.shadowFailures += 1;
      remember(state.shadowFailureSamples, {
        command,
        reason: String((viaWs.reason && viaWs.reason.message) || viaWs.reason),
      });
      console.warn(`[bridge] shadow: ws failed for '${command}': ${viaWs.reason}`);
    } else if (!sameShape(viaWs.value, viaTauri.value)) {
      state.shadowDivergences += 1;
      remember(state.shadowSamples, {
        command,
        tauri: brief(viaTauri.value),
        ws: brief(viaWs.value),
      });
      console.warn(`[bridge] shadow: divergence for '${command}'`);
    }
    if (viaTauri.status === "rejected") {
      throw viaTauri.reason;
    }
    return viaTauri.value;
  }

  return wsCall(command, args);
}

/** Subscribes to an event.
 *
 * Tauri remains the default. For events listed in `WS_EVENTS` the broker becomes the
 * source once the transport is `shadow` or `ws`: the frame already carries a `seq`, and a
 * `gap` (the buffer no longer covers our cursor) triggers a re-subscribe without a cursor,
 * which makes the server answer with the newest value — a resync, not a silent hole.
 */
export function listen(event, handler) {
  const migrated = WS_EVENTS.includes(event);
  const wantsWs = state.transport === "ws" || state.transport === "shadow";
  if (!migrated || !wantsWs) {
    tally("tauri", `listen:${event}`);
    return tauriListen(event, handler);
  }

  tally("ws", `listen:${event}`);
  let handlers = eventHandlers.get(event);
  if (!handlers) {
    handlers = new Set();
    eventHandlers.set(event, handlers);
  }
  handlers.add(handler);
  ensureSubscription(event).catch((error) => {
    console.warn(`[bridge] ws subscribe failed for '${event}': ${error}`);
  });

  if (state.transport === "shadow" && !shadowComparators.has(event)) {
    // One comparator per event, not one per `listen` call: several widgets subscribe to
    // the same event, and registering a comparator each time made them consume the same
    // buffered socket payload twice — reporting a mismatch that was not there.
    shadowComparators.add(event);
    // For a **state** event the right question is not "did the frames arrive in the same
    // order" (they travel asynchronously, and the subscribe snapshot has no Tauri twin to
    // pair with) but "do both doors converge to the same value shortly after a change".
    // Ordering matters for stream events; those need their own rule.
    tauriListen(event, (tauriEvent) => {
      setTimeout(() => {
        const ring = liveEventPayloads[event];
        if (!ring || ring.length === 0) {
          return; // no live socket delivery for this event yet
        }
        state.shadowChecks += 1;
        // Delivery, not equality: the socket must have passed on *this* event, whether or
        // not newer ones have arrived since.
        if (!ring.some((payload) => sameShape(payload, tauriEvent.payload))) {
          state.shadowDivergences += 1;
          remember(state.shadowSamples, {
            command: `event:${event}`,
            tauri: brief(tauriEvent.payload),
            ws: brief(ring[ring.length - 1]),
          });
        }
      }, 250);
    });
  }

  return Promise.resolve(() => {
    handlers.delete(handler);
    if (handlers.size === 0) {
      eventHandlers.delete(event);
      sendFrame({ v: PROTOCOL, type: "unsubscribe", event });
    }
  });
}

/** Subscribes on the socket, asking for a replay when we already have a cursor. */
function ensureSubscription(event) {
  return ensureSocket().then(() => {
    const since = state.eventCursor[event];
    const frame = { v: PROTOCOL, type: "subscribe", event };
    if (typeof since === "number") {
      frame.since = since;
    }
    sendFrame(frame);
  });
}

function sendFrame(frame) {
  if (!socket || socket.readyState !== WebSocket.OPEN) {
    return;
  }
  try {
    socket.send(JSON.stringify(frame));
  } catch (error) {
    console.warn(`[bridge] ws send failed: ${error}`);
  }
}

/**
 * Bootstrap for the page's own door: the WebSocket port and the per-run token.
 * Always Tauri — asking for the session over the session would be circular.
 */
export async function sessionInfo() {
  return session();
}

/** Diagnostics handle for the CLI: `udtk gui js:window.__UNIDTK_BRIDGE__.probe()`. */
window.__UNIDTK_BRIDGE__ = {
  getTransportState,
  probe: async () => {
    await ensureSocket();
    return getTransportState();
  },
};

// Warm the door once when it is actually in use, so the first migrated command does not
// pay for the handshake (on a cold page the app is still booting its modules).
if (TRANSPORT !== "tauri") {
  ensureSocket().catch((error) => {
    state.wsState = "failed";
    console.warn(`[bridge] ws warm-up failed: ${error}`);
  });
}
