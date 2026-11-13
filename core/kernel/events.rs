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

use std::collections::{HashMap, HashSet, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};

use serde_json::Value;

/// How many frames of one event are kept for replay.
const RING_CAPACITY: usize = 64;

/// Events whose value is a *state*, not a stream of events. For these only the newest
/// frame matters, so the ring holds one entry and a subscriber gets the latest value
/// immediately on subscribe. The modules declare them (`Module::state_events`), and the
/// kernel hands the list over once, before the first event is published.
static STATE_EVENTS: OnceLock<Vec<&'static str>> = OnceLock::new();

/// Records which events are states. Called once, from `setup`, before `set_emitter`:
/// a state's first frame published earlier would be kept as a stream. A second call
/// is refused, so the table cannot change under a running broker.
pub fn declare_state_events(events: Vec<&'static str>) -> Result<(), String> {
    STATE_EVENTS
        .set(events)
        .map_err(|_| "the state events are already declared".to_string())
}

/// One delivered frame. `seq` is assigned by the broker before fan-out, which is what
/// makes ordering claims possible.
#[derive(Clone, Debug)]
pub struct Frame {
    pub seq: u64,
    pub event: String,
    pub payload: Value,
}

/// Default bounded capacity per subscriber stream queue before emitting an explicit gap.
const SUBSCRIBER_QUEUE_CAPACITY: usize = 64;

#[derive(Default)]
struct Broker {
    seq: AtomicU64,
    rings: Mutex<HashMap<String, VecDeque<Frame>>>,
    subscribers: Mutex<Vec<(u64, SubscriberSink)>>,
    next_subscriber: AtomicU64,
}

enum SubscriberSink {
    Legacy(crossbeam_channel::Sender<Frame>),
    Managed(Arc<SubscriberState>),
}

struct SubscriberState {
    capacity: usize,
    wake: WakeHook,
    inner: Mutex<SubscriberInner>,
}

struct SubscriberInner {
    topics: HashSet<String>,
    stream_frames: VecDeque<Frame>,
    state_frames: Vec<Frame>,
    gaps: VecDeque<(String, u64)>,
    last_seq_by_event: HashMap<String, u64>,
}

impl SubscriberInner {
    fn push(&mut self, frame: &Frame, capacity: usize) -> bool {
        if !self.topics.contains(&frame.event) {
            return false;
        }
        if is_state_event(&frame.event) {
            self.last_seq_by_event.insert(frame.event.clone(), frame.seq);
            if let Some(slot) = self.state_frames.iter_mut().find(|f| f.event == frame.event) {
                *slot = frame.clone();
            } else {
                self.state_frames.push(frame.clone());
            }
            return true;
        }
        if self.stream_frames.len() >= capacity {
            if !self.gaps.iter().any(|(ev, _)| ev == &frame.event) {
                let since = self
                    .last_seq_by_event
                    .get(&frame.event)
                    .copied()
                    .unwrap_or_else(|| frame.seq.saturating_sub(1));
                self.gaps.push_back((frame.event.clone(), since));
                return true;
            }
            return false;
        }
        self.last_seq_by_event.insert(frame.event.clone(), frame.seq);
        self.stream_frames.push_back(frame.clone());
        true
    }
}

fn broker() -> &'static Broker {
    static BROKER: OnceLock<Broker> = OnceLock::new();
    BROKER.get_or_init(Broker::default)
}

