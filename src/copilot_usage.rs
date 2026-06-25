//! Local Copilot usage tracking
//!
//! Tracks request counts and token usage locally since GitHub Copilot
//! doesn't expose a usage API. Data persists to ~/.neura/copilot_usage.json.

use chrono::{Datelike, Utc};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::Mutex;

static TRACKER: Mutex<Option<CopilotUsageTracker>> = Mutex::new(None);

fn usage_path() -> PathBuf {
    crate::storage::neura_dir()
        .unwrap_or_else(|_| PathBuf::from(".").join(".neura"))
        .join("copilot_usage.json")
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct CopilotUsageTracker {
    pub today: DayUsage,
    pub month: MonthUsage,
    pub all_time: AllTimeUsage,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct DayUsage {
    pub date: String,
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct MonthUsage {
    pub month: String,
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default)]
pub struct AllTimeUsage {
    pub requests: u64,
    pub premium_requests: u64,
    pub input_tokens: u64,
    pub output_tokens: u64,
}

impl CopilotUsageTracker {
    fn load() -> Self {
        let path = usage_path();
        crate::storage::read_json(&path).unwrap_or_default()
    }

    fn save(&self) {
        let path = usage_path();
        let _ = crate::storage::write_json(&path, self);
    }

    fn roll_if_needed(&mut self) {
        let now = Utc::now();
        let today = now.format("%Y-%m-%d").to_string();
        let month = format!("{}-{:02}", now.year(), now.month());

        if self.today.date != today {
            self.today = DayUsage {
                date: today,
                ..Default::default()
            };
        }
        if self.month.month != month {
            self.month = MonthUsage {
                month,
                ..Default::default()
            };
        }
    }

    fn record(&mut self, input_tokens: u64, output_tokens: u64, is_premium: bool) {
        self.roll_if_needed();

        self.today.requests += 1;
        self.today.input_tokens += input_tokens;
        self.today.output_tokens += output_tokens;
        if is_premium {
            self.today.premium_requests += 1;
        }

        self.month.requests += 1;
        self.month.input_tokens += input_tokens;
        self.month.output_tokens += output_tokens;
        if is_premium {
            self.month.premium_requests += 1;
        }

        self.all_time.requests += 1;
        self.all_time.input_tokens += input_tokens;
        self.all_time.output_tokens += output_tokens;
        if is_premium {
            self.all_time.premium_requests += 1;
        }

        self.save();
    }
}

/// Record a completed Copilot request.
pub fn record_request(input_tokens: u64, output_tokens: u64, is_premium: bool) {
    let mut guard = match TRACKER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let tracker = guard.get_or_insert_with(CopilotUsageTracker::load);
    tracker.record(input_tokens, output_tokens, is_premium);
}

/// Get current usage snapshot.
pub fn get_usage() -> CopilotUsageTracker {
    let mut guard = match TRACKER.lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    let tracker = guard.get_or_insert_with(CopilotUsageTracker::load);
    tracker.roll_if_needed();
    tracker.clone()
}

#[cfg(test)]
mod tests {
    use super::{AllTimeUsage, CopilotUsageTracker, DayUsage, MonthUsage, TRACKER, usage_path};
    use std::ffi::OsString;
    use std::sync::{Mutex, OnceLock};

    static ENV_LOCK: OnceLock<Mutex<()>> = OnceLock::new();

    fn lock_env() -> std::sync::MutexGuard<'static, ()> {
        let mutex = ENV_LOCK.get_or_init(|| Mutex::new(()));
        match mutex.lock() {
            Ok(guard) => guard,
            Err(poisoned) => poisoned.into_inner(),
        }
    }

    struct EnvVarGuard {
        key: &'static str,
        prev: Option<OsString>,
    }

    impl EnvVarGuard {
        fn set(key: &'static str, value: impl AsRef<std::ffi::OsStr>) -> Self {
            let prev = std::env::var_os(key);
            crate::env::set_var(key, value);
            Self { key, prev }
        }
    }

    impl Drop for EnvVarGuard {
        fn drop(&mut self) {
            if let Some(prev) = &self.prev {
                crate::env::set_var(self.key, prev);
            } else {
                crate::env::remove_var(self.key);
            }
        }
    }

    fn clear_tracker() {
        if let Ok(mut tracker) = TRACKER.lock() {
            *tracker = None;
        }
    }

    #[test]
    fn usage_path_respects_neura_home() {
        let _env_lock = lock_env();
        clear_tracker();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("NEURA_HOME", temp.path().as_os_str());

        assert_eq!(usage_path(), temp.path().join("copilot_usage.json"));
    }

    #[test]
    fn save_and_load_roundtrip_under_neura_home() {
        let _env_lock = lock_env();
        clear_tracker();
        let temp = tempfile::tempdir().expect("tempdir");
        let _home = EnvVarGuard::set("NEURA_HOME", temp.path().as_os_str());

        let tracker = CopilotUsageTracker {
            today: DayUsage {
                date: "2026-03-06".to_string(),
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
            month: MonthUsage {
                month: "2026-03".to_string(),
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
            all_time: AllTimeUsage {
                requests: 2,
                premium_requests: 1,
                input_tokens: 100,
                output_tokens: 50,
            },
        };

        tracker.save();
        let loaded = CopilotUsageTracker::load();

        assert_eq!(loaded.today.date, "2026-03-06");
        assert_eq!(loaded.today.requests, 2);
        assert_eq!(loaded.all_time.output_tokens, 50);
    }
}
