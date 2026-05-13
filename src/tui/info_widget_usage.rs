use super::{InfoWidgetData, UsageInfo, UsageProvider};
use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use unicode_width::UnicodeWidthStr;

#[derive(Debug, Clone)]
struct LocalLimitEstimator {
    reset_key: Option<String>,
    official_used_pct: f32,
    anchor_weighted_units: f64,
    units_per_percent: f64,
    confidence: f64,
}

impl Default for LocalLimitEstimator {
    fn default() -> Self {
        Self {
            reset_key: None,
            official_used_pct: 0.0,
            anchor_weighted_units: 0.0,
            units_per_percent: 50_000.0,
            confidence: 0.0,
        }
    }
}

static LOCAL_LIMIT_ESTIMATORS: OnceLock<Mutex<HashMap<String, LocalLimitEstimator>>> =
    OnceLock::new();

pub(super) fn render_usage_widget(data: &InfoWidgetData, inner: Rect) -> Vec<Line<'static>> {
    let Some(info) = &data.usage_info else {
        return Vec::new();
    };
    if !info.available {
        return Vec::new();
    }

    match info.provider {
        UsageProvider::Copilot => {
            vec![Line::from(vec![Span::styled(
                format!(
                    "{} in + {} out",
                    format_tokens(info.input_tokens),
                    format_tokens(info.output_tokens)
                ),
                Style::default().fg(rgb(140, 140, 150)),
            )])]
        }
        UsageProvider::CostBased => {
            vec![
                Line::from(vec![
                    Span::styled("💰 ", Style::default().fg(rgb(140, 180, 255))),
                    Span::styled(
                        format!("${:.4}", info.total_cost),
                        Style::default().fg(rgb(180, 180, 190)).bold(),
                    ),
                ]),
                Line::from(vec![Span::styled(
                    format!(
                        "{} in + {} out",
                        format_tokens(info.input_tokens),
                        format_tokens(info.output_tokens)
                    ),
                    Style::default().fg(rgb(140, 140, 150)),
                )]),
            ]
        }
        _ => {
            let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
            let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
            let five_hr_left = percent_left(info.five_hour);
            let seven_day_left = percent_left(info.seven_day);
            let weighted_units = weighted_local_units(info);
            let five_hr_est_left = (info.provider == UsageProvider::OpenAI).then(|| {
                estimate_left_percent(
                    "openai:5h",
                    info.five_hour,
                    info.five_hour_resets_at.as_deref(),
                    weighted_units,
                )
            });
            let seven_day_est_left = (info.provider == UsageProvider::OpenAI).then(|| {
                estimate_left_percent(
                    "openai:weekly",
                    info.seven_day,
                    info.seven_day_resets_at.as_deref(),
                    weighted_units,
                )
            });

            let five_hr_reset = info
                .five_hour_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);
            let seven_day_reset = info
                .seven_day_resets_at
                .as_deref()
                .map(crate::usage::format_reset_time);

            let mut lines = Vec::new();
            let label = info.provider.label();
            if !label.is_empty() {
                lines.push(Line::from(vec![Span::styled(
                    format!("{} limits", label),
                    Style::default()
                        .fg(rgb(140, 140, 150))
                        .add_modifier(ratatui::style::Modifier::DIM),
                )]));
            }
            lines.push(render_labeled_bar(
                "5-hour",
                five_hr_used,
                five_hr_left,
                five_hr_est_left,
                five_hr_reset.as_deref(),
                inner.width,
            ));
            lines.push(render_labeled_bar(
                "Weekly",
                seven_day_used,
                seven_day_left,
                seven_day_est_left,
                seven_day_reset.as_deref(),
                inner.width,
            ));
            if let Some(spark_usage) = info.spark {
                let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
                let spark_left = percent_left(spark_usage);
                let spark_reset = info
                    .spark_resets_at
                    .as_deref()
                    .map(crate::usage::format_reset_time);
                lines.push(render_labeled_bar(
                    "Spark",
                    spark_used,
                    spark_left,
                    None,
                    spark_reset.as_deref(),
                    inner.width,
                ));
            }
            lines
        }
    }
}

