use crate::{
    session_launch::{DesktopSessionEvent, DesktopSessionHandle},
    workspace,
};
use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Parser, Tag, TagEnd};
use workspace::{KeyInput, KeyOutcome};

pub(crate) const SINGLE_SESSION_FONT_FAMILY: &str = "JetBrainsMono Nerd Font";
pub(crate) const SINGLE_SESSION_FONT_WEIGHT: &str = "Light";
pub(crate) const SINGLE_SESSION_FONT_FALLBACKS: &[&str] = &[
    "JetBrainsMono Nerd Font Mono",
    "JetBrains Mono",
    "monospace",
];
pub(crate) const SINGLE_SESSION_TITLE_FONT_SIZE: f32 = 28.0;
pub(crate) const SINGLE_SESSION_BODY_FONT_SIZE: f32 = 22.0;
pub(crate) const SINGLE_SESSION_META_FONT_SIZE: f32 = 16.0;
pub(crate) const SINGLE_SESSION_CODE_FONT_SIZE: f32 = 21.0;
pub(crate) const SINGLE_SESSION_BODY_LINE_HEIGHT: f32 = 1.45;
pub(crate) const SINGLE_SESSION_CODE_LINE_HEIGHT: f32 = 1.35;
pub(crate) const SINGLE_SESSION_META_LINE_HEIGHT: f32 = 1.25;

#[allow(dead_code)]
#[derive(Clone, Copy, Debug)]
pub(crate) struct SingleSessionTypography {
    pub(crate) family: &'static str,
    pub(crate) weight: &'static str,
    pub(crate) fallbacks: &'static [&'static str],
    pub(crate) title_size: f32,
    pub(crate) body_size: f32,
    pub(crate) meta_size: f32,
    pub(crate) code_size: f32,
    pub(crate) body_line_height: f32,
    pub(crate) code_line_height: f32,
    pub(crate) meta_line_height: f32,
}

pub(crate) const fn single_session_typography() -> SingleSessionTypography {
    SingleSessionTypography {
        family: SINGLE_SESSION_FONT_FAMILY,
        weight: SINGLE_SESSION_FONT_WEIGHT,
        fallbacks: SINGLE_SESSION_FONT_FALLBACKS,
        title_size: SINGLE_SESSION_TITLE_FONT_SIZE,
        body_size: SINGLE_SESSION_BODY_FONT_SIZE,
        meta_size: SINGLE_SESSION_META_FONT_SIZE,
        code_size: SINGLE_SESSION_CODE_FONT_SIZE,
        body_line_height: SINGLE_SESSION_BODY_LINE_HEIGHT,
        code_line_height: SINGLE_SESSION_CODE_LINE_HEIGHT,
        meta_line_height: SINGLE_SESSION_META_LINE_HEIGHT,
    }
}

#[derive(Clone, Debug)]
pub(crate) struct SingleSessionApp {
    pub(crate) session: Option<workspace::SessionCard>,
    pub(crate) draft: String,
    pub(crate) draft_cursor: usize,
    pub(crate) detail_scroll: usize,
    pub(crate) live_session_id: Option<String>,
    pub(crate) messages: Vec<SingleSessionMessage>,
    pub(crate) streaming_response: String,
    pub(crate) status: Option<String>,
    pub(crate) error: Option<String>,
    pub(crate) is_processing: bool,
    pub(crate) body_scroll_lines: usize,
    input_undo_stack: Vec<(String, usize)>,
    session_handle: Option<DesktopSessionHandle>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub(crate) struct SingleSessionMessage {
    role: SingleSessionRole,
    content: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
#[allow(dead_code)]
pub(crate) enum SingleSessionRole {
    User,
    Assistant,
    Tool,
    System,
    Meta,
}

impl SingleSessionRole {
    pub(crate) fn is_user(self) -> bool {
        matches!(self, Self::User)
    }
}

impl SingleSessionMessage {
    pub(crate) fn user(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::User,
            content: content.into(),
        }
    }

    pub(crate) fn assistant(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Assistant,
            content: content.into(),
        }
    }

    pub(crate) fn tool(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Tool,
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn system(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::System,
            content: content.into(),
        }
    }

