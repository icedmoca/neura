use super::text::{truncate_chars, truncate_smart};
use super::{AuthMethod, InfoWidgetData};
use super::{memory_render, usage_render};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;

pub(super) fn render_model_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(model) = &data.model else {
        return Vec::new();
    };

    let mut lines: Vec<Line> = Vec::new();

    let short_name = shorten_model_name(model);
    let max_len = inner.width.saturating_sub(2) as usize;

    let mut spans = vec![
        Span::styled("⚡ ", Style::default().fg(rgb(140, 180, 255))),
        Span::styled(
            truncate_smart(&short_name, max_len.saturating_sub(2)),
            Style::default().fg(rgb(180, 180, 190)).bold(),
        ),
    ];

    append_model_runtime_metadata(&mut spans, data);

    lines.push(Line::from(spans));

    if data.session_count.is_some() || data.session_name.is_some() {
        let mut parts = Vec::new();

        if let Some(sessions) = data.session_count {
            parts.push(format!(
                "{} session{}",
                sessions,
                if sessions == 1 { "" } else { "s" }
            ));
        }

        if let Some(name) = data.session_name.as_deref()
            && !name.trim().is_empty()
        {
            parts.push(name.to_string());
        }

        if !parts.is_empty() {
            let detail = truncate_smart(&parts.join(" · "), max_len.saturating_sub(2));
            lines.push(Line::from(vec![Span::styled(
                detail,
                Style::default().fg(rgb(140, 140, 150)),
            )]));
        }
    }

    if let Some(provider) = data
        .provider_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        let mut provider_spans = vec![
            Span::styled("☁ ", Style::default().fg(rgb(140, 180, 255))),
            Span::styled(
                provider.to_lowercase(),
                Style::default().fg(rgb(140, 180, 255)),
            ),
        ];
        if let Some(upstream) = data.upstream_provider.as_deref().map(str::trim)
            && !upstream.is_empty()
        {
            provider_spans.push(Span::styled(
                " -> ",
                Style::default().fg(rgb(100, 100, 110)),
            ));
            provider_spans.push(Span::styled(
                upstream.to_string(),
                Style::default().fg(rgb(220, 190, 120)),
            ));
        }
        lines.push(Line::from(provider_spans));
    }

    if let Some(connection) = data
        .connection_type
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
    {
        lines.push(Line::from(vec![
            Span::styled("↔ ", Style::default().fg(rgb(140, 180, 255))),
            Span::styled(
                connection.to_lowercase(),
                Style::default().fg(rgb(140, 180, 255)),
            ),
        ]));
    }

    if data.auth_method != AuthMethod::Unknown {
        let (icon, label, color) = match data.auth_method {
            AuthMethod::AnthropicOAuth => ("🔐", "OAuth", rgb(255, 160, 100)),
            AuthMethod::AnthropicApiKey => ("🔑", "API Key", rgb(180, 180, 190)),
            AuthMethod::OpenAIOAuth => ("🔐", "OAuth", rgb(100, 200, 180)),
            AuthMethod::OpenAIApiKey => ("🔑", "API Key", rgb(180, 180, 190)),
            AuthMethod::OpenRouterApiKey => ("🔑", "API Key", rgb(140, 180, 255)),
            AuthMethod::CopilotOAuth => ("🔐", "OAuth", rgb(110, 200, 140)),
            AuthMethod::GeminiOAuth => ("🔐", "OAuth", rgb(120, 190, 255)),
            AuthMethod::Unknown => unreachable!(),
        };

        if let Some(ref upstream) = data.upstream_provider {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(rgb(140, 140, 150))),
                Span::styled(" via ", Style::default().fg(rgb(100, 100, 110))),
                Span::styled(upstream.clone(), Style::default().fg(rgb(200, 180, 100))),
            ]));
        } else {
            lines.push(Line::from(vec![
                Span::styled(format!("{} ", icon), Style::default().fg(color)),
                Span::styled(label, Style::default().fg(rgb(140, 140, 150))),
            ]));
        }
    }

    if let Some(line) = render_interlang_savings_line(max_len) {
        lines.push(line);
    }
    if let Some(line) = render_interlang_latest_line(max_len) {
        lines.push(line);
    }
    if let Some(line) = render_interlang_estimate_line(max_len) {
        lines.push(line);
    }
    if let Some(line) = render_local_bridge_line(max_len) {
        lines.push(line);
    }

    if let Some(tps) = data.tokens_per_second
        && tps.is_finite()
        && tps > 0.1
    {
        lines.push(Line::from(vec![
            Span::styled("⏱ ", Style::default().fg(rgb(140, 180, 255))),
            Span::styled(
                format!("{:.1} t/s", tps),
                Style::default().fg(rgb(140, 140, 150)),
            ),
        ]));
    }

    if data.usage_info.is_some() {
        lines.push(Line::from(vec![Span::styled(
            "─ limits",
            Style::default().fg(rgb(70, 75, 90)),
        )]));
        lines.extend(usage_render::render_usage_widget(
            data,
            Rect::new(0, 0, inner.width, inner.height),
        ));
    }

    if data.memory_info.is_some() {
        lines.push(Line::from(vec![Span::styled(
            "─ memory",
            Style::default().fg(rgb(70, 75, 90)),
        )]));
        lines.extend(memory_render::render_memory_widget(
            data,
            Rect::new(0, 0, inner.width, inner.height),
        ));
    }

    lines
}

