//! rox addition: what a cached split or tab group watches to know its
//! chrome went stale.
//!
//! A stack's children render through `.cached()`, so a frame asked for by
//! one panel replays every other group instead of rebuilding the whole dock.
//! gpui only redraws a cached view when it or a descendant notifies, and the
//! tab bar also reads state that lives above it: the dock area's lock and
//! panel style, the parent splits' shape, the inactive tabs' titles. Each
//! group subscribes to those and notifies itself when they move. Settings
//! that live in statics (design mode, resize lock, the focus ring) repaint
//! through `window.refresh()`, which redraws cached views too.

use std::collections::HashMap;
use std::sync::Arc;

use gpui::{App, Entity, EntityId, Subscription};

use crate::PanelView;

pub(crate) trait Watched {
    fn watched_id(&self) -> EntityId;
    fn watch(&self, watcher: EntityId, cx: &mut App) -> Subscription;
}

impl<T: 'static> Watched for Entity<T> {
    fn watched_id(&self) -> EntityId {
        self.entity_id()
    }

    fn watch(&self, watcher: EntityId, cx: &mut App) -> Subscription {
        cx.observe(self, move |_, cx| cx.notify(watcher))
    }
}

impl Watched for Arc<dyn PanelView> {
    fn watched_id(&self) -> EntityId {
        self.view().entity_id()
    }

    fn watch(&self, watcher: EntityId, cx: &mut App) -> Subscription {
        self.notify_on_change(watcher, cx)
    }
}

/// One subscription per watched entity, kept in step with the current set.
#[derive(Default)]
pub(crate) struct Watch(HashMap<EntityId, Subscription>);

impl Watch {
    pub(crate) fn sync(&mut self, watcher: EntityId, targets: &[&dyn Watched], cx: &mut App) {
        self.0
            .retain(|id, _| targets.iter().any(|target| target.watched_id() == *id));

        for target in targets {
            self.0
                .entry(target.watched_id())
                .or_insert_with(|| target.watch(watcher, cx));
        }
    }
}
