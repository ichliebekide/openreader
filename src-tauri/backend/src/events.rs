//! Port of `openreader_backend/events.py`.
//!
//! Every subscriber has a bounded queue (16 events). A slow subscriber is never
//! disconnected; instead its oldest queued event is dropped.

use std::collections::VecDeque;
use std::sync::{Arc, Mutex, Weak};

use serde::Serialize;
use tokio::sync::Notify;

use crate::models::{FallbackEvent, PlaybackEvent, SelectionEvent};

pub const SUBSCRIBER_QUEUE: usize = 16;

/// Messages on `WS /ws/events`, discriminated by their `kind` field.
#[derive(Clone, Debug, PartialEq, Serialize, ts_rs::TS)]
#[serde(untagged)]
pub enum ReaderEvent {
    Selection(SelectionEvent),
    Playback(PlaybackEvent),
    Fallback(FallbackEvent),
}

impl ReaderEvent {
    pub fn to_json(&self) -> String {
        serde_json::to_string(self).expect("event serialize")
    }
}

struct Queue {
    events: Mutex<VecDeque<ReaderEvent>>,
    notify: Notify,
}

#[derive(Default)]
pub struct EventHub {
    subscribers: Mutex<Vec<Weak<Queue>>>,
}

impl EventHub {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn publish(&self, event: ReaderEvent) {
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|queue| queue.strong_count() > 0);
        for queue in subscribers.iter().filter_map(Weak::upgrade) {
            let mut events = queue.events.lock().unwrap();
            if events.len() >= SUBSCRIBER_QUEUE {
                events.pop_front();
            }
            events.push_back(event.clone());
            drop(events);
            queue.notify.notify_one();
        }
    }

    pub fn subscribe(&self) -> Subscription {
        let queue = Arc::new(Queue {
            events: Mutex::new(VecDeque::new()),
            notify: Notify::new(),
        });
        self.subscribers.lock().unwrap().push(Arc::downgrade(&queue));
        Subscription { queue }
    }

    pub fn subscriber_count(&self) -> usize {
        let mut subscribers = self.subscribers.lock().unwrap();
        subscribers.retain(|queue| queue.strong_count() > 0);
        subscribers.len()
    }
}

/// Dropping a subscription unsubscribes it.
pub struct Subscription {
    queue: Arc<Queue>,
}

impl Subscription {
    pub async fn next(&mut self) -> ReaderEvent {
        loop {
            if let Some(event) = self.queue.events.lock().unwrap().pop_front() {
                return event;
            }
            self.queue.notify.notified().await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::PlaybackState;
    use std::time::Duration;

    fn playback(revision: i64) -> ReaderEvent {
        ReaderEvent::Playback(PlaybackEvent::new(
            PlaybackState {
                revision,
                ..Default::default()
            },
            None,
        ))
    }

    fn revision(event: &ReaderEvent) -> i64 {
        match event {
            ReaderEvent::Playback(event) => event.state.revision,
            _ => panic!("unexpected event"),
        }
    }

    #[tokio::test]
    async fn slow_event_subscriber_is_not_disconnected() {
        let hub = EventHub::new();
        let mut stream = hub.subscribe();
        for i in 0..100 {
            hub.publish(playback(i));
        }
        let first = tokio::time::timeout(Duration::from_millis(100), stream.next()).await.unwrap();
        assert_eq!(revision(&first), 84);
        assert_eq!(hub.subscriber_count(), 1);
        // Drain what is left, then publish 16 more.
        for _ in 0..15 {
            stream.next().await;
        }
        for i in 0..16 {
            hub.publish(playback(200 + i));
        }
        let mut events = Vec::new();
        for _ in 0..16 {
            events.push(tokio::time::timeout(Duration::from_millis(100), stream.next()).await.unwrap());
        }
        assert_eq!(revision(events.last().unwrap()), 215);
        drop(stream);
        assert_eq!(hub.subscriber_count(), 0);
    }
}
