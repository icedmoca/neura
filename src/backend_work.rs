//! Typed coordinator for coalescible backend maintenance work.
//!
//! This is the shared entry point backend subsystems should use when work can be
//! prioritized, deduplicated, and safely retried later. It deliberately does not
//! execute model/tool work itself; it coordinates maintenance-style work such as
//! session persistence, telemetry flushes, cache refreshes, and index updates.

use crate::work_queue::{QueuePushResult, WorkPriority, WorkQueue};
use std::path::PathBuf;

#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub enum BackendWorkKey {
    SessionSave(PathBuf),
    TelemetryFlush,
    CacheRefresh(&'static str),
    IndexRefresh(&'static str),
    Custom(&'static str),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendWorkItem {
    SessionSave { path: PathBuf },
    TelemetryFlush,
    CacheRefresh { name: &'static str },
    IndexRefresh { name: &'static str },
    Custom { name: &'static str },
}

impl BackendWorkItem {
    pub fn key(&self) -> BackendWorkKey {
        match self {
            Self::SessionSave { path } => BackendWorkKey::SessionSave(path.clone()),
            Self::TelemetryFlush => BackendWorkKey::TelemetryFlush,
            Self::CacheRefresh { name } => BackendWorkKey::CacheRefresh(name),
            Self::IndexRefresh { name } => BackendWorkKey::IndexRefresh(name),
            Self::Custom { name } => BackendWorkKey::Custom(name),
        }
    }

    pub fn default_priority(&self) -> WorkPriority {
        match self {
            Self::SessionSave { .. } => WorkPriority::High,
            Self::TelemetryFlush => WorkPriority::Low,
            Self::CacheRefresh { .. } => WorkPriority::Normal,
            Self::IndexRefresh { .. } => WorkPriority::Normal,
            Self::Custom { .. } => WorkPriority::Normal,
        }
    }
}

#[derive(Debug, Clone)]
pub struct BackendWorkQueue {
    inner: WorkQueue<BackendWorkKey, BackendWorkItem>,
}

impl BackendWorkQueue {
    pub fn new(max_len: usize) -> Self {
        Self {
            inner: WorkQueue::new(max_len),
        }
    }

    pub fn len(&self) -> usize {
        self.inner.len()
    }

    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    pub fn enqueue(&mut self, item: BackendWorkItem) -> QueuePushResult<BackendWorkItem> {
        let priority = item.default_priority();
        self.enqueue_with_priority(item, priority)
    }

    pub fn enqueue_with_priority(
        &mut self,
        item: BackendWorkItem,
        priority: WorkPriority,
    ) -> QueuePushResult<BackendWorkItem> {
        self.inner.push(item.key(), priority, item)
    }

    pub fn pop(&mut self) -> Option<BackendWorkItem> {
        self.inner.pop().map(|(_, item)| item)
    }
}

impl Default for BackendWorkQueue {
    fn default() -> Self {
        Self::new(256)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn coalesces_duplicate_session_saves() {
        let path = PathBuf::from("session.json");
        let mut queue = BackendWorkQueue::default();
        assert!(matches!(
            queue.enqueue(BackendWorkItem::SessionSave { path: path.clone() }),
            QueuePushResult::Inserted
        ));
        assert!(matches!(
            queue.enqueue(BackendWorkItem::SessionSave { path: path.clone() }),
            QueuePushResult::Coalesced { .. }
        ));
        assert_eq!(queue.len(), 1);
        assert_eq!(queue.pop(), Some(BackendWorkItem::SessionSave { path }));
    }

    #[test]
    fn session_saves_outrank_telemetry_flushes() {
        let mut queue = BackendWorkQueue::default();
        queue.enqueue(BackendWorkItem::TelemetryFlush);
        queue.enqueue(BackendWorkItem::SessionSave {
            path: PathBuf::from("session.json"),
        });
        assert!(matches!(
            queue.pop(),
            Some(BackendWorkItem::SessionSave { .. })
        ));
        assert_eq!(queue.pop(), Some(BackendWorkItem::TelemetryFlush));
    }
}
