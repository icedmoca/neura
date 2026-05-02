use super::*;
use crate::message::{ContentBlock, Role};
use crate::session::Session;
use serde_json::json;
use std::time::{SystemTime, UNIX_EPOCH};

fn with_temp_home<T>(f: impl FnOnce(&std::path::Path) -> T) -> T {
    let _guard = crate::storage::lock_test_env();
    let temp = tempfile::TempDir::new().expect("create temp dir");
    let previous_home = std::env::var("KCODE_HOME").ok();
    crate::env::set_var("KCODE_HOME", temp.path());
    std::fs::create_dir_all(temp.path().join("sessions")).expect("create sessions dir");

    let result = f(temp.path());

    if let Some(previous_home) = previous_home {
        crate::env::set_var("KCODE_HOME", previous_home);
    } else {
        crate::env::remove_var("KCODE_HOME");
    }

    result
}

fn save_test_session(messages: Vec<(Role, Vec<ContentBlock>)>) {
    let nonce = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_nanos();
    let mut session = Session::create_with_id(format!("test-session-{nonce}"), None, None);
    session.short_name = Some("search-test".to_string());
    session.working_dir = Some("/tmp/project".to_string());
    for (role, content) in messages {
        session.add_message(role, content);
    }
    session.save().expect("save test session");
}

#[test]
fn token_overlap_matches_when_exact_phrase_is_absent() {
    with_temp_home(|home| {
        save_test_session(vec![(
            Role::Assistant,
            vec![ContentBlock::Text {
                text: "Try reconnecting your AirPods after the Bluetooth audio drops.".to_string(),
                cache_control: None,
            }],
        )]);

        let query = QueryProfile::new("airpods reconnect bluetooth");
        let results =
            search_sessions_blocking(&home.join("sessions"), &query, None, 10, "test-session")
                .expect("search succeeds");

        assert!(!results.is_empty(), "expected token-overlap match");
        assert!(results[0].snippet.to_lowercase().contains("airpods"));
    });
}

#[test]
fn tool_use_input_is_searchable() {
    with_temp_home(|home| {
        save_test_session(vec![(
            Role::Assistant,
            vec![ContentBlock::ToolUse {
                id: "tool-1".to_string(),
                name: "websearch".to_string(),
                input: json!({
                    "query": "best time post hackernews visibility upvotes"
                }),
            }],
        )]);

        let query = QueryProfile::new("hackernews visibility upvotes");
        let results =
            search_sessions_blocking(&home.join("sessions"), &query, None, 10, "test-session")
                .expect("search succeeds");

        assert!(!results.is_empty(), "expected tool input match");
        assert!(results[0].snippet.to_lowercase().contains("hackernews"));
    });
}
