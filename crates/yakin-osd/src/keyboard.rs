//! The on-screen keyboard as an SVG: a translucent panel docked at the bottom of the screen, its
//! halves split where the trackpads meet, the key under each finger lit in its own colour.

use yakin_ipc::IpcKeyboard;

use crate::draw::FONT;
use crate::draw::escape;

/// Share of the screen's height the keyboard's surface takes, at most.
const HEIGHT_SHARE: f32 = 0.4;

/// Share of each side edge left empty, for TVs that overscan.
const SAFE: f32 = 0.05;

/// Space under the panel, logical pixels.
const BOTTOM: f32 = 12.0;

/// Panel padding around the keys.
const PAD: f32 = 10.0;

const GAP: f32 = 4.0;

/// Extra gap between the left and right halves.
const SPLIT: f32 = 24.0;

const COLUMNS: u32 = 10;

const RING: f32 = 3.0;

/// Average glyph width of the font, as a share of its size.
const GLYPH_WIDTH: f32 = 0.62;

/// The left finger's key, the right finger's, the D-pad's focus.
const LEFT: &str = "#3b82f6";
const RIGHT: &str = "#f59e0b";
const FOCUS: &str = "#ffffff";

/// Logical height of the keyboard's surface on a screen `height` tall.
pub fn keyboard_height(height: u32) -> u32 {
    (height as f32 * HEIGHT_SHARE).round() as u32
}

/// The keyboard over a `width` by `height` logical surface.
pub fn keyboard_svg(
    keyboard: &IpcKeyboard,
    (width, height): (u32, u32),
) -> String {
    let (w, h) = (width as f32, height as f32);
    let left = w * SAFE;
    let panel_w = w - 2.0 * left;
    let panel_h = h - BOTTOM;
    let rows = keyboard.rows.len().max(1) as f32;
    let key_w = (panel_w - 2.0 * PAD - (COLUMNS - 1) as f32 * GAP - SPLIT) / COLUMNS as f32;
    let key_h = (panel_h - 2.0 * PAD - (rows - 1.0) * GAP) / rows;
    let column_x = |column: u32| {
        let split = match column >= COLUMNS / 2 {
            true => SPLIT,
            false => 0.0,
        };
        left + PAD + column as f32 * (key_w + GAP) + split
    };
    let mut keys = String::new();
    let mut cell = 0;
    for (row, caps) in keyboard.rows.iter().enumerate() {
        let y = PAD + row as f32 * (key_h + GAP);
        let mut column = 0;
        for cap in caps {
            let x = column_x(column);
            let end = column + cap.span.max(1);
            let width = column_x(end - 1) + key_w - x;
            let latched = match cap.label.as_str() {
                "Shift" => keyboard.shift || keyboard.caps,
                _ => false,
            };
            let rings: Vec<&str> = [
                (keyboard.hover_left == Some(cell)).then_some(LEFT),
                (keyboard.hover_right == Some(cell)).then_some(RIGHT),
                (keyboard.focus == cell).then_some(FOCUS),
            ]
            .into_iter()
            .flatten()
            .collect();
            keys.push_str(&key_svg(
                &cap.label,
                (x, y),
                (width, key_h),
                latched,
                &rings,
            ));
            column = end;
            cell += 1;
        }
    }
    let split_x = column_x(COLUMNS / 2) - (SPLIT + GAP) / 2.0;
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" font-family="{FONT}, sans-serif"><rect x="{left}" width="{panel_w}" height="{panel_h}" rx="20" fill="#12151e" fill-opacity="0.85"/><line class="split" x1="{split_x}" y1="{y1}" x2="{split_x}" y2="{y2}" stroke="#ffffff" stroke-opacity="0.15" stroke-width="2" stroke-linecap="round"/>{keys}</svg>"##,
        y1 = PAD * 2.0,
        y2 = panel_h - PAD * 2.0,
    )
}

/// One key at `(x, y)`; a finger on it fills it with that finger's colour, the focus rings it.
fn key_svg(
    label: &str,
    (x, y): (f32, f32),
    (width, height): (f32, f32),
    latched: bool,
    rings: &[&str],
) -> String {
    let fill = match (rings.first(), latched) {
        (Some(&LEFT), _) => LEFT,
        (Some(&RIGHT), _) => RIGHT,
        (_, true) => "#34406a",
        _ => "#232838",
    };
    // A key under both fingers shows the right one's colour as a ring
    let ring = rings
        .iter()
        .skip(match fill {
            LEFT | RIGHT => 1,
            _ => 0,
        })
        .enumerate()
        .map(|(index, colour)| {
            let inset = RING / 2.0 + index as f32 * RING;
            format!(
                r##"<rect class="ring" x="{inset}" y="{inset}" width="{w}" height="{h}" rx="8" fill="none" stroke="{colour}" stroke-width="{RING}"/>"##,
                w = width - 2.0 * inset,
                h = height - 2.0 * inset,
            )
        })
        .collect::<String>();
    let chars = label.chars().count().max(1) as f32;
    let font = (height * 0.5).min(width * 0.8 / (chars * GLYPH_WIDTH));
    format!(
        r##"<g class="key" transform="translate({x} {y})"><rect width="{width}" height="{height}" rx="8" fill="{fill}"/>{ring}<text x="{cx}" y="{cy}" font-size="{font}" fill="#ffffff" text-anchor="middle" dominant-baseline="central">{label}</text></g>"##,
        cx = width / 2.0,
        cy = height / 2.0,
        label = escape(label),
    )
}

#[cfg(test)]
mod test {
    use yakin_core::keyboard::Keyboard;

    use super::*;

    fn demo() -> IpcKeyboard {
        let keyboard = Keyboard {
            hover_left: Some(12),
            hover_right: Some(17),
            focus: 43,
            ..Keyboard::default()
        };
        IpcKeyboard::from(&keyboard)
    }

    #[test]
    fn test_keyboard_svg_keys_and_highlights() {
        let screen = (960, 540);
        let height = keyboard_height(screen.1);
        assert!(height as f32 <= screen.1 as f32 * 0.4);
        let svg = keyboard_svg(&demo(), (screen.0, height));
        assert_eq!(svg.matches(r#"class="key""#).count(), 46);
        assert_eq!(svg.matches(r#"class="split""#).count(), 1);
        assert_eq!(svg.matches(&format!(r#"fill="{LEFT}""#)).count(), 1);
        assert_eq!(svg.matches(&format!(r#"fill="{RIGHT}""#)).count(), 1);
        assert_eq!(svg.matches(r#"class="ring""#).count(), 1);
        assert!(svg.contains(">Shift<") && svg.contains(">123<"));
        resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    }

    #[test]
    fn test_keys_read_at_ten_feet() {
        // 1080p at scale 2: keys at least 64 px, 32 logical, tall
        let svg = keyboard_svg(&demo(), (960, keyboard_height(540)));
        let height: f32 = svg
            .split(r#"<g class="key""#)
            .nth(1)
            .and_then(|key| key.split(r#"height=""#).nth(1))
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(height * 2.0 >= 64.0, "{height}");
    }
}
