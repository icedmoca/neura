use super::super::commands_review::{ImproveCommand, RefactorCommand};
use super::{parse_improve_command, parse_manual_subagent_spec, parse_refactor_command};

#[test]
fn parse_improve_command_accepts_builtin_forms() {
    assert_eq!(
        parse_improve_command("/improve")
            .expect("/improve should be recognized")
            .expect("/improve should parse"),
        ImproveCommand::Run {
            plan_only: false,
            focus: None,
        }
    );
    assert_eq!(
        parse_improve_command("/improve plan ui rendering")
            .expect("/improve plan should be recognized")
            .expect("/improve plan should parse"),
        ImproveCommand::Run {
            plan_only: true,
            focus: Some("ui rendering".to_string()),
        }
    );
    assert_eq!(
        parse_improve_command("/improve resume")
            .expect("/improve resume should be recognized")
            .expect("/improve resume should parse"),
        ImproveCommand::Resume
    );
    assert_eq!(
        parse_improve_command("/improve status")
            .expect("/improve status should be recognized")
            .expect("/improve status should parse"),
        ImproveCommand::Status
    );
    assert_eq!(
        parse_improve_command("/improve stop")
            .expect("/improve stop should be recognized")
            .expect("/improve stop should parse"),
        ImproveCommand::Stop
    );
}

#[test]
fn parse_improve_command_rejects_invalid_builtin_forms_without_falling_through() {
    assert!(parse_improve_command("/improve --bad").is_some());
    assert_eq!(
        parse_improve_command("/improve plan")
            .expect("/improve plan should be recognized")
            .expect("/improve plan without focus should parse"),
        ImproveCommand::Run {
            plan_only: true,
            focus: None,
        }
    );
    assert!(parse_improve_command("/improved").is_none());
}

#[test]
fn parse_refactor_command_accepts_builtin_forms() {
    assert_eq!(
        parse_refactor_command("/refactor")
            .expect("/refactor should be recognized")
            .expect("/refactor should parse"),
        RefactorCommand::Run {
            plan_only: false,
            focus: None,
        }
    );
    assert_eq!(
        parse_refactor_command("/refactor plan command wiring")
            .expect("/refactor plan should be recognized")
            .expect("/refactor plan should parse"),
        RefactorCommand::Run {
            plan_only: true,
            focus: Some("command wiring".to_string()),
        }
    );
    assert!(parse_refactor_command("/refactored").is_none());
}

#[test]
fn parse_manual_subagent_spec_accepts_flags_and_prompt() {
    let spec = parse_manual_subagent_spec(
        "--type research --model gpt-5.4 --continue session_123 investigate this bug",
    )
    .expect("parse manual subagent spec");

    assert_eq!(spec.subagent_type, "research");
    assert_eq!(spec.model.as_deref(), Some("gpt-5.4"));
    assert_eq!(spec.session_id.as_deref(), Some("session_123"));
    assert_eq!(spec.prompt, "investigate this bug");
}

#[test]
fn parse_manual_subagent_spec_rejects_missing_prompt() {
    let err = parse_manual_subagent_spec("--model gpt-5.4")
        .expect_err("missing prompt should be rejected");
    assert!(err.contains("Missing prompt"));
}

#[test]
fn parse_voice_command_accepts_exact_command_only() {
    assert!(matches!(parse_voice_command("/voice"), Some(Ok(()))));
    assert!(parse_voice_command("voice").is_none());
    assert!(parse_voice_command("/voicemail").is_none());
    assert!(matches!(parse_voice_command("/voice now"), Some(Err(_))));
}
