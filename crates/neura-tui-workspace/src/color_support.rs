use ratatui::style::Color;
use std::sync::OnceLock;

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorCapability {
    TrueColor,
    Color256,
}

static CAPABILITY: OnceLock<ColorCapability> = OnceLock::new();

pub fn color_capability() -> ColorCapability {
    *CAPABILITY.get_or_init(detect_color_capability)
}

fn detect_color_capability() -> ColorCapability {
    if let Ok(val) = std::env::var("COLORTERM") {
        let v = val.to_lowercase();
        if v == "truecolor" || v == "24bit" {
            return ColorCapability::TrueColor;
        }
    }

    if let Ok(term_program) = std::env::var("TERM_PROGRAM") {
        let tp = term_program.to_lowercase();
        if tp == "ghostty"
            || tp == "iterm.app"
            || tp == "wezterm"
            || tp == "warp"
            || tp == "alacritty"
            || tp == "hyper"
        {
            return ColorCapability::TrueColor;
        }
    }

    if std::env::var("GHOSTTY_RESOURCES_DIR").is_ok()
        || std::env::var("GHOSTTY_BIN_DIR").is_ok()
        || std::env::var("WEZTERM_EXECUTABLE").is_ok()
        || std::env::var("WEZTERM_PANE").is_ok()
    {
        return ColorCapability::TrueColor;
    }

    if let Ok(term) = std::env::var("TERM") {
        let t = term.to_lowercase();
        if t.contains("kitty") || t.contains("ghostty") || t.contains("alacritty") {
            return ColorCapability::TrueColor;
        }
        if t.contains("256color") {
            return ColorCapability::Color256;
        }
    }

    ColorCapability::Color256
}

pub fn has_truecolor() -> bool {
    color_capability() == ColorCapability::TrueColor
}

pub fn clear_buf(area: Rect, buf: &mut Buffer) {
    for x in area.left()..area.right() {
        for y in area.top()..area.bottom() {
            buf[(x, y)].reset();
        }
    }
}

#[inline]
pub fn rgb(r: u8, g: u8, b: u8) -> Color {
    if has_truecolor() {
        Color::Rgb(r, g, b)
    } else {
        Color::Indexed(rgb_to_xterm256(r, g, b))
    }
}

// The xterm-256 color cube: indices 16-231 map to a 6x6x6 RGB cube.
// Each axis uses values: 0, 95, 135, 175, 215, 255 (indices 0-5).
// Indices 232-255 are a grayscale ramp from rgb(8,8,8) to rgb(238,238,238).
fn rgb_to_xterm256(r: u8, g: u8, b: u8) -> u8 {
    let gray_avg = (r as u16 + g as u16 + b as u16) / 3;
    let is_grayish = (r as i16 - g as i16).unsigned_abs() < 15
        && (g as i16 - b as i16).unsigned_abs() < 15
        && (r as i16 - b as i16).unsigned_abs() < 15;

    let cube_idx = nearest_cube_index(r, g, b);
    let cube_color = cube_index_to_rgb(cube_idx);
    let cube_dist = color_distance(r, g, b, cube_color.0, cube_color.1, cube_color.2);

    if is_grayish {
        let gray_idx = nearest_gray_index(gray_avg as u8);
        let gray_val = gray_index_to_value(gray_idx);
        let gray_dist = color_distance(r, g, b, gray_val, gray_val, gray_val);

        if gray_dist < cube_dist {
            return 232 + gray_idx;
        }
    }

    cube_idx as u8 + 16
}

const CUBE_VALUES: [u8; 6] = [0, 95, 135, 175, 215, 255];

fn nearest_cube_component(v: u8) -> u8 {
    let mut best = 0u8;
    let mut best_dist = 255u16;
    for (i, &cv) in CUBE_VALUES.iter().enumerate() {
        let d = (v as i16 - cv as i16).unsigned_abs();
        if d < best_dist {
            best_dist = d;
            best = i as u8;
        }
    }
    best
}

fn nearest_cube_index(r: u8, g: u8, b: u8) -> u16 {
    let ri = nearest_cube_component(r) as u16;
    let gi = nearest_cube_component(g) as u16;
    let bi = nearest_cube_component(b) as u16;
    ri * 36 + gi * 6 + bi
}