pub(super) fn render_usage_compact(info: &UsageInfo, width: u16) -> Vec<Line<'static>> {
    if !info.available {
        return Vec::new();
    }

    let five_hr_used = (info.five_hour * 100.0).round().clamp(0.0, 100.0) as u8;
    let seven_day_used = (info.seven_day * 100.0).round().clamp(0.0, 100.0) as u8;
    let five_hr_left = percent_left(info.five_hour);
    let seven_day_left = percent_left(info.seven_day);
    let weighted_units = weighted_local_units(info);
    let five_hr_est_left = (info.provider == UsageProvider::OpenAI).then(|| {
        estimate_left_percent(
            "openai:5h",
            info.five_hour,
            info.five_hour_resets_at.as_deref(),
            weighted_units,
        )
    });
    let seven_day_est_left = (info.provider == UsageProvider::OpenAI).then(|| {
        estimate_left_percent(
            "openai:weekly",
            info.seven_day,
            info.seven_day_resets_at.as_deref(),
            weighted_units,
        )
    });
    let five_hr_reset = info
        .five_hour_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);
    let seven_day_reset = info
        .seven_day_resets_at
        .as_deref()
        .map(crate::usage::format_reset_time);

    let mut lines = Vec::new();
    let label = info.provider.label();
    if !label.is_empty() {
        lines.push(Line::from(vec![Span::styled(
            format!("{} limits", label),
            Style::default()
                .fg(rgb(140, 140, 150))
                .add_modifier(ratatui::style::Modifier::DIM),
        )]));
    }
    lines.push(render_labeled_bar(
        "5-hour",
        five_hr_used,
        five_hr_left,
        five_hr_est_left,
        five_hr_reset.as_deref(),
        width,
    ));
    lines.push(render_labeled_bar(
        "Weekly",
        seven_day_used,
        seven_day_left,
        seven_day_est_left,
        seven_day_reset.as_deref(),
        width,
    ));
    if let Some(spark_usage) = info.spark {
        let spark_used = (spark_usage * 100.0).round().clamp(0.0, 100.0) as u8;
        let spark_left = percent_left(spark_usage);
        let spark_reset = info
            .spark_resets_at
            .as_deref()
            .map(crate::usage::format_reset_time);
        lines.push(render_labeled_bar(
            "Spark",
            spark_used,
            spark_left,
            None,
            spark_reset.as_deref(),
            width,
        ));
    }
    lines
}

fn percent_left(used_fraction: f32) -> f32 {
    (100.0 - used_fraction * 100.0).clamp(0.0, 100.0)
}

fn weighted_local_units(info: &UsageInfo) -> f64 {
    // Output/reasoning tends to be more rate-limit expensive than plain prompt
    // text in practice, so weight it more heavily. This is intentionally a local
    // estimator, not an official OpenAI value.
    info.input_tokens as f64 + info.output_tokens as f64 * 4.0
}

fn estimate_left_percent(
    key: &str,
    official_used_fraction: f32,
    reset_key: Option<&str>,
    weighted_units: f64,
) -> f32 {
    let official_used_pct = (official_used_fraction * 100.0).clamp(0.0, 100.0);
    let map = LOCAL_LIMIT_ESTIMATORS.get_or_init(|| Mutex::new(HashMap::new()));
    let Ok(mut map) = map.lock() else {
        return percent_left(official_used_fraction);
    };
    let state = map.entry(key.to_string()).or_default();

    if state.reset_key.as_deref() != reset_key || official_used_pct < state.official_used_pct {
        *state = LocalLimitEstimator {
            reset_key: reset_key.map(str::to_string),
            official_used_pct,
            anchor_weighted_units: weighted_units,
            ..LocalLimitEstimator::default()
        };
        return percent_left(official_used_fraction);
    }

    let official_delta = official_used_pct - state.official_used_pct;
    let local_delta = (weighted_units - state.anchor_weighted_units).max(0.0);
    if official_delta >= 1.0 && local_delta > 0.0 {
        let sample_units_per_percent =
            (local_delta / official_delta as f64).clamp(2_000.0, 2_000_000.0);
        let alpha = if state.confidence <= 0.0 { 1.0 } else { 0.35 };
        state.units_per_percent =
            state.units_per_percent * (1.0 - alpha) + sample_units_per_percent * alpha;
        state.confidence = (state.confidence + official_delta as f64).min(20.0);
        state.official_used_pct = official_used_pct;
        state.anchor_weighted_units = weighted_units;
        return percent_left(official_used_fraction);
    }

    let estimated_extra_pct = if state.units_per_percent > 0.0 {
        local_delta / state.units_per_percent
    } else {
        0.0
    };
    let estimated_used_pct =
        (state.official_used_pct + estimated_extra_pct as f32).clamp(0.0, 100.0);
    (100.0 - estimated_used_pct).clamp(0.0, 100.0)
}