pub(super) fn render_model_info(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(model) = &data.model else {
        return Vec::new();
    };

    let short_name = shorten_model_name(model);
    let max_len = inner.width.saturating_sub(2) as usize;

    let mut spans = vec![Span::styled(
        if short_name.chars().count() > max_len {
            format!(
                "{}...",
                truncate_chars(&short_name, max_len.saturating_sub(3))
            )
        } else {
            short_name
        },
        Style::default().fg(rgb(180, 180, 190)).bold(),
    )];

    append_model_runtime_metadata(&mut spans, data);

    if let Some(mode) = &data.native_compaction_mode {
        let label = if let Some(tokens) = data.native_compaction_threshold_tokens {
            format!("native {} @ {}k", mode, tokens / 1000)
        } else {
            format!("native {}", mode)
        };
        spans.push(Span::styled(" ", Style::default()));
        spans.push(Span::styled(label, Style::default().fg(rgb(120, 210, 230))));
    }

    let mut lines = vec![Line::from(spans)];

    let has_provider = data
        .provider_name
        .as_deref()
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .is_some();
    let has_auth = data.auth_method != AuthMethod::Unknown;

    if has_provider || has_auth {
        let mut detail_spans: Vec<Span> = Vec::new();

        if let Some(provider) = data
            .provider_name
            .as_deref()
            .map(str::trim)
            .filter(|s| !s.is_empty())
        {
            detail_spans.push(Span::styled(
                provider.to_lowercase(),
                Style::default().fg(rgb(140, 180, 255)),
            ));
        }

        if has_auth {
            let (icon, label, _color) = match data.auth_method {
                AuthMethod::AnthropicOAuth => ("🔐", "OAuth", rgb(255, 160, 100)),
                AuthMethod::AnthropicApiKey => ("🔑", "API Key", rgb(180, 180, 190)),
                AuthMethod::OpenAIOAuth => ("🔐", "OAuth", rgb(100, 200, 180)),
                AuthMethod::OpenAIApiKey => ("🔑", "API Key", rgb(180, 180, 190)),
                AuthMethod::OpenRouterApiKey => ("🔑", "API Key", rgb(140, 180, 255)),
                AuthMethod::CopilotOAuth => ("🔐", "OAuth", rgb(110, 200, 140)),
                AuthMethod::GeminiOAuth => ("🔐", "OAuth", rgb(120, 190, 255)),
                AuthMethod::Unknown => unreachable!(),
            };
            if !detail_spans.is_empty() {
                detail_spans.push(Span::styled(" · ", Style::default().fg(rgb(80, 80, 90))));
            }
            detail_spans.push(Span::styled(
                format!("{} {}", icon, label),
                Style::default().fg(rgb(140, 140, 150)),
            ));
        }

        if !detail_spans.is_empty() {
            lines.push(Line::from(detail_spans));
        }
    }

    if data.session_count.is_some() || data.session_name.is_some() {
        let mut parts = Vec::new();

        if let Some(sessions) = data.session_count {
            parts.push(format!(
                "{} session{}",
                sessions,
                if sessions == 1 { "" } else { "s" }
            ));
        }

        if let Some(name) = data.session_name.as_deref()
            && !name.trim().is_empty()
        {
            parts.push(name.to_string());
        }

        if !parts.is_empty() {
            let detail = truncate_smart(&parts.join(" · "), max_len.saturating_sub(2));
            lines.push(Line::from(vec![Span::styled(
                detail,
                Style::default().fg(rgb(140, 140, 150)),
            )]));
        }
    }

    if let Some(line) = render_interlang_savings_line(max_len) {
        lines.push(line);
    }
    if let Some(line) = render_interlang_latest_line(max_len) {
        lines.push(line);
    }
    if let Some(line) = render_interlang_estimate_line(max_len) {
        lines.push(line);
    }

    lines
}