fn cube_index_to_rgb(idx: u16) -> (u8, u8, u8) {
    let bi = (idx % 6) as usize;
    let gi = ((idx / 6) % 6) as usize;
    let ri = (idx / 36) as usize;
    (CUBE_VALUES[ri], CUBE_VALUES[gi], CUBE_VALUES[bi])
}

fn nearest_gray_index(v: u8) -> u8 {
    // Grayscale ramp: 232-255, values 8, 18, 28, ..., 238 (24 steps, step=10)
    if v < 4 {
        return 0;
    }
    if v > 243 {
        return 23;
    }
    ((v as u16 - 8 + 5) / 10).min(23) as u8
}

fn gray_index_to_value(idx: u8) -> u8 {
    8 + idx * 10
}

fn color_distance(r1: u8, g1: u8, b1: u8, r2: u8, g2: u8, b2: u8) -> u32 {
    let dr = r1 as i32 - r2 as i32;
    let dg = g1 as i32 - g2 as i32;
    let db = b1 as i32 - b2 as i32;
    // Weighted Euclidean - human eye is more sensitive to green
    (2 * dr * dr + 4 * dg * dg + 3 * db * db) as u32
}

pub fn indexed_to_rgb(idx: u8) -> (u8, u8, u8) {
    if idx >= 232 {
        let v = gray_index_to_value(idx - 232);
        (v, v, v)
    } else if idx >= 16 {
        cube_index_to_rgb((idx - 16) as u16)
    } else {
        match idx {
            0 => (0, 0, 0),
            1 => (128, 0, 0),
            2 => (0, 128, 0),
            3 => (128, 128, 0),
            4 => (0, 0, 128),
            5 => (128, 0, 128),
            6 => (0, 128, 128),
            7 => (192, 192, 192),
            8 => (128, 128, 128),
            9 => (255, 0, 0),
            10 => (0, 255, 0),
            11 => (255, 255, 0),
            12 => (0, 0, 255),
            13 => (255, 0, 255),
            14 => (0, 255, 255),
            _ => (255, 255, 255),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_pure_black() {
        let idx = rgb_to_xterm256(0, 0, 0);
        assert_eq!(idx, 16); // cube index 0,0,0
    }

    #[test]
    fn test_pure_white() {
        let idx = rgb_to_xterm256(255, 255, 255);
        assert_eq!(idx, 231); // cube index 5,5,5
    }

    #[test]
    fn test_mid_gray() {
        let idx = rgb_to_xterm256(128, 128, 128);
        // Should pick grayscale 243 (value 128) or nearby
        assert!(
            (232..=255).contains(&u16::from(idx)),
            "Expected grayscale, got {}",
            idx
        );
    }

    #[test]
    fn test_dim_gray() {
        let idx = rgb_to_xterm256(80, 80, 80);
        assert!(
            (232..=255).contains(&u16::from(idx)),
            "Expected grayscale for dim, got {}",
            idx
        );
    }

    #[test]
    fn test_red() {
        let idx = rgb_to_xterm256(255, 0, 0);
        assert_eq!(idx, 196); // cube 5,0,0
    }

    #[test]
    fn test_green() {
        let idx = rgb_to_xterm256(0, 255, 0);
        assert_eq!(idx, 46); // cube 0,5,0
    }

    #[test]
    fn test_blue() {
        let idx = rgb_to_xterm256(0, 0, 255);
        assert_eq!(idx, 21); // cube 0,0,5
    }

    #[test]
    fn test_rgb_truecolor() {
        // When we have truecolor, rgb() should return Color::Rgb
        // (can't easily test since it depends on env, but test the mapper)
        let color = Color::Indexed(rgb_to_xterm256(138, 180, 248));
        match color {
            Color::Indexed(n) => assert!(n >= 16, "Should be extended color"),
            _ => panic!("Expected indexed color"),
        }
    }

    #[test]
    fn test_near_colors_are_stable() {
        let a = rgb_to_xterm256(80, 80, 80);
        let b = rgb_to_xterm256(82, 82, 82);
        assert_eq!(a, b, "Similar grays should map to same index");
    }
}
