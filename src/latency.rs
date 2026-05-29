//! Lightweight latency attribution for interactive and backend hot paths.
//!
//! This module is intentionally dependency-free and low overhead. Recording a
//! sample is a small mutex-protected ring-buffer write. It is suitable for
//! coarse attribution such as render, streaming flush, session save, and backend
//! queue operations, not for per-token nanosecond profiling.

use std::collections::{HashMap, VecDeque};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

const DEFAULT_CAPACITY: usize = 1024;
const SLOW_SAMPLE_THRESHOLD: Duration = Duration::from_millis(50);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum LatencyKind {
    TuiRender,
    StreamAppend,
    StreamFlush,
    SessionSave,
    BackendQueuePush,
    BackendQueuePop,
}

impl LatencyKind {
    pub fn label(self) -> &'static str {
        match self {
            Self::TuiRender => "tui.render",
            Self::StreamAppend => "stream.append",
            Self::StreamFlush => "stream.flush",
            Self::SessionSave => "session.save",
            Self::BackendQueuePush => "backend.queue.push",
            Self::BackendQueuePop => "backend.queue.pop",
        }
    }
}

#[derive(Debug, Clone)]
pub struct LatencySample {
    pub kind: LatencyKind,
    pub label: Option<String>,
    pub duration: Duration,
    pub slow: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencySummary {
    pub kind: LatencyKind,
    pub count: usize,
    pub slow_count: usize,
    pub total: Duration,
    pub max: Duration,
}

impl LatencySummary {
    pub fn average(&self) -> Duration {
        if self.count == 0 {
            Duration::ZERO
        } else {
            Duration::from_nanos((self.total.as_nanos() / self.count as u128) as u64)
        }
    }
}

#[derive(Debug)]
pub struct LatencyRecorder {
    capacity: usize,
    samples: VecDeque<LatencySample>,
}

impl LatencyRecorder {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::with_capacity(capacity.max(1)),
        }
    }

    pub fn record(&mut self, sample: LatencySample) {
        if self.samples.len() == self.capacity {
            self.samples.pop_front();
        }
        self.samples.push_back(sample);
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }

    pub fn summaries(&self) -> Vec<LatencySummary> {
        let mut map: HashMap<LatencyKind, LatencySummary> = HashMap::new();
        for sample in &self.samples {
            let entry = map.entry(sample.kind).or_insert(LatencySummary {
                kind: sample.kind,
                count: 0,
                slow_count: 0,
                total: Duration::ZERO,
                max: Duration::ZERO,
            });
            entry.count += 1;
            entry.slow_count += usize::from(sample.slow);
            entry.total += sample.duration;
            entry.max = entry.max.max(sample.duration);
        }
        let mut summaries: Vec<_> = map.into_values().collect();
        summaries.sort_by_key(|summary| summary.kind);
        summaries
    }

    pub fn recent(&self, limit: usize) -> Vec<LatencySample> {
        self.samples.iter().rev().take(limit).cloned().collect()
    }
}

static GLOBAL_RECORDER: OnceLock<Mutex<LatencyRecorder>> = OnceLock::new();

fn global_recorder() -> &'static Mutex<LatencyRecorder> {
    GLOBAL_RECORDER.get_or_init(|| Mutex::new(LatencyRecorder::new(DEFAULT_CAPACITY)))
}

pub fn record(kind: LatencyKind, label: impl Into<Option<String>>, duration: Duration) {
    let slow = duration >= SLOW_SAMPLE_THRESHOLD;
    let sample = LatencySample {
        kind,
        label: label.into(),
        duration,
        slow,
    };
    if let Ok(mut recorder) = global_recorder().lock() {
        recorder.record(sample);
    }
}

pub fn summaries() -> Vec<LatencySummary> {
    global_recorder()
        .lock()
        .map(|recorder| recorder.summaries())
        .unwrap_or_default()
}

pub fn recent(limit: usize) -> Vec<LatencySample> {
    global_recorder()
        .lock()
        .map(|recorder| recorder.recent(limit))
        .unwrap_or_default()
}

pub fn format_summaries() -> String {
    let summaries = summaries();
    if summaries.is_empty() {
        return "No latency samples recorded.".to_string();
    }

    let mut lines = vec!["Latency summary:".to_string()];
    for summary in summaries {
        lines.push(format!(
            "- {}: count={} slow={} avg={}ms max={}ms",
            summary.kind.label(),
            summary.count,
            summary.slow_count,
            summary.average().as_millis(),
            summary.max.as_millis()
        ));
    }
    lines.join("\n")
}

pub fn clear() {
    if let Ok(mut recorder) = global_recorder().lock() {
        recorder.clear();
    }
}

pub struct LatencyTimer {
    kind: LatencyKind,
    label: Option<String>,
    start: Instant,
}

impl LatencyTimer {
    pub fn start(kind: LatencyKind) -> Self {
        Self {
            kind,
            label: None,
            start: Instant::now(),
        }
    }

    pub fn labeled(kind: LatencyKind, label: impl Into<String>) -> Self {
        Self {
            kind,
            label: Some(label.into()),
            start: Instant::now(),
        }
    }

    pub fn finish(self) -> Duration {
        let elapsed = self.start.elapsed();
        record(self.kind, self.label, elapsed);
        elapsed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn recorder_is_bounded_and_summarizes() {
        let mut recorder = LatencyRecorder::new(2);
        recorder.record(LatencySample {
            kind: LatencyKind::TuiRender,
            label: None,
            duration: Duration::from_millis(10),
            slow: false,
        });
        recorder.record(LatencySample {
            kind: LatencyKind::SessionSave,
            label: None,
            duration: Duration::from_millis(75),
            slow: true,
        });
        recorder.record(LatencySample {
            kind: LatencyKind::SessionSave,
            label: None,
            duration: Duration::from_millis(25),
            slow: false,
        });

        assert_eq!(recorder.len(), 2);
        let summaries = recorder.summaries();
        assert_eq!(summaries.len(), 1);
        assert_eq!(summaries[0].kind, LatencyKind::SessionSave);
        assert_eq!(summaries[0].count, 2);
        assert_eq!(summaries[0].slow_count, 1);
        assert_eq!(summaries[0].max, Duration::from_millis(75));
    }

    #[test]
    fn formats_empty_summary() {
        clear();
        assert_eq!(format_summaries(), "No latency samples recorded.");
    }
}
