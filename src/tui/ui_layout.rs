use crate::tui::color_support;
use ratatui::prelude::*;
use ratatui::widgets::Paragraph;

pub(super) fn clear_area(frame: &mut Frame, area: Rect) {
    color_support::clear_buf(area, frame.buffer_mut());
}

pub(crate) fn left_aligned_content_inset(width: u16, centered: bool) -> u16 {
    if centered || width <= 1 { 0 } else { 1 }
}

pub(crate) fn centered_content_block_width(width: u16, max_width: usize) -> usize {
    (width as usize).min(max_width).max(1)
}

pub(crate) fn left_pad_lines_to_block_width(
    lines: &mut [Line<'static>],
    width: u16,
    block_width: usize,
) {
    let block_width = block_width.min(width as usize);
    let pad = (width as usize).saturating_sub(block_width) / 2;
    for line in lines {
        if pad > 0 {
            line.spans.insert(0, Span::raw(" ".repeat(pad)));
        }
        line.alignment = Some(ratatui::layout::Alignment::Left);
    }
}

const RIGHT_RAIL_HEADER_HEIGHT: u16 = 1;

pub(super) fn right_rail_border_style(focused: bool, focus_color: Color) -> Style {
    let border_color = if focused {
        focus_color
    } else {
        super::theme_support::dim_color()
    };
    Style::default().fg(border_color)
}

fn right_rail_inner(area: Rect) -> Rect {
    ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT)
        .inner(area)
}

fn right_rail_content_area(area: Rect) -> Option<Rect> {
    let inner = right_rail_inner(area);
    if inner.width == 0 || inner.height <= RIGHT_RAIL_HEADER_HEIGHT {
        return None;
    }

    Some(Rect {
        x: inner.x,
        y: inner.y + RIGHT_RAIL_HEADER_HEIGHT,
        width: inner.width,
        height: inner.height - RIGHT_RAIL_HEADER_HEIGHT,
    })
}

pub(super) fn draw_right_rail_chrome(
    frame: &mut Frame,
    area: Rect,
    title: Line<'static>,
    border_style: Style,
) -> Option<Rect> {
    let inner = right_rail_inner(area);
    let content_area = right_rail_content_area(area)?;

    let block = ratatui::widgets::Block::default()
        .borders(ratatui::widgets::Borders::LEFT)
        .border_style(border_style);
    frame.render_widget(block, area);
    frame.render_widget(
        Paragraph::new(title),
        Rect {
            x: inner.x,
            y: inner.y,
            width: inner.width,
            height: RIGHT_RAIL_HEADER_HEIGHT,
        },
    );

    Some(content_area)
}

/// Set alignment on a line only if it doesn't already have one set.
/// This allows markdown rendering to mark code blocks as left-aligned while
/// other content inherits the default alignment (e.g., centered mode).
pub(crate) fn align_if_unset(line: Line<'static>, align: Alignment) -> Line<'static> {
    if line.alignment.is_some() {
        line
    } else {
        line.alignment(align)
    }
}