    #[allow(dead_code)]
    pub(crate) fn meta(content: impl Into<String>) -> Self {
        Self {
            role: SingleSessionRole::Meta,
            content: content.into(),
        }
    }
}

impl SingleSessionApp {
    pub(crate) fn new(session: Option<workspace::SessionCard>) -> Self {
        Self {
            session,
            draft: String::new(),
            draft_cursor: 0,
            detail_scroll: 0,
            live_session_id: None,
            messages: Vec::new(),
            streaming_response: String::new(),
            status: None,
            error: None,
            is_processing: false,
            body_scroll_lines: 0,
            input_undo_stack: Vec::new(),
            session_handle: None,
        }
    }

    pub(crate) fn replace_session(&mut self, session: Option<workspace::SessionCard>) {
        self.session = session;
        if let Some(session) = &self.session {
            self.live_session_id = Some(session.session_id.clone());
        }
        self.detail_scroll = 0;
    }

    pub(crate) fn reset_fresh_session(&mut self) {
        self.session = None;
        self.draft.clear();
        self.draft_cursor = 0;
        self.detail_scroll = 0;
        self.live_session_id = None;
        self.messages.clear();
        self.streaming_response.clear();
        self.status = None;
        self.error = None;
        self.is_processing = false;
        self.body_scroll_lines = 0;
        self.input_undo_stack.clear();
        self.session_handle = None;
    }

    pub(crate) fn status_title(&self) -> String {
        let title = self.title();
        format!(
            "Neura Desktop · single session · {title} · Ctrl+Enter send · Enter newline · Ctrl+; spawn · Ctrl+R refresh · Esc quit · --workspace for Niri layout"
        )
    }

    pub(crate) fn title(&self) -> String {
        if let Some(session) = &self.session {
            session.title.clone()
        } else if let Some(session_id) = &self.live_session_id {
            format!("session {}", short_session_id(session_id))
        } else {
            "fresh session".to_string()
        }
    }

    pub(crate) fn has_background_work(&self) -> bool {
        self.is_processing
    }

    pub(crate) fn user_turn_count(&self) -> usize {
        self.messages
            .iter()
            .filter(|message| message.role.is_user())
            .count()
    }

    pub(crate) fn next_prompt_number(&self) -> usize {
        self.user_turn_count() + 1
    }

    pub(crate) fn composer_prompt(&self) -> String {
        format!("{}› ", self.next_prompt_number())
    }

    pub(crate) fn composer_text(&self) -> String {
        format!("{}{}", self.composer_prompt(), self.draft)
    }

    pub(crate) fn composer_status_line(&self) -> String {
        let status = self.status.as_deref().unwrap_or("ready");
        let mode = if self.is_processing {
            "Ctrl+C interrupt"
        } else {
            "Ctrl+Enter send · Enter newline"
        };
        format!("{status} · {mode}")
    }

