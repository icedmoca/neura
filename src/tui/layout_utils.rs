use super::visual_debug::RectCapture;
use ratatui::layout::Rect;

pub(crate) fn rect_from_capture(rect: RectCapture) -> Rect {
    Rect {
        x: rect.x,
        y: rect.y,
        width: rect.width,
        height: rect.height,
    }
}

pub(crate) fn rect_contains(outer: Rect, inner: Rect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x.saturating_add(inner.width) <= outer.x.saturating_add(outer.width)
        && inner.y.saturating_add(inner.height) <= outer.y.saturating_add(outer.height)
}

pub(crate) fn point_in_rect(col: u16, row: u16, rect: Rect) -> bool {
    col >= rect.x
        && row >= rect.y
        && col < rect.x.saturating_add(rect.width)
        && row < rect.y.saturating_add(rect.height)
}

pub(crate) fn parse_area_spec(spec: &str) -> Option<Rect> {
    let mut parts = spec.split('+');
    let size = parts.next()?;
    let x = parts.next()?;
    let y = parts.next()?;
    if parts.next().is_some() {
        return None;
    }
    let (w, h) = size.split_once('x')?;
    Some(Rect {
        width: w.parse::<u16>().ok()?,
        height: h.parse::<u16>().ok()?,
        x: x.parse::<u16>().ok()?,
        y: y.parse::<u16>().ok()?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rect_from_capture_copies_all_fields() {
        let rect = rect_from_capture(RectCapture {
            x: 3,
            y: 5,
            width: 8,
            height: 13,
        });

        assert_eq!(rect, Rect::new(3, 5, 8, 13));
    }

    #[test]
    fn rect_contains_requires_full_containment() {
        let outer = Rect::new(2, 2, 10, 10);

        assert!(rect_contains(outer, Rect::new(4, 4, 2, 2)));
        assert!(rect_contains(outer, Rect::new(2, 2, 10, 10)));
        assert!(!rect_contains(outer, Rect::new(1, 2, 10, 10)));
        assert!(!rect_contains(outer, Rect::new(2, 2, 11, 10)));
    }

    #[test]
    fn point_in_rect_uses_half_open_bounds() {
        let rect = Rect::new(10, 20, 5, 4);

        assert!(point_in_rect(10, 20, rect));
        assert!(point_in_rect(14, 23, rect));
        assert!(!point_in_rect(15, 23, rect));
        assert!(!point_in_rect(14, 24, rect));
    }

    #[test]
    fn parse_area_spec_parses_geometry() {
        assert_eq!(parse_area_spec("80x24+4+2"), Some(Rect::new(4, 2, 80, 24)));
        assert_eq!(parse_area_spec("bad"), None);
        assert_eq!(parse_area_spec("80x24+4"), None);
    }
}
