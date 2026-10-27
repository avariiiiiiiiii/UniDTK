//! Event broker: the single place that numbers and fans out events to the UI.
//!
//! Why it exists: the WebSocket door only knew request/response, so half the interface —
//! everything built on `listen` — could never move off Tauri. The broker gives events a
//! protocol of their own instead of pretending they are commands:
//!
//! - **a global `seq`**, so a client can say "I last saw 41" and mean it;
//! - **a bounded ring per event**, so a short reconnect can be replayed instead of losing
//!   whatever happened while the socket was down;
//! - **a coalescing rule for high-frequency state**, because replaying every 120 ms
//!   volume sample to a slow WebView buys nothing and costs memory;
//! - **an explicit gap signal**: if the ring no longer covers the cursor, the client is
//!   told instead of being handed a silently incomplete sequence.
//!
//! Trust note: this module holds no token and does not decide who may subscribe — the
//! authenticated WS connection does that before it gets here.

use std::collections::HashMap;
use std::collections::VecDeque;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Mutex, OnceLock};

use serde_json::Value;

/// How many frames of one event are kept for replay.
const RING_CAPACITY: usize = 64;

/// Events whose value is a *state*, not a stream of events. For these only the newest
/// frame matters, so the ring holds one entry and a subscriber gets the latest value
/// immediately on subscribe.
const STATE_EVENTS: &[&str] = &[
    "system-volume",
    "audio-bands",
    "vpn",
    "telegram-connection-state",
    "sync-tasks",
    // A current-conditions reading is a state, not a stream: an observer wants
    // the latest, never all fifteen minutes of history.
    "weather-update",
];

/// One delivered frame. `seq` is assigned by the broker before fan-out, which is what
/// makes ordering claims possible.
#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: u64,
    pub event: String,
    pub payload: Value,
}

#[derive(Default)]
struct Broker {
    seq: AtomicU64,
    rings: Mutex<HashMap<String, VecDeque<Frame>>>,
    subscribers: Mutex<Vec<(u64, crossbeam_channel::Sender<Frame>)>>,
    next_subscriber: AtomicU64,
}

fn broker() -> &'static Broker {
    static BROKER: OnceLock<Broker> = OnceLock::new();
    BROKER.get_or_init(Broker::default)
}

fn is_state_event(event: &str) -> bool {
    STATE_EVENTS.contains(&event)
}

/// Whether only the newest value of this event matters (see `STATE_EVENTS`).
///
/// The WebSocket door uses this to coalesce a burst into one message: replaying thirty
/// 120 ms spectrum samples to a WebView buys nothing, one current value does.
pub fn is_state(event: &str) -> bool {
    is_state_event(event)
}

/// Numbers an event and pushes it to every subscriber.
///
/// Called from the kernel's emitter, so the Tauri door and this broker carry the same
/// events with the same numbering.
pub fn publish(event: &str, payload: Value) {
    let broker = broker();
    let seq = broker.seq.fetch_add(1, Ordering::SeqCst) + 1;
    let frame = Frame {
        seq,
        event: event.to_string(),
        payload,
    };

    if let Ok(mut rings) = broker.rings.lock() {
        let ring = rings.entry(frame.event.clone()).or_default();
        if is_state_event(&frame.event) {
            // Only the newest value is worth remembering.
            ring.clear();
        }
        ring.push_back(frame.clone());
        while ring.len() > RING_CAPACITY {
            ring.pop_front();
        }
    }

    if let Ok(subscribers) = broker.subscribers.lock() {
        for (_, sender) in subscribers.iter() {
            // A slow or dead client must never block the kernel: try_send drops instead.
            let _ = sender.try_send(frame.clone());
        }
    }
}

/// Registers a subscriber and returns its id.
pub fn subscribe(sender: crossbeam_channel::Sender<Frame>) -> u64 {
    let broker = broker();
    let id = broker.next_subscriber.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut subscribers) = broker.subscribers.lock() {
        subscribers.push((id, sender));
    }
    id
}

/// Removes a subscriber. Called when its connection ends.
pub fn unsubscribe(id: u64) {
    if let Ok(mut subscribers) = broker().subscribers.lock() {
        subscribers.retain(|(existing, _)| *existing != id);
    }
}

/// What a fresh subscriber must be told: the newest value of each state event, so a page
/// that just connected is not blind until the next change.
pub fn snapshot(event: &str) -> Option<Frame> {
    let rings = broker().rings.lock().ok()?;
    rings.get(event).and_then(|ring| ring.back().cloned())
}

/// Frames after `cursor` for one event, or `None` when the ring no longer covers it —
/// the caller must then tell the client about the gap rather than send a partial stream.
pub fn replay(event: &str, cursor: u64) -> Option<Vec<Frame>> {
    let rings = broker().rings.lock().ok()?;
    let ring = rings.get(event)?;
    if ring.is_empty() {
        return Some(Vec::new());
    }
    let oldest = ring.front().map(|frame| frame.seq).unwrap_or(0);
    if cursor + 1 < oldest && !is_state_event(event) {
        return None;
    }
    Some(
        ring.iter()
            .filter(|frame| frame.seq > cursor)
            .cloned()
            .collect(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sequence_is_monotonic_across_events() {
        let first = {
            publish("test-a", serde_json::json!(1));
            snapshot("test-a").unwrap().seq
        };
        publish("test-b", serde_json::json!(2));
        let second = snapshot("test-b").unwrap().seq;
        assert!(second > first, "seq must keep increasing across events");
    }

    #[test]
    fn state_events_keep_only_the_newest_frame() {
        for value in 0..5 {
            publish("system-volume", serde_json::json!({ "level": value }));
        }
        let newest = snapshot("system-volume").unwrap();
        assert_eq!(newest.payload["level"], 4);
        let replayed = replay("system-volume", 0).unwrap();
        assert_eq!(replayed.len(), 1, "a state event must not replay a stream");
    }

    #[test]
    fn stream_replay_returns_only_frames_after_the_cursor() {
        publish("stream-test", serde_json::json!(1));
        let cursor = snapshot("stream-test").unwrap().seq;
        publish("stream-test", serde_json::json!(2));
        publish("stream-test", serde_json::json!(3));
        let frames = replay("stream-test", cursor).unwrap();
        assert_eq!(frames.len(), 2);
        assert!(frames.iter().all(|frame| frame.seq > cursor));
    }

    #[test]
    fn a_subscriber_receives_published_frames() {
        let (sender, receiver) = crossbeam_channel::bounded(64);
        let id = subscribe(sender);
        publish("subscriber-test", serde_json::json!({ "ok": true }));

        // The broker is global and every subscriber sees every event, so other tests
        // publishing in parallel can land here first — look for our frame instead of
        // assuming it is next in line.
        let mut found = false;
        while let Ok(frame) = receiver.try_recv() {
            if frame.event == "subscriber-test" {
                assert_eq!(frame.payload["ok"], true);
                found = true;
                break;
            }
        }
        unsubscribe(id);
        assert!(found, "subscriber must receive the frame it published");
    }
}