fn is_state_event(event: &str) -> bool {
    STATE_EVENTS.get().is_some_and(|events| events.contains(&event))
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
    let mut wakes: Vec<WakeHook> = Vec::new();
    {
        // The number is taken under the lock that orders the rings: taken before it, an
        // older frame could land last and stay as the state a new client is given.
        let Ok(mut rings) = broker.rings.lock() else {
            return;
        };
        let seq = broker.seq.fetch_add(1, Ordering::SeqCst) + 1;
        let frame = Frame {
            seq,
            event: event.to_string(),
            payload,
        };

        {
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

        // Still under the rings lock, so subscribers see the same order.
        if let Ok(subscribers) = broker.subscribers.lock() {
            for (_, sink) in subscribers.iter() {
                match sink {
                    SubscriberSink::Legacy(sender) => {
                        let _ = sender.try_send(frame.clone());
                    }
                    SubscriberSink::Managed(state) => {
                        if let Ok(mut inner) = state.inner.lock() {
                            if inner.push(&frame, state.capacity) {
                                wakes.push(state.wake.clone());
                            }
                        }
                    }
                }
            }
        }
    }
    for wake in wakes {
        wake();
    }
}

/// Registers a subscriber and returns its id.
pub fn subscribe(sender: crossbeam_channel::Sender<Frame>) -> u64 {
    let broker = broker();
    let id = broker.next_subscriber.fetch_add(1, Ordering::SeqCst) + 1;
    if let Ok(mut subscribers) = broker.subscribers.lock() {
        subscribers.push((id, SubscriberSink::Legacy(sender)));
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

fn replay_from_rings(
    rings: &HashMap<String, VecDeque<Frame>>,
    event: &str,
    cursor: u64,
) -> Option<Vec<Frame>> {
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

#[derive(Clone, Debug)]
pub enum Delivery {
    Frame(Frame),
    Gap { event: String, since: u64 },
}

#[derive(Clone, Debug)]
pub enum SubscribeInit {
    Snapshot(Option<Frame>),
    Replay(Vec<Frame>),
    Gap { since: u64 },
}

pub type WakeHook = Arc<dyn Fn() + Send + Sync>;

pub struct Subscription {
    id: u64,
    state: Arc<SubscriberState>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        unsubscribe(self.id);
    }
}

impl Subscription {
    /// Atomically subscribes `event` under the rings lock so catchup and live fan-out
    /// have zero gap or reordering window across concurrent `publish` calls.
    pub fn subscribe_topic(&self, event: &str, since: Option<u64>) -> SubscribeInit {
        let Ok(rings) = broker().rings.lock() else {
            return match since {
                None => SubscribeInit::Snapshot(None),
                Some(c) => SubscribeInit::Gap { since: c },
            };
        };
        let init = match since {
            None => SubscribeInit::Snapshot(rings.get(event).and_then(|ring| ring.back().cloned())),
            Some(c) => match replay_from_rings(&rings, event, c) {
                Some(frames) => SubscribeInit::Replay(frames),
                None => SubscribeInit::Gap { since: c },
            },
        };
        if let Ok(mut inner) = self.state.inner.lock() {
            inner.topics.insert(event.to_string());
            if let Some(newest) = rings.get(event).and_then(|r| r.back()).map(|f| f.seq) {
                inner.last_seq_by_event.insert(event.to_string(), newest);
            }
        }
        init
    }

    pub fn unsubscribe_topic(&self, event: &str) {
        if let Ok(mut inner) = self.state.inner.lock() {
            inner.topics.remove(event);
            inner.stream_frames.retain(|f| f.event != event);
            inner.state_frames.retain(|f| f.event != event);
            inner.gaps.retain(|(ev, _)| ev != event);
        }
    }

    pub fn has_topics(&self) -> bool {
        self.state
            .inner
            .lock()
            .map(|inner| !inner.topics.is_empty())
            .unwrap_or(false)
    }

    pub fn drain(&self) -> Vec<Delivery> {
        let Ok(mut inner) = self.state.inner.lock() else {
            return Vec::new();
        };
        if inner.stream_frames.is_empty() && inner.state_frames.is_empty() && inner.gaps.is_empty() {
            return Vec::new();
        }
        let mut frames: Vec<Frame> = inner.stream_frames.drain(..).collect();
        frames.extend(inner.state_frames.drain(..));
        frames.sort_by_key(|f| f.seq);

        let mut out: Vec<Delivery> = frames.into_iter().map(Delivery::Frame).collect();
        let gaps: Vec<(String, u64)> = inner.gaps.drain(..).collect();
        for (event, since) in gaps {
            out.push(Delivery::Gap { event, since });
        }
        out
    }
}

pub fn subscribe_with_wake(wake: WakeHook) -> Subscription {
    subscribe_with_capacity(SUBSCRIBER_QUEUE_CAPACITY, wake)
}

pub fn subscribe_with_capacity(capacity: usize, wake: WakeHook) -> Subscription {
    let broker = broker();
    let id = broker.next_subscriber.fetch_add(1, Ordering::SeqCst) + 1;
    let state = Arc::new(SubscriberState {
        capacity: capacity.max(1),
        wake,
        inner: Mutex::new(SubscriberInner {
            topics: HashSet::new(),
            stream_frames: VecDeque::new(),
            state_frames: Vec::new(),
            gaps: VecDeque::new(),
            last_seq_by_event: HashMap::new(),
        }),
    });
    if let Ok(mut subscribers) = broker.subscribers.lock() {
        subscribers.push((id, SubscriberSink::Managed(state.clone())));
    }
    Subscription { id, state }
}

/// Frames after `cursor` for one event, or `None` when the ring no longer covers it —
/// the caller must then tell the client about the gap rather than send a partial stream.
pub fn replay(event: &str, cursor: u64) -> Option<Vec<Frame>> {
    let rings = broker().rings.lock().ok()?;
    replay_from_rings(&rings, event, cursor)
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

    /// The broker's table is set once per process, so every test that needs it
    /// declares the same list.
    fn declare_test_states() {
        let _ = declare_state_events(vec![
            "state-test",
            "state-coalesce-isolated",
            "mt-state-isolated",
        ]);
    }

    #[test]
    fn state_events_keep_only_the_newest_frame() {
        declare_test_states();
        for value in 0..5 {
            publish("state-test", serde_json::json!({ "level": value }));
        }
        let newest = snapshot("state-test").unwrap();
        assert_eq!(newest.payload["level"], 4);
        let replayed = replay("state-test", 0).unwrap();
        assert_eq!(replayed.len(), 1, "a state event must not replay a stream");
    }

    #[test]
    fn the_state_table_is_declared_once() {
        declare_test_states();
        assert!(declare_state_events(vec!["other"]).is_err());
        assert!(is_state("state-test") && !is_state("other"));
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

    #[test]
    fn overflow_emits_explicit_gap_instead_of_silent_drop() {
        let sub = subscribe_with_capacity(4, std::sync::Arc::new(|| {}));
        let _ = sub.subscribe_topic("stream-gap", None);
        for i in 1..=10 {
            publish("stream-gap", serde_json::json!({ "n": i }));
        }
        let delivered = sub.drain();
        let has_gap = delivered.iter().any(|item| matches!(item, Delivery::Gap { event, .. } if event == "stream-gap"));
        assert!(has_gap, "overflow on bounded stream queue must emit explicit Delivery::Gap");
    }

    #[test]
    fn state_events_coalesce_without_overflowing_stream_queue() {
        declare_test_states();
        let sub = subscribe_with_capacity(2, std::sync::Arc::new(|| {}));
        let _ = sub.subscribe_topic("state-coalesce-isolated", None);
        let _ = sub.subscribe_topic("stream-safe", None);
        publish("stream-safe", serde_json::json!(1));
        for v in 0..20 {
            publish("state-coalesce-isolated", serde_json::json!({ "level": v }));
        }
        publish("stream-safe", serde_json::json!(2));

        let delivered = sub.drain();
        assert!(
            !delivered.iter().any(|item| matches!(item, Delivery::Gap { .. })),
            "coalesced state burst must not cause stream gap"
        );
        let state_frames: Vec<_> = delivered
            .iter()
            .filter_map(|d| match d {
                Delivery::Frame(f) if f.event == "state-coalesce-isolated" => Some(f),
                _ => None,
            })
            .collect();
        let stream_frames: Vec<_> = delivered
            .iter()
            .filter_map(|d| match d {
                Delivery::Frame(f) if f.event == "stream-safe" => Some(f),
                _ => None,
            })
            .collect();
        assert_eq!(state_frames.len(), 1);
        assert_eq!(state_frames[0].payload["level"], 19);
        assert_eq!(stream_frames.len(), 2);
    }

    #[test]
    fn wake_hook_fires_only_for_subscribed_topics() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let wakes = std::sync::Arc::new(AtomicUsize::new(0));
        let w = wakes.clone();
        let sub = subscribe_with_capacity(8, std::sync::Arc::new(move || {
            w.fetch_add(1, Ordering::SeqCst);
        }));
        let _ = sub.subscribe_topic("wake-yes", None);
        publish("wake-no", serde_json::json!(1));
        assert_eq!(wakes.load(Ordering::SeqCst), 0, "unrelated topics must not wake");
        publish("wake-yes", serde_json::json!(2));
        assert_eq!(wakes.load(Ordering::SeqCst), 1, "subscribed topic must wake once");
    }

    #[test]
    fn multi_topic_subscribe_preserves_seq_order_and_catchup() {
        declare_test_states();
        publish("mt-stream", serde_json::json!(1));
        let cursor = snapshot("mt-stream").unwrap().seq;
        publish("mt-stream", serde_json::json!(2));
        publish("mt-state-isolated", serde_json::json!({ "level": 42 }));

        let sub = subscribe_with_capacity(16, std::sync::Arc::new(|| {}));
        let init1 = sub.subscribe_topic("mt-stream", Some(cursor));
        let init2 = sub.subscribe_topic("mt-state-isolated", None);

        match init1 {
            SubscribeInit::Replay(frames) => {
                assert_eq!(frames.len(), 1);
                assert_eq!(frames[0].payload, serde_json::json!(2));
            }
            other => panic!("expected replay, got {other:?}"),
        }
        match init2 {
            SubscribeInit::Snapshot(Some(frame)) => {
                assert_eq!(frame.payload["level"], 42);
            }
            other => panic!("expected snapshot, got {other:?}"),
        }

        publish("mt-stream", serde_json::json!(3));
        let drained = sub.drain();
        assert_eq!(drained.len(), 1);
    }
}
