//! The push half of the surface: a registry of subscribed connections the
//! app broadcasts into. The app holds one [`Events`] handle and emits from
//! the UI thread; each emit serializes the frame once and hands it to every
//! subscriber's outbound channel without ever blocking, so a slow or dead
//! consumer costs the player nothing. A subscriber whose buffer is full has
//! stopped draining and gets cut off instead of accumulating, because a
//! consumer that fell a bufferful behind holds a broken picture anyway and
//! reconnecting is how it gets a true one.

use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::protocol::EventFrame;

/// The broadcast handle [`Server::spawn`](crate::Server::spawn) returns. The
/// app emits through it; connections register through it when their client
/// subscribes. Clone is a second handle over the same registry.
#[derive(Clone)]
pub struct Events {
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

/// One subscribed connection: its outbound channel, and the lever that
/// forces the connection down when it can't keep up.
struct Subscriber {
    tx: SyncSender<Arc<[u8]>>,
    kill: Arc<dyn Fn() + Send + Sync>,
}

impl Events {
    pub(crate) fn new() -> Events {
        Events {
            subscribers: Arc::new(Mutex::new(Vec::new())),
        }
    }

    /// Enroll a connection's outbound channel; every emit from here on
    /// lands in it. Called by the connection thread on `subscribe`.
    pub(crate) fn register(&self, tx: SyncSender<Arc<[u8]>>, kill: Arc<dyn Fn() + Send + Sync>) {
        self.subscribers
            .lock()
            .unwrap()
            .push(Subscriber { tx, kill });
    }

    /// Push one event to every subscriber. Never blocks: a full buffer
    /// means the consumer stopped reading, so it's disconnected and dropped
    /// from the registry; a gone one (its connection already died) is just
    /// dropped.
    pub fn emit(&self, method: &str, params: Value) {
        let mut subscribers = self.subscribers.lock().unwrap();
        if subscribers.is_empty() {
            return;
        }
        let frame = EventFrame {
            jsonrpc: "2.0",
            method,
            params: &params,
        };
        let Ok(mut bytes) = serde_json::to_vec(&frame) else {
            return;
        };
        bytes.push(b'\n');
        let bytes: Arc<[u8]> = bytes.into();
        subscribers.retain(|sub| match sub.tx.try_send(bytes.clone()) {
            Ok(()) => true,
            Err(TrySendError::Full(_)) => {
                (sub.kill)();
                false
            }
            Err(TrySendError::Disconnected(_)) => false,
        });
    }
}