    pub(crate) fn handle_key(&mut self, key: KeyInput) -> KeyOutcome {
        match key {
            KeyInput::SpawnPanel => KeyOutcome::SpawnSession,
            KeyInput::RefreshSessions => KeyOutcome::Redraw,
            KeyInput::CancelGeneration => {
                if self.is_processing {
                    KeyOutcome::CancelGeneration
                } else {
                    KeyOutcome::None
                }
            }
            KeyInput::ScrollBodyPages(pages) => {
                self.scroll_body_lines(pages * 12);
                KeyOutcome::Redraw
            }
            KeyInput::SubmitDraft => self.submit_draft(),
            KeyInput::Escape => KeyOutcome::Exit,
            KeyInput::Enter => {
                self.insert_draft_text("\n");
                KeyOutcome::Redraw
            }
            KeyInput::Backspace => {
                self.delete_previous_char();
                KeyOutcome::Redraw
            }
            KeyInput::DeletePreviousWord => {
                self.delete_previous_word();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextWord => {
                self.delete_next_word();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteNextChar => {
                self.delete_next_char();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordLeft => {
                self.move_cursor_word_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorWordRight => {
                self.move_cursor_word_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorLeft => {
                self.move_cursor_left();
                KeyOutcome::Redraw
            }
            KeyInput::MoveCursorRight => {
                self.move_cursor_right();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineStart => {
                self.move_to_line_start();
                KeyOutcome::Redraw
            }
            KeyInput::MoveToLineEnd => {
                self.move_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineStart => {
                self.delete_to_line_start();
                KeyOutcome::Redraw
            }
            KeyInput::DeleteToLineEnd => {
                self.delete_to_line_end();
                KeyOutcome::Redraw
            }
            KeyInput::UndoInput => {
                self.undo_input_change();
                KeyOutcome::Redraw
            }
            KeyInput::Character(text) => {
                self.insert_draft_text(&text);
                KeyOutcome::Redraw
            }
            _ => KeyOutcome::None,
        }
    }

    pub(crate) fn body_lines(&self) -> Vec<String> {
        if !self.messages.is_empty() || !self.streaming_response.is_empty() || self.error.is_some()
        {
            let mut lines = Vec::new();
            let mut user_turn = 1;
            for message in &self.messages {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                append_chat_message_lines(&mut lines, message, &mut user_turn);
            }
            if !self.streaming_response.is_empty() {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                append_assistant_lines(&mut lines, self.streaming_response.trim_end());
            }
            if let Some(error) = &self.error {
                if !lines.is_empty() {
                    lines.push(String::new());
                }
                lines.push(format!("error: {error}"));
            }
            return lines;
        }

        if let Some(status) = &self.status {
            return vec![status.clone()];
        }

        single_session_lines(self.session.as_ref())
    }

    pub(crate) fn apply_session_event(&mut self, event: DesktopSessionEvent) {
        match event {
            DesktopSessionEvent::Status(status) => self.status = Some(status),
            DesktopSessionEvent::Reloading { .. } => {
                self.status = Some("server reloading, reconnecting".to_string());
                self.is_processing = true;
            }
            DesktopSessionEvent::SessionStarted { session_id } => {
                self.live_session_id = Some(session_id);
                self.status = Some("connected".to_string());
            }
            DesktopSessionEvent::TextDelta(text) => {
                self.streaming_response.push_str(&text);
                self.status = Some("receiving".to_string());
            }
            DesktopSessionEvent::TextReplace(text) => {
                self.streaming_response = text;
                self.status = Some("receiving".to_string());
            }
            DesktopSessionEvent::ToolStarted { name } => {
                self.status = Some(format!("using tool {name}"));
                self.messages
                    .push(SingleSessionMessage::tool(format!("{name} running")));
            }
            DesktopSessionEvent::ToolFinished {
                name,
                summary,
                is_error,
            } => {
                self.status = Some(if is_error {
                    format!("tool {name} failed")
                } else {
                    format!("tool {name} done")
                });
                let marker = if is_error { "failed" } else { "done" };
                self.messages.push(SingleSessionMessage::tool(format!(
                    "{name} {marker}: {summary}"
                )));
            }
            DesktopSessionEvent::Done => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.session_handle = None;
                self.status = Some("ready".to_string());
            }
            DesktopSessionEvent::Error(error) => {
                self.finish_streaming_response();
                self.is_processing = false;
                self.session_handle = None;
                self.status = Some("error".to_string());
                self.error = Some(error);
            }
        }
    }

    pub(crate) fn set_session_handle(&mut self, handle: DesktopSessionHandle) {
        self.session_handle = Some(handle);
    }

    pub(crate) fn cancel_generation(&mut self) -> bool {
        let Some(handle) = &self.session_handle else {
            return false;
        };
        match handle.cancel() {
            Ok(()) => {
                self.status = Some("cancelling".to_string());
                true
            }
            Err(error) => {
                self.error = Some(format!("{error:#}"));
                self.is_processing = false;
                self.session_handle = None;
                true
            }
        }
    }

    pub(crate) fn scroll_body_lines(&mut self, lines: i32) {
        if lines > 0 {
            self.body_scroll_lines = self.body_scroll_lines.saturating_add(lines as usize);
        } else {
            self.body_scroll_lines = self
                .body_scroll_lines
                .saturating_sub(lines.unsigned_abs() as usize);
        }
    }

    pub(crate) fn scroll_body_to_bottom(&mut self) {
        self.body_scroll_lines = 0;
    }

    pub(crate) fn draft_cursor_line_col(&self) -> (usize, usize) {
        let before_cursor = &self.draft[..self.draft_cursor.min(self.draft.len())];
        let line = before_cursor.chars().filter(|ch| *ch == '\n').count();
        let column = before_cursor
            .rsplit('\n')
            .next()
            .unwrap_or_default()
            .chars()
            .count();
        (line, column)
    }

    pub(crate) fn draft_cursor_line_byte_index(&self) -> (usize, usize) {
        let cursor = self.draft_cursor.min(self.draft.len());
        let line = self.draft[..cursor]
            .chars()
            .filter(|ch| *ch == '\n')
            .count();
        let line_start = line_start(&self.draft, cursor);
        (line, cursor - line_start)
    }

    pub(crate) fn composer_cursor_line_byte_index(&self) -> (usize, usize) {
        let (line, index) = self.draft_cursor_line_byte_index();
        if line == 0 {
            (line, self.composer_prompt().len() + index)
        } else {
            (line, index)
        }
    }

    fn submit_draft(&mut self) -> KeyOutcome {
        let message = self.draft.trim().to_string();
        if message.is_empty() {
            return KeyOutcome::None;
        }
        self.record_user_submit(&message);
        let Some(session) = &self.session else {
            return KeyOutcome::StartFreshSession { message };
        };
        let session_id = session.session_id.clone();
        let title = session.title.clone();
        KeyOutcome::SendDraft {
            session_id,
            title,
            message,
        }
    }

    fn record_user_submit(&mut self, message: &str) {
        self.messages.push(SingleSessionMessage::user(message));
        self.draft.clear();
        self.draft_cursor = 0;
        self.input_undo_stack.clear();
        self.streaming_response.clear();
        self.scroll_body_to_bottom();
        self.status = Some("sending".to_string());
        self.error = None;
        self.is_processing = true;
    }

    fn finish_streaming_response(&mut self) {
        let response = self.streaming_response.trim().to_string();
        if !response.is_empty() {
            self.messages
                .push(SingleSessionMessage::assistant(response));
        }
        self.streaming_response.clear();
    }

    fn insert_draft_text(&mut self, text: &str) {
        if !text.is_empty() {
            self.remember_input_undo_state();
        }
        self.clamp_draft_cursor();
        self.draft.insert_str(self.draft_cursor, text);
        self.draft_cursor += text.len();
    }

    fn delete_previous_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor == 0 {
            return;
        }
        self.remember_input_undo_state();
        let previous = previous_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(previous..self.draft_cursor, "");
        self.draft_cursor = previous;
    }

    fn delete_next_char(&mut self) {
        self.clamp_draft_cursor();
        if self.draft_cursor >= self.draft.len() {
            return;
        }
        self.remember_input_undo_state();
        let next = next_char_boundary(&self.draft, self.draft_cursor);
        self.draft.replace_range(self.draft_cursor..next, "");
    }

    fn delete_previous_word(&mut self) {
        self.clamp_draft_cursor();
        let start = previous_word_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_next_word(&mut self) {
        self.clamp_draft_cursor();
        let end = next_word_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn move_cursor_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_char_boundary(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_left(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = previous_word_start(&self.draft, self.draft_cursor);
    }

    fn move_cursor_word_right(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = next_word_end(&self.draft, self.draft_cursor);
    }

    fn move_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_start(&self.draft, self.draft_cursor);
    }

    fn move_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        self.draft_cursor = line_end(&self.draft, self.draft_cursor);
    }

    fn delete_to_line_start(&mut self) {
        self.clamp_draft_cursor();
        let start = line_start(&self.draft, self.draft_cursor);
        if start < self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(start..self.draft_cursor, "");
        self.draft_cursor = start;
    }

    fn delete_to_line_end(&mut self) {
        self.clamp_draft_cursor();
        let end = line_end(&self.draft, self.draft_cursor);
        if end > self.draft_cursor {
            self.remember_input_undo_state();
        }
        self.draft.replace_range(self.draft_cursor..end, "");
    }

    fn remember_input_undo_state(&mut self) {
        if self
            .input_undo_stack
            .last()
            .is_some_and(|(draft, cursor)| draft == &self.draft && *cursor == self.draft_cursor)
        {
            return;
        }
        self.input_undo_stack
            .push((self.draft.clone(), self.draft_cursor));
        const MAX_UNDO: usize = 64;
        if self.input_undo_stack.len() > MAX_UNDO {
            self.input_undo_stack.remove(0);
        }
    }

    fn undo_input_change(&mut self) {
        if let Some((draft, cursor)) = self.input_undo_stack.pop() {
            self.draft = draft;
            self.draft_cursor = cursor.min(self.draft.len());
            self.clamp_draft_cursor();
        }
    }

    fn clamp_draft_cursor(&mut self) {
        self.draft_cursor = self.draft_cursor.min(self.draft.len());
        while !self.draft.is_char_boundary(self.draft_cursor) {
            self.draft_cursor -= 1;
        }
    }
}

fn append_chat_message_lines(
    lines: &mut Vec<String>,
    message: &SingleSessionMessage,
    user_turn: &mut usize,
) {
    match message.role {
        SingleSessionRole::User => {
            append_user_lines(lines, *user_turn, message.content.trim());
            *user_turn += 1;
        }
        SingleSessionRole::Assistant => append_assistant_lines(lines, message.content.trim()),
        SingleSessionRole::Tool => append_tool_lines(lines, message.content.trim()),
        SingleSessionRole::System | SingleSessionRole::Meta => {
            append_meta_lines(lines, message.content.trim())
        }
    }
}

fn append_user_lines(lines: &mut Vec<String>, turn: usize, content: &str) {
    let mut content_lines = content.lines();
    let Some(first) = content_lines.next() else {
        return;
    };
    lines.push(format!("{turn}  {first}"));
    for line in content_lines {
        lines.push(format!("   {line}"));
    }
}

fn append_assistant_lines(lines: &mut Vec<String>, content: &str) {
    lines.extend(render_assistant_markdown_lines(content));
}

fn render_assistant_markdown_lines(content: &str) -> Vec<String> {
    let mut lines = Vec::new();
    let mut current = String::new();
    let mut list_stack = Vec::<Option<u64>>::new();
    let mut in_code_block = false;

    for event in Parser::new(content) {
        match event {
            Event::Start(Tag::Heading { level, .. }) => {
                flush_current_line(&mut lines, &mut current);
                current.push_str(heading_prefix(level));
            }
            Event::End(TagEnd::Heading(_)) => flush_current_line(&mut lines, &mut current),
            Event::Start(Tag::Paragraph) => {}
            Event::End(TagEnd::Paragraph) => flush_current_line(&mut lines, &mut current),
            Event::Start(Tag::List(start)) => list_stack.push(start),
            Event::End(TagEnd::List(_)) => {
                list_stack.pop();
                flush_current_line(&mut lines, &mut current);
            }
            Event::Start(Tag::Item) => {
                flush_current_line(&mut lines, &mut current);
                if let Some(Some(next)) = list_stack.last_mut() {
                    current.push_str(&format!("{next}. "));
                    *next += 1;
                } else {
                    current.push_str("• ");
                }
            }
            Event::End(TagEnd::Item) => flush_current_line(&mut lines, &mut current),
            Event::Start(Tag::CodeBlock(kind)) => {
                flush_current_line(&mut lines, &mut current);
                let lang = match kind {
                    CodeBlockKind::Fenced(lang) if !lang.is_empty() => format!(" {lang}"),
                    _ => String::new(),
                };
                lines.push(format!("```{lang}"));
                in_code_block = true;
            }
            Event::End(TagEnd::CodeBlock) => {
                flush_current_line(&mut lines, &mut current);
                lines.push("```".to_string());
                in_code_block = false;
            }
            Event::Text(text) => {
                if in_code_block {
                    for line in text.lines() {
                        lines.push(format!("    {line}"));
                    }
                } else {
                    current.push_str(&text);
                }
            }
            Event::Code(code) => {
                current.push('`');
                current.push_str(&code);
                current.push('`');
            }
            Event::SoftBreak | Event::HardBreak => flush_current_line(&mut lines, &mut current),
            Event::Rule => {
                flush_current_line(&mut lines, &mut current);
                lines.push("───".to_string());
            }
            _ => {}
        }
    }

    flush_current_line(&mut lines, &mut current);
    if lines.is_empty() && !content.trim().is_empty() {
        lines.extend(content.lines().map(ToOwned::to_owned));
    }
    lines
}

fn flush_current_line(lines: &mut Vec<String>, current: &mut String) {
    let trimmed = current.trim_end();
    if !trimmed.is_empty() {
        lines.push(trimmed.to_string());
    }
    current.clear();
}

fn heading_prefix(level: HeadingLevel) -> &'static str {
    match level {
        HeadingLevel::H1 => "# ",
        HeadingLevel::H2 => "## ",
        HeadingLevel::H3 => "### ",
        _ => "#### ",
    }
}

fn append_tool_lines(lines: &mut Vec<String>, content: &str) {
    if content.is_empty() {
        return;
    }
    lines.push(format!("• {content}"));
}

fn append_meta_lines(lines: &mut Vec<String>, content: &str) {
    if content.is_empty() {
        return;
    }
    lines.push(format!("  {content}"));
}

fn previous_char_boundary(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .char_indices()
        .last()
        .map(|(index, _)| index)
        .unwrap_or(0)
}

fn next_char_boundary(text: &str, cursor: usize) -> usize {
    if cursor >= text.len() {
        return text.len();
    }
    text[cursor..]
        .char_indices()
        .nth(1)
        .map(|(offset, _)| cursor + offset)
        .unwrap_or(text.len())
}

fn previous_word_start(text: &str, cursor: usize) -> usize {
    let mut start = cursor.min(text.len());
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    while start > 0 {
        let previous = previous_char_boundary(text, start);
        let ch = text[previous..start].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        start = previous;
    }
    start
}

fn next_word_end(text: &str, cursor: usize) -> usize {
    let mut end = cursor.min(text.len());
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if !ch.is_whitespace() {
            break;
        }
        end = next;
    }
    while end < text.len() {
        let next = next_char_boundary(text, end);
        let ch = text[end..next].chars().next().unwrap_or_default();
        if ch.is_whitespace() {
            break;
        }
        end = next;
    }
    end
}

fn line_start(text: &str, cursor: usize) -> usize {
    text[..cursor.min(text.len())]
        .rfind('\n')
        .map(|index| index + 1)
        .unwrap_or(0)
}

fn line_end(text: &str, cursor: usize) -> usize {
    text[cursor.min(text.len())..]
        .find('\n')
        .map(|offset| cursor + offset)
        .unwrap_or(text.len())
}

fn short_session_id(session_id: &str) -> &str {
    session_id
        .strip_prefix("session_")
        .and_then(|rest| rest.split('_').next())
        .filter(|name| !name.is_empty())
        .unwrap_or(session_id)
}

pub(crate) fn single_session_surface(
    session: Option<&workspace::SessionCard>,
) -> workspace::Surface {
    let lines = single_session_lines(session);
    workspace::Surface {
        id: 1,
        title: session
            .map(|session| session.title.clone())
            .unwrap_or_else(|| "new neura session".to_string()),
        body_lines: lines.clone(),
        detail_lines: lines,
        session_id: session.map(|session| session.session_id.clone()),
        lane: 0,
        column: 0,
        color_index: 0,
    }
}

pub(crate) fn single_session_lines(session: Option<&workspace::SessionCard>) -> Vec<String> {
    let Some(session) = session else {
        return vec![
            "single session mode".to_string(),
            "fresh desktop-native session draft".to_string(),
            "type here without nav or insert modes".to_string(),
            "ctrl+enter will send once desktop-native execution is connected".to_string(),
            "ctrl+; clears this draft and starts another fresh desktop session".to_string(),
            "run with --workspace for the niri layout wrapper".to_string(),
        ];
    };

    let mut lines = vec![
        "single session mode".to_string(),
        session.subtitle.clone(),
        session.detail.clone(),
    ];
    if !session.preview_lines.is_empty() {
        lines.push("recent transcript".to_string());
        lines.extend(session.preview_lines.clone());
    }
    if !session.detail_lines.is_empty() {
        lines.push("expanded transcript".to_string());
        lines.extend(session.detail_lines.clone());
    }
    lines
}
