use crate::tui::color_support::rgb;
use ratatui::prelude::*;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

pub fn wrap_line(
    line: Line<'static>,
    width: usize,
    repeated_gutter_prefix: impl Fn(&Line<'static>) -> Option<(Vec<Span<'static>>, usize)>,
) -> Vec<Line<'static>> {
    if width == 0 {
        return vec![line];
    }

    let alignment = line.alignment;

    let repeated_prefix = repeated_gutter_prefix(&line).and_then(|(prefix_spans, prefix_width)| {
        if prefix_width == 0 || prefix_width >= width {
            None
        } else {
            Some((prefix_spans, prefix_width))
        }
    });

    let seed_repeated_prefix =
        |current_spans: &mut Vec<Span<'static>>, current_width: &mut usize, pending: &mut bool| {
            if *pending {
                if let Some((prefix_spans, prefix_width)) = &repeated_prefix {
                    current_spans.extend(prefix_spans.iter().cloned());
                    *current_width = *prefix_width;
                }
                *pending = false;
            }
        };

    if let Some(balanced) = wrap_line_balanced(&line, width) {
        return balanced;
    }

    let initial_prefix_width = repeated_prefix
        .as_ref()
        .map(|(_, prefix_width)| *prefix_width)
        .unwrap_or(0);

    let mut result: Vec<Line<'static>> = Vec::new();
    let mut current_spans: Vec<Span<'static>> = Vec::with_capacity(line.spans.len());
    let mut current_width = 0usize;
    let mut current_has_content = false;
    let mut pending_repeated_prefix = false;

    for span in line.spans {
        let style = span.style;
        let text = span.content.as_ref();

        let mut remaining = text;
        while !remaining.is_empty() {
            let (chunk, rest) = if let Some(space_idx) = remaining.find(' ') {
                let (word, after_space) = remaining.split_at(space_idx);
                if after_space.len() > 1 {
                    let mut buf = String::with_capacity(word.len() + 1);
                    buf.push_str(word);
                    buf.push(' ');
                    (buf, &after_space[1..])
                } else {
                    let mut buf = String::with_capacity(word.len() + 1);
                    buf.push_str(word);
                    buf.push(' ');
                    (buf, "")
                }
            } else {
                (remaining.to_string(), "")
            };
            remaining = rest;

            let chunk_width = chunk.width();

            if current_width + chunk_width > width && current_has_content {
                let mut new_line = Line::from(std::mem::take(&mut current_spans));
                if let Some(align) = alignment {
                    new_line = new_line.alignment(align);
                }
                result.push(new_line);
                current_width = 0;
                current_has_content = false;
                pending_repeated_prefix = repeated_prefix.is_some();
            }

            if chunk_width > width {
                let mut part = String::new();
                let mut part_width = 0usize;

                for c in chunk.chars() {
                    seed_repeated_prefix(
                        &mut current_spans,
                        &mut current_width,
                        &mut pending_repeated_prefix,
                    );
                    let char_width = c.width().unwrap_or(0);

                    if current_width + part_width + char_width > width
                        && (current_width + part_width) > 0
                    {
                        if !part.is_empty() {
                            current_spans.push(Span::styled(std::mem::take(&mut part), style));
                            let width_before = current_width;
                            current_width += part_width;
                            if width_before + part_width > initial_prefix_width {
                                current_has_content = true;
                            }
                            part_width = 0;
                        }

                        if current_has_content {
                            let mut new_line = Line::from(std::mem::take(&mut current_spans));
                            if let Some(align) = alignment {
                                new_line = new_line.alignment(align);
                            }
                            result.push(new_line);
                            current_width = 0;
                            current_has_content = false;
                            pending_repeated_prefix = repeated_prefix.is_some();
                        }

                        seed_repeated_prefix(
                            &mut current_spans,
                            &mut current_width,
                            &mut pending_repeated_prefix,
                        );
                    }

                    part.push(c);
                    part_width += char_width;
                }

                if !part.is_empty() {
                    seed_repeated_prefix(
                        &mut current_spans,
                        &mut current_width,
                        &mut pending_repeated_prefix,
                    );
                    current_spans.push(Span::styled(part, style));
                    let width_before = current_width;
                    current_width += part_width;
                    if width_before + part_width > initial_prefix_width {
                        current_has_content = true;
                    }
                }
            } else {
                seed_repeated_prefix(
                    &mut current_spans,
                    &mut current_width,
                    &mut pending_repeated_prefix,
                );
                current_spans.push(Span::styled(chunk, style));
                let width_before = current_width;
                current_width += chunk_width;
                if width_before + chunk_width > initial_prefix_width {
                    current_has_content = true;
                }
            }
        }
    }

    if !current_spans.is_empty() && current_has_content {
        let mut new_line = Line::from(current_spans);
        if let Some(align) = alignment {
            new_line = new_line.alignment(align);
        }
        result.push(new_line);
    }

    if result.is_empty() {
        let mut empty_line = Line::from("");
        if let Some(align) = alignment {
            empty_line = empty_line.alignment(align);
        }
        result.push(empty_line);
    }

    result
}

