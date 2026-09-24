//! The push half of the surface: a registry of subscribed connections the
//! app broadcasts into from the UI thread. An emit never blocks; a subscriber
//! whose buffer is full is cut off, and reconnecting is how it catches up.

use std::sync::mpsc::{SyncSender, TrySendError};
use std::sync::{Arc, Mutex};

use serde_json::Value;

use crate::protocol::EventFrame;

/// Clone is a second handle over the same registry.
#[derive(Clone)]
pub struct Events {
    subscribers: Arc<Mutex<Vec<Subscriber>>>,
}

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

    pub(crate) fn register(&self, tx: SyncSender<Arc<[u8]>>, kill: Arc<dyn Fn() + Send + Sync>) {
        self.subscribers
            .lock()
            .unwrap()
            .push(Subscriber { tx, kill });
    }

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