fn render_interlang_savings_line(max_len: usize) -> Option<Line<'static>> {
    let status = crate::interlang::status_json();
    let enabled = status
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    if !enabled {
        return None;
    }
    let saved = status
        .get("exact_saved_tokens")
        .or_else(|| status.get("saved_tokens_estimate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let events = status.get("events").and_then(|v| v.as_u64()).unwrap_or(0);
    let mode = status
        .get("mode")
        .and_then(|v| v.as_str())
        .unwrap_or("safe");
    let label = if events == 0 || saved <= 0 {
        format!("IL {mode} · saving ready")
    } else {
        let approx = if status
            .get("exact_tokenizer")
            .and_then(|v| v.as_bool())
            .unwrap_or(false)
        {
            ""
        } else {
            "~"
        };
        format!(
            "IL {mode} · saved {approx}{} tok",
            format_token_count(saved as u64)
        )
    };
    Some(Line::from(vec![
        Span::styled("↯ ", Style::default().fg(rgb(120, 210, 230))),
        Span::styled(
            truncate_smart(&label, max_len.saturating_sub(2)),
            Style::default().fg(rgb(120, 210, 230)),
        ),
    ]))
}

fn render_interlang_latest_line(max_len: usize) -> Option<Line<'static>> {
    let status = crate::interlang::status_json();
    if !status
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let events = status.get("events").and_then(|v| v.as_u64()).unwrap_or(0);
    if events == 0 {
        return None;
    }
    let last = status
        .get("last_exact_saved_tokens")
        .or_else(|| status.get("last_saved_tokens_estimate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let blocks = status
        .get("last_blocks_encoded")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let avoided = status
        .get("last_raw_context_avoided_tokens_estimate")
        .and_then(|v| v.as_i64())
        .unwrap_or(0);
    let approx = if status
        .get("exact_tokenizer")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        ""
    } else {
        "~"
    };
    let label = if avoided > 0 {
        format!(
            "L {approx}{} (+{} raw)",
            format_token_count(last.max(0) as u64),
            format_token_count(avoided as u64)
        )
    } else if last > 0 {
        format!("L {approx}{} tok", format_token_count(last as u64))
    } else if blocks == 0 {
        "L 0 tok".to_string()
    } else {
        "L pending".to_string()
    };
    Some(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            truncate_smart(&label, max_len.saturating_sub(2)),
            Style::default().fg(rgb(100, 170, 190)),
        ),
    ]))
}

fn render_interlang_estimate_line(max_len: usize) -> Option<Line<'static>> {
    let status = crate::interlang::status_json();
    if !status
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let saved = status
        .get("last_exact_saved_tokens")
        .or_else(|| status.get("last_saved_tokens_estimate"))
        .and_then(|v| v.as_i64())
        .unwrap_or(0)
        .max(0) as u64;
    if saved == 0 {
        return None;
    }

    let label = format_interlang_gpt55_estimate(saved);

    Some(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            truncate_smart(&label, max_len.saturating_sub(2)),
            Style::default().fg(rgb(120, 200, 140)),
        ),
    ]))
}

fn render_local_bridge_line(max_len: usize) -> Option<Line<'static>> {
    let status = crate::local_model::status_json();
    if !status
        .get("enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false)
    {
        return None;
    }
    let api = status
        .get("api_exchanges")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let mem = status
        .get("distilled_records")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let promoted = status
        .get("promoted_records")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let routes = status
        .get("pre_route_events")
        .and_then(|v| v.as_u64())
        .unwrap_or(0);
    let enrich = status
        .get("enrich_enabled")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let label = if api == 0 && mem == 0 {
        "LM bridge · capture ready".to_string()
    } else {
        format!(
            "LM bridge · {} route · {} api → {} mem/{} promo{}",
            routes,
            api,
            mem,
            promoted,
            if enrich { " +oss" } else { "" }
        )
    };
    Some(Line::from(vec![
        Span::styled("  ", Style::default()),
        Span::styled(
            truncate_smart(&label, max_len.saturating_sub(2)),
            Style::default().fg(rgb(180, 145, 240)),
        ),
    ]))
}

fn format_interlang_gpt55_estimate(saved_tokens: u64) -> String {
    // GPT-5.5 pricing/reference from OpenAI docs: 1,050,000 token context,
    // $5/M input tokens, and >272K-token prompts price input at 2x.
    const GPT55_CONTEXT_WINDOW: f64 = 1_050_000.0;
    const GPT55_INPUT_PER_MILLION: f64 = 5.0;
    const GPT55_HIGH_CONTEXT_MULTIPLIER: f64 = 2.0;

    let tokens = saved_tokens as f64;
    let context_pct = tokens / GPT55_CONTEXT_WINDOW * 100.0;
    let base_dollars = tokens / 1_000_000.0 * GPT55_INPUT_PER_MILLION;
    let high_context_dollars = base_dollars * GPT55_HIGH_CONTEXT_MULTIPLIER;
    format!(
        "est ${:.3} input (${:.3} hi) · {:.2}% ctx",
        base_dollars, high_context_dollars, context_pct
    )
}