#[derive(Clone)]
struct StyledPiece {
    text: String,
    style: Style,
}

#[derive(Clone)]
struct WrapToken {
    word: Vec<StyledPiece>,
    spaces: Vec<StyledPiece>,
    word_width: usize,
    space_width: usize,
}

fn wrap_line_balanced(line: &Line<'static>, width: usize) -> Option<Vec<Line<'static>>> {
    let alignment = line.alignment?;
    if alignment == Alignment::Left {
        return None;
    }

    let flat_text: String = line
        .spans
        .iter()
        .map(|span| span.content.as_ref())
        .collect();
    if UnicodeWidthStr::width(flat_text.as_str()) <= width || !flat_text.contains(' ') {
        return None;
    }
    if flat_text.starts_with(char::is_whitespace)
        || flat_text.ends_with(char::is_whitespace)
        || flat_text.contains("  ")
        || flat_text.contains('\t')
    {
        return None;
    }

    let tokens = tokenize_balanced_wrap(line)?;
    if tokens.len() < 3 || tokens.iter().any(|token| token.word_width > width) {
        return None;
    }

    let (breaks, line_count) = balanced_wrap_breaks(&tokens, width)?;
    if line_count <= 1 {
        return None;
    }

    let mut result = Vec::with_capacity(line_count);
    let mut start = 0usize;
    while start < tokens.len() {
        let end = breaks[start];
        if end <= start {
            return None;
        }
        let spans = build_balanced_line_spans(&tokens[start..end]);
        result.push(Line::from(spans).alignment(alignment));
        start = end;
    }
    Some(result)
}

fn tokenize_balanced_wrap(line: &Line<'static>) -> Option<Vec<WrapToken>> {
    let mut tokens = Vec::new();
    let mut word = Vec::new();
    let mut spaces = Vec::new();
    let mut word_width = 0usize;
    let mut space_width = 0usize;
    let mut seen_word_char = false;
    let mut in_spaces = false;

    for span in &line.spans {
        let style = span.style;
        for ch in span.content.chars() {
            let ch_width = UnicodeWidthChar::width(ch).unwrap_or(0);
            if ch.is_whitespace() {
                if !seen_word_char {
                    return None;
                }
                in_spaces = true;
                push_piece_char(&mut spaces, ch, style);
                space_width += ch_width;
            } else {
                if in_spaces {
                    tokens.push(WrapToken {
                        word: std::mem::take(&mut word),
                        spaces: std::mem::take(&mut spaces),
                        word_width,
                        space_width,
                    });
                    word_width = 0;
                    space_width = 0;
                    in_spaces = false;
                }
                seen_word_char = true;
                push_piece_char(&mut word, ch, style);
                word_width += ch_width;
            }
        }
    }

    if !seen_word_char || in_spaces {
        return None;
    }

    tokens.push(WrapToken {
        word,
        spaces,
        word_width,
        space_width,
    });
    Some(tokens)
}

fn push_piece_char(pieces: &mut Vec<StyledPiece>, ch: char, style: Style) {
    if let Some(last) = pieces.last_mut()
        && last.style == style
    {
        last.text.push(ch);
        return;
    }
    pieces.push(StyledPiece {
        text: ch.to_string(),
        style,
    });
}

fn balanced_wrap_breaks(tokens: &[WrapToken], width: usize) -> Option<(Vec<usize>, usize)> {
    let n = tokens.len();
    let mut dp = vec![usize::MAX; n + 1];
    let mut breaks = vec![0usize; n];
    let mut line_counts = vec![usize::MAX; n + 1];
    dp[n] = 0;
    line_counts[n] = 0;

    for start in (0..n).rev() {
        let mut line_width = 0usize;
        for end in start..n {
            if end == start {
                line_width = tokens[end].word_width;
            } else {
                line_width = line_width
                    .saturating_add(tokens[end - 1].space_width)
                    .saturating_add(tokens[end].word_width);
            }

            if line_width > width {
                break;
            }

            if dp[end + 1] == usize::MAX {
                continue;
            }

            let slack = width - line_width;
            let cost = slack.saturating_mul(slack).saturating_add(dp[end + 1]);
            let lines_used = 1usize.saturating_add(line_counts[end + 1]);

            let should_replace = cost < dp[start]
                || (cost == dp[start] && lines_used < line_counts[start])
                || (cost == dp[start]
                    && lines_used == line_counts[start]
                    && line_width < line_width_for_break(tokens, start, breaks[start]));

            if should_replace {
                dp[start] = cost;
                breaks[start] = end + 1;
                line_counts[start] = lines_used;
            }
        }
    }

    if dp[0] == usize::MAX {
        None
    } else {
        Some((breaks, line_counts[0]))
    }
}

fn line_width_for_break(tokens: &[WrapToken], start: usize, end: usize) -> usize {
    if end <= start {
        return usize::MAX;
    }
    let mut width = 0usize;
    for idx in start..end {
        if idx > start {
            width = width.saturating_add(tokens[idx - 1].space_width);
        }
        width = width.saturating_add(tokens[idx].word_width);
    }
    width
}

fn build_balanced_line_spans(tokens: &[WrapToken]) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    for (idx, token) in tokens.iter().enumerate() {
        for piece in &token.word {
            spans.push(Span::styled(piece.text.clone(), piece.style));
        }
        if idx + 1 < tokens.len() {
            for piece in &token.spaces {
                spans.push(Span::styled(piece.text.clone(), piece.style));
            }
        }
    }
    spans
}

pub fn wrap_lines(
    lines: Vec<Line<'static>>,
    width: usize,
    repeated_gutter_prefix: impl Fn(&Line<'static>) -> Option<(Vec<Span<'static>>, usize)> + Copy,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .flat_map(|line| wrap_line(line, width, repeated_gutter_prefix))
        .collect()
}

pub fn progress_bar(progress: f32, width: usize) -> String {
    let filled = (progress * width as f32) as usize;
    let empty = width.saturating_sub(filled);

    std::iter::repeat_n('█', filled)
        .chain(std::iter::repeat_n('░', empty))
        .collect()
}

pub fn progress_line(label: &str, progress: f32, width: usize) -> Line<'static> {
    let bar = progress_bar(progress, width.saturating_sub(label.len() + 3));
    let pct = (progress * 100.0) as u8;

    Line::from(vec![
        Span::styled(label.to_string(), Style::default().dim()),
        Span::raw(" "),
        Span::styled(bar, Style::default().fg(rgb(129, 199, 132))),
        Span::styled(format!(" {}%", pct), Style::default().dim()),
    ])
}