fn render_labeled_bar(
    label: &str,
    _used_pct: u8,
    left_pct: f32,
    estimated_left_pct: Option<f32>,
    reset_time: Option<&str>,
    _width: u16,
) -> Line<'static> {
    let display_left_pct = estimated_left_pct.unwrap_or(left_pct);
    let status_color = if display_left_pct <= 20.0 {
        rgb(255, 100, 100)
    } else if display_left_pct <= 50.0 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };

    let suffix = if display_left_pct <= 0.005 {
        if let Some(reset) = reset_time {
            format!(" resets {}", reset)
        } else {
            " 0.00% left".to_string()
        }
    } else if let Some(estimated) = estimated_left_pct {
        format!(" ~{} left", format_left_percent(estimated))
    } else {
        format!(" {} left", format_left_percent(left_pct))
    };

    let padded_label = format!("{:<7}", label);

    Line::from(vec![
        Span::styled(padded_label, Style::default().fg(rgb(140, 140, 150))),
        Span::styled("∞", Style::default().fg(rainbow_infinity_color())),
        Span::styled(suffix, Style::default().fg(status_color)),
    ])
}

fn rainbow_infinity_color() -> Color {
    const RAINBOW: [(u8, u8, u8); 7] = [
        (255, 80, 80),
        (255, 160, 60),
        (255, 230, 80),
        (90, 220, 120),
        (80, 180, 255),
        (130, 110, 255),
        (220, 100, 255),
    ];

    let step = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| (duration.as_millis() / 220) as usize)
        .unwrap_or(0);
    let (r, g, b) = RAINBOW[step % RAINBOW.len()];
    rgb(r, g, b)
}

fn format_left_percent(left_pct: f32) -> String {
    if (left_pct.fract()).abs() < 0.005 {
        format!("{}%", left_pct.round() as u8)
    } else {
        format!("{:.2}%", left_pct)
    }
}