fn format_token_count(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 10_000 {
        format!("{}k", tokens / 1_000)
    } else if tokens >= 1_000 {
        format!("{:.1}k", tokens as f64 / 1_000.0)
    } else {
        tokens.to_string()
    }
}

pub(super) fn shorten_model_name(model: &str) -> String {
    if model.contains("claude") {
        if model.contains("opus-4-5") || model.contains("opus-4.5") {
            return "opus-4.5".to_string();
        }
        if model.contains("sonnet-4") {
            return "sonnet-4".to_string();
        }
        if model.contains("sonnet-3-5") || model.contains("sonnet-3.5") {
            return "sonnet-3.5".to_string();
        }
        if model.contains("haiku") {
            return "haiku".to_string();
        }
        if let Some(idx) = model.find("claude-") {
            let rest = &model[idx + 7..];
            if let Some(end) = rest.find('-') {
                return rest[..end].to_string();
            }
        }
    }

    if model.contains("gpt")
        && let Some(start) = model.find("gpt-")
    {
        let rest = &model[start..];
        let parts: Vec<&str> = rest.splitn(3, '-').collect();
        if parts.len() >= 2 {
            return format!("{}-{}", parts[0], parts[1]);
        }
    }

    if model.len() > 15 {
        format!("{}…", crate::util::truncate_str(model, 14))
    } else {
        model.to_string()
    }
}

fn append_model_runtime_metadata(spans: &mut Vec<Span<'static>>, data: &InfoWidgetData) {
    if let Some(effort) = data
        .reasoning_effort
        .as_deref()
        .and_then(short_reasoning_effort)
    {
        spans.push(Span::styled(" ", Style::default()));
        spans.push(Span::styled(
            format!("({effort})"),
            Style::default().fg(rgb(255, 200, 100)),
        ));
    }

    if let Some(tier) = data.service_tier.as_deref().and_then(short_service_tier) {
        spans.push(Span::styled(" ", Style::default()));
        spans.push(Span::styled(
            format!("[{tier}]"),
            Style::default().fg(rgb(200, 140, 255)).bold(),
        ));
    }
}

fn short_reasoning_effort(effort: &str) -> Option<&str> {
    let effort = effort.trim();
    if effort.is_empty() {
        return None;
    }
    Some(match effort {
        "xhigh" => "xhi",
        "high" => "hi",
        "medium" => "med",
        "low" => "lo",
        "none" => "∅",
        other => other,
    })
}

fn short_service_tier(service_tier: &str) -> Option<&str> {
    let service_tier = service_tier.trim();
    if service_tier.is_empty() || service_tier == "off" || service_tier == "default" {
        return None;
    }
    Some(match service_tier {
        "priority" => "fast",
        "flex" => "flex",
        other => other,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::info_widget::InfoWidgetData;

    fn data() -> InfoWidgetData {
        InfoWidgetData {
            todos: Vec::new(),
            context_info: None,
            queue_mode: None,
            context_limit: None,
            model: Some("gpt-5-codex".to_string()),
            reasoning_effort: Some("high".to_string()),
            service_tier: Some("priority".to_string()),
            native_compaction_mode: None,
            native_compaction_threshold_tokens: None,
            session_count: None,
            session_name: None,
            client_count: None,
            memory_info: None,
            swarm_info: None,
            background_info: None,
            usage_info: None,
            tokens_per_second: None,
            provider_name: None,
            auth_method: crate::tui::info_widget::AuthMethod::Unknown,
            upstream_provider: None,
            connection_type: None,
            diagrams: Vec::new(),
            workspace_rows: Vec::new(),
            workspace_animation_tick: 0,
            ambient_info: None,
            observed_context_tokens: None,
            is_compacting: false,
            git_info: None,
        }
    }

    fn first_line_text(lines: Vec<Line<'static>>) -> String {
        lines
            .into_iter()
            .next()
            .expect("first model line")
            .spans
            .into_iter()
            .map(|span| span.content.into_owned())
            .collect::<String>()
    }

    #[test]
    fn model_widget_and_overview_show_same_runtime_metadata() {
        let rect = Rect::new(0, 0, 40, 8);
        let data = data();

        let standalone = first_line_text(render_model_widget(&data, rect));
        let overview = first_line_text(render_model_info(&data, rect));

        assert!(standalone.contains("(hi)"));
        assert!(standalone.contains("[fast]"));
        assert!(overview.contains("(hi)"));
        assert!(overview.contains("[fast]"));
    }

    #[test]
    fn interlang_gpt55_estimate_uses_expected_pricing_and_context() {
        let estimate = format_interlang_gpt55_estimate(105_000);

        assert_eq!(estimate, "est $0.525 input ($1.050 hi) · 10.00% ctx");
    }
}
