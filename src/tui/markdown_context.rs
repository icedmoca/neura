use crate::config::{DiagramDisplayMode, MarkdownSpacingMode, config};
use std::cell::Cell;
use std::sync::{LazyLock, Mutex};

static DIAGRAM_MODE_OVERRIDE: LazyLock<Mutex<Option<DiagramDisplayMode>>> =
    LazyLock::new(|| Mutex::new(None));

thread_local! {
    /// Whether markdown rendering is running in streaming mode.
    /// In this mode mermaid diagrams update an ephemeral side-panel preview
    /// instead of being persisted in ACTIVE_DIAGRAMS history.
    static STREAMING_RENDER_CONTEXT: Cell<bool> = const { Cell::new(false) };
    /// Whether code blocks should be horizontally centered within available width.
    /// Set to true in centered mode, false in left-aligned mode.
    static CENTER_CODE_BLOCKS: Cell<bool> = const { Cell::new(true) };
    /// Optional test/debug override for markdown spacing mode.
    static MARKDOWN_SPACING_MODE_OVERRIDE: Cell<Option<MarkdownSpacingMode>> = const { Cell::new(None) };
    /// Whether Mermaid cache misses should be rendered in the background and
    /// replaced on a later redraw instead of blocking the current frame.
    static DEFER_MERMAID_RENDER_CONTEXT: Cell<bool> = const { Cell::new(false) };
}

struct ScopedReset<'a, T: Copy> {
    cell: &'a Cell<T>,
    prev: T,
}

impl<T: Copy> Drop for ScopedReset<'_, T> {
    fn drop(&mut self) {
        self.cell.set(self.prev);
    }
}

fn with_scoped_cell_value<T: Copy, R>(cell: &Cell<T>, value: T, f: impl FnOnce() -> R) -> R {
    let prev = cell.replace(value);
    let _guard = ScopedReset { cell, prev };
    f()
}

pub fn set_diagram_mode_override(mode: Option<DiagramDisplayMode>) {
    if let Ok(mut override_mode) = DIAGRAM_MODE_OVERRIDE.lock() {
        *override_mode = mode;
    }
}

pub fn get_diagram_mode_override() -> Option<DiagramDisplayMode> {
    DIAGRAM_MODE_OVERRIDE.lock().ok().and_then(|mode| *mode)
}

pub(super) fn effective_diagram_mode() -> DiagramDisplayMode {
    if let Ok(mode) = DIAGRAM_MODE_OVERRIDE.lock()
        && let Some(override_mode) = *mode
    {
        return override_mode;
    }
    config().display.diagram_mode
}

pub(super) fn effective_markdown_spacing_mode() -> MarkdownSpacingMode {
    MARKDOWN_SPACING_MODE_OVERRIDE
        .with(|mode| mode.get().unwrap_or(config().display.markdown_spacing))
}

#[cfg(test)]
pub(crate) fn with_markdown_spacing_mode_override<T>(
    mode: Option<MarkdownSpacingMode>,
    f: impl FnOnce() -> T,
) -> T {
    MARKDOWN_SPACING_MODE_OVERRIDE.with(|ctx| with_scoped_cell_value(ctx, mode, f))
}

pub(super) fn with_streaming_render_context<T>(f: impl FnOnce() -> T) -> T {
    STREAMING_RENDER_CONTEXT.with(|ctx| with_scoped_cell_value(ctx, true, f))
}

pub(super) fn streaming_render_context_enabled() -> bool {
    STREAMING_RENDER_CONTEXT.with(|ctx| ctx.get())
}

pub fn with_deferred_mermaid_render_context<T>(f: impl FnOnce() -> T) -> T {
    DEFER_MERMAID_RENDER_CONTEXT.with(|ctx| with_scoped_cell_value(ctx, true, f))
}

pub(super) fn deferred_mermaid_render_context_enabled() -> bool {
    DEFER_MERMAID_RENDER_CONTEXT.with(|ctx| ctx.get())
}

pub fn set_center_code_blocks(centered: bool) {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.set(centered));
}

pub fn center_code_blocks() -> bool {
    CENTER_CODE_BLOCKS.with(|ctx| ctx.get())
}