pub(super) fn render_usage_bar(
    used_tokens: usize,
    limit_tokens: usize,
    width: u16,
) -> Line<'static> {
    let safe_limit = limit_tokens.max(1);
    let bar_width = width.saturating_sub(2).min(24) as usize;
    if bar_width == 0 {
        return Line::default();
    }

    let mut used_cells = ((used_tokens as f64 / safe_limit as f64) * bar_width as f64)
        .round()
        .max(0.0) as usize;
    if used_cells > bar_width {
        used_cells = bar_width;
    }

    let used_pct = ((used_tokens as f64 / safe_limit as f64) * 100.0)
        .round()
        .clamp(0.0, 100.0) as u8;
    let left_pct = 100u8.saturating_sub(used_pct);
    let used_color = if left_pct <= 20 {
        rgb(255, 100, 100)
    } else if left_pct <= 50 {
        rgb(255, 200, 100)
    } else {
        rgb(100, 200, 100)
    };

    let label = format!(
        "{}/{}",
        format_token_k(used_tokens),
        format_token_k(limit_tokens)
    );
    let show_label = UnicodeWidthStr::width(label.as_str()) <= bar_width;
    let mut spans = Vec::new();
    spans.push(Span::styled("[", Style::default().fg(rgb(90, 90, 100))));
    if show_label {
        let label_start = (bar_width - label.len()) / 2;
        let label_end = label_start + label.len();
        for idx in 0..bar_width {
            let in_used = idx < used_cells;
            let base_char = if in_used { '█' } else { '░' };
            let ch = if idx >= label_start && idx < label_end {
                label.as_bytes()[idx - label_start] as char
            } else {
                base_char
            };
            let style = if idx >= label_start && idx < label_end {
                if in_used {
                    Style::default().fg(rgb(20, 30, 35)).bold()
                } else {
                    Style::default().fg(rgb(170, 170, 180)).bold()
                }
            } else if in_used {
                Style::default().fg(used_color)
            } else {
                Style::default().fg(rgb(50, 50, 60))
            };
            spans.push(Span::styled(ch.to_string(), style));
        }
    } else {
        let empty_cells = bar_width.saturating_sub(used_cells);
        spans.push(Span::styled(
            "█".repeat(used_cells),
            Style::default().fg(used_color),
        ));
        if empty_cells > 0 {
            spans.push(Span::styled(
                "░".repeat(empty_cells),
                Style::default().fg(rgb(50, 50, 60)),
            ));
        }
    }
    spans.push(Span::styled("]", Style::default().fg(rgb(90, 90, 100))));
    Line::from(spans)
}

pub(super) fn render_context_usage_line(
    label: &str,
    used_tokens: usize,
    limit_tokens: usize,
    width: u16,
) -> Line<'static> {
    let label_width = UnicodeWidthStr::width(label);
    let bar_width = width.saturating_sub(label_width as u16 + 1);

    if bar_width < 3 {
        return Line::from(vec![
            Span::styled(label.to_string(), Style::default().fg(rgb(140, 140, 150))),
            Span::raw(" "),
            Span::styled(
                format!(
                    "{}/{}",
                    format_token_k(used_tokens),
                    format_token_k(limit_tokens)
                ),
                Style::default().fg(rgb(100, 200, 100)).bold(),
            ),
        ]);
    }

    let mut spans = vec![Span::styled(
        format!("{label} "),
        Style::default().fg(rgb(140, 140, 150)),
    )];
    spans.extend(render_usage_bar(used_tokens, limit_tokens, bar_width).spans);
    Line::from(spans)
}

fn format_token_k(tokens: usize) -> String {
    if tokens >= 1000 {
        format!("{}k", tokens / 1000)
    } else {
        format!("{}", tokens)
    }
}

fn format_tokens(tokens: u64) -> String {
    if tokens >= 1_000_000 {
        format!("{:.1}M", tokens as f64 / 1_000_000.0)
    } else if tokens >= 1_000 {
        format!("{:.1}K", tokens as f64 / 1_000.0)
    } else {
        format!("{}", tokens)
    }
}

#[cfg(test)]
mod tests {
    use super::{estimate_left_percent, format_left_percent};

    #[test]
    fn left_percent_omits_fake_decimal_precision_for_whole_values() {
        assert_eq!(format_left_percent(81.0), "81%");
        assert_eq!(format_left_percent(94.004), "94%");
    }

    #[test]
    fn left_percent_keeps_real_decimal_precision() {
        assert_eq!(format_left_percent(94.54), "94.54%");
        assert_eq!(format_left_percent(12.345), "12.35%");
    }

    #[test]
    fn local_estimator_interpolates_between_official_percent_jumps() {
        let key = format!(
            "test:{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        );

        assert_eq!(estimate_left_percent(&key, 0.19, Some("r1"), 0.0), 81.0);
        assert_eq!(
            estimate_left_percent(&key, 0.20, Some("r1"), 10_000.0),
            80.0
        );
        let interpolated = estimate_left_percent(&key, 0.20, Some("r1"), 12_500.0);

        assert!((interpolated - 79.75).abs() < 0.001);
    }
}
