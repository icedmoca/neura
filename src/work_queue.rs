//! Priority work queue for coalescible backend maintenance tasks.
//!
//! This queue is intentionally small and deterministic. It is not a general async
//! executor. It helps subsystems coordinate background work by providing:
//!
//! - priority ordering,
//! - bounded capacity/backpressure,
//! - deduplication/coalescing by key.

use std::cmp::Ordering;
use std::collections::{BinaryHeap, HashMap};
use std::hash::Hash;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum WorkPriority {
    Low = 0,
    Normal = 1,
    High = 2,
    Critical = 3,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum QueuePushResult<T> {
    Inserted,
    Coalesced { old: T },
    Dropped { item: T },
}

#[derive(Debug, Clone)]
struct HeapEntry<K> {
    priority: WorkPriority,
    sequence: u64,
    key: K,
}

impl<K: Eq> PartialEq for HeapEntry<K> {
    fn eq(&self, other: &Self) -> bool {
        self.priority == other.priority && self.sequence == other.sequence
    }
}

impl<K: Eq> Eq for HeapEntry<K> {}

impl<K: Eq> PartialOrd for HeapEntry<K> {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl<K: Eq> Ord for HeapEntry<K> {
    fn cmp(&self, other: &Self) -> Ordering {
        self.priority
            .cmp(&other.priority)
            // Earlier sequence wins within the same priority.
            .then_with(|| other.sequence.cmp(&self.sequence))
    }
}

#[derive(Debug, Clone)]
struct Slot<T> {
    item: T,
    priority: WorkPriority,
    sequence: u64,
}

#[derive(Debug, Clone)]
pub struct WorkQueue<K, T> {
    max_len: usize,
    next_sequence: u64,
    heap: BinaryHeap<HeapEntry<K>>,
    slots: HashMap<K, Slot<T>>,
}

impl<K, T> WorkQueue<K, T>
where
    K: Clone + Eq + Hash,
{
    pub fn new(max_len: usize) -> Self {
        Self {
            max_len: max_len.max(1),
            next_sequence: 0,
            heap: BinaryHeap::new(),
            slots: HashMap::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn capacity(&self) -> usize {
        self.max_len
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }

    pub fn push(&mut self, key: K, priority: WorkPriority, item: T) -> QueuePushResult<T> {
        if let Some(slot) = self.slots.get_mut(&key) {
            let old = std::mem::replace(&mut slot.item, item);
            if priority > slot.priority {
                slot.priority = priority;
                self.next_sequence = self.next_sequence.wrapping_add(1);
                slot.sequence = self.next_sequence;
                self.heap.push(HeapEntry {
                    priority,
                    sequence: slot.sequence,
                    key,
                });
            }
            return QueuePushResult::Coalesced { old };
        }

        if self.slots.len() >= self.max_len && !self.evict_one_below(priority) {
            return QueuePushResult::Dropped { item };
        }

        self.next_sequence = self.next_sequence.wrapping_add(1);
        let sequence = self.next_sequence;
        self.slots.insert(
            key.clone(),
            Slot {
                item,
                priority,
                sequence,
            },
        );
        self.heap.push(HeapEntry {
            priority,
            sequence,
            key,
        });
        QueuePushResult::Inserted
    }

    pub fn pop(&mut self) -> Option<(K, T)> {
        while let Some(entry) = self.heap.pop() {
            let is_current = self
                .slots
                .get(&entry.key)
                .map(|slot| slot.priority == entry.priority && slot.sequence == entry.sequence)
                .unwrap_or(false);
            if !is_current {
                continue;
            }
            let slot = self.slots.remove(&entry.key)?;
            return Some((entry.key, slot.item));
        }
        None
    }

    fn evict_one_below(&mut self, incoming_priority: WorkPriority) -> bool {
        let victim_key = self
            .slots
            .iter()
            .filter(|(_, slot)| slot.priority < incoming_priority)
            .min_by_key(|(_, slot)| (slot.priority, slot.sequence))
            .map(|(key, _)| key.clone());

        if let Some(key) = victim_key {
            self.slots.remove(&key);
            true
        } else {
            false
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn pops_highest_priority_first_then_fifo() {
        let mut q = WorkQueue::new(8);
        q.push("a", WorkPriority::Normal, 1);
        q.push("b", WorkPriority::High, 2);
        q.push("c", WorkPriority::High, 3);
        q.push("d", WorkPriority::Low, 4);

        assert_eq!(q.pop(), Some(("b", 2)));
        assert_eq!(q.pop(), Some(("c", 3)));
        assert_eq!(q.pop(), Some(("a", 1)));
        assert_eq!(q.pop(), Some(("d", 4)));
        assert_eq!(q.pop(), None);
    }

    #[test]
    fn exposes_capacity() {
        let q: WorkQueue<&str, i32> = WorkQueue::new(7);
        assert_eq!(q.capacity(), 7);
    }

    #[test]
    fn coalesces_by_key_and_keeps_latest_item() {
        let mut q = WorkQueue::new(8);
        assert_eq!(
            q.push("save", WorkPriority::Low, 1),
            QueuePushResult::Inserted
        );
        assert_eq!(
            q.push("save", WorkPriority::High, 2),
            QueuePushResult::Coalesced { old: 1 }
        );
        assert_eq!(q.len(), 1);
        assert_eq!(q.pop(), Some(("save", 2)));
    }

    #[test]
    fn bounded_queue_drops_lower_or_rejects_when_full() {
        let mut q = WorkQueue::new(2);
        q.push("low", WorkPriority::Low, 1);
        q.push("normal", WorkPriority::Normal, 2);
        assert_eq!(
            q.push("low2", WorkPriority::Low, 3),
            QueuePushResult::Dropped { item: 3 }
        );
        assert_eq!(
            q.push("critical", WorkPriority::Critical, 4),
            QueuePushResult::Inserted
        );

        assert_eq!(q.pop(), Some(("critical", 4)));
        assert_eq!(q.pop(), Some(("normal", 2)));
        assert_eq!(q.pop(), None);
    }
}
