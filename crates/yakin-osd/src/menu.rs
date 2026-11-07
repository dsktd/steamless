//! The quick menu as an SVG: a dimmed screen and a centred grid of tiles, sized to read from
//! across the room on a TV.

use yakin_ipc::IpcMenu;

use crate::draw::FONT;
use crate::draw::escape;

/// Share of each screen edge left empty, for TVs that overscan.
const SAFE: f32 = 0.05;

/// Largest and smallest tile side, logical pixels.
const TILE_MAX: f32 = 160.0;
const TILE_MIN: f32 = 120.0;

const GAP: f32 = 16.0;

/// Panel padding around the grid.
const PAD: f32 = 24.0;

/// The focused tile grows by this, around its centre.
const FOCUS_SCALE: f32 = 1.08;

const RING: f32 = 4.0;

/// Smallest label size, logical pixels; longer labels shrink to fit, down to this.
const LABEL_MIN: f32 = 18.0;
const LABEL_MAX: f32 = 22.0;

/// Average glyph width of the font, as a share of its size.
const GLYPH_WIDTH: f32 = 0.62;

/// Glyphs on a 24 px grid, stroked: hand-written paths, one per [`yakin_core::menu::ICONS`].
const GLYPHS: [(&str, &str); 14] = [
    (
        "apps",
        "M4 4h3v3H4z M10.5 4h3v3h-3z M17 4h3v3h-3z M4 10.5h3v3H4z M10.5 10.5h3v3h-3z \
         M17 10.5h3v3h-3z M4 17h3v3H4z M10.5 17h3v3h-3z M17 17h3v3h-3z",
    ),
    (
        "keyboard",
        "M2 6h20v12H2z M6 10h.01 M10 10h.01 M14 10h.01 M18 10h.01 M7 14h10",
    ),
    (
        "overview",
        "M2 5h9v6H2z M13 5h9v6h-9z M2 13h9v6H2z M13 13h9v6h-9z",
    ),
    ("fullscreen", "M4 9V4h5 M15 4h5v5 M20 15v5h-5 M9 20H4v-5"),
    // A column stretched to both edges: arrows out to the sides
    (
        "maximize",
        "M4 4v16 M20 4v16 M8 12h8 M8 12l3-3 M8 12l3 3 M16 12l-3-3 M16 12l-3 3",
    ),
    ("close", "M6 6l12 12 M18 6L6 18"),
    (
        "volume-up",
        "M3 9h4l5-4v14l-5-4H3z M16 9a4 4 0 0 1 0 6 M19 6a8 8 0 0 1 0 12",
    ),
    ("volume-down", "M3 9h4l5-4v14l-5-4H3z M16 9a4 4 0 0 1 0 6"),
    ("mute", "M3 9h4l5-4v14l-5-4H3z M16 9l5 6 M21 9l-5 6"),
    (
        "gamepad",
        "M6 7h12a4 4 0 0 1 4 4v3a3 3 0 0 1-5.5 1.7L15 14H9l-1.5 1.7A3 3 0 0 1 2 14v-3a4 4 0 0 1 4-4z \
         M7 9.5v4 M5 11.5h4 M16 10.5h.01 M18 12.5h.01",
    ),
    ("mouse", "M6 9a6 6 0 0 1 12 0v6a6 6 0 0 1-12 0z M12 3v6"),
    (
        "screenshot",
        "M3 8h4l2-3h6l2 3h4v11H3z M15 13a3 3 0 1 1-6 0a3 3 0 1 1 6 0",
    ),
    ("display-off", "M3 4h18v12H3z M8 20h8 M12 16v4 M2 2l20 20"),
    ("power", "M12 3v9 M7 6.3a7 7 0 1 0 10 0"),
];

/// The menu over a `width` by `height` logical screen.
pub fn menu_svg(
    menu: &IpcMenu,
    (width, height): (u32, u32),
) -> String {
    let (w, h) = (width as f32, height as f32);
    let count = menu.items.len().max(1);
    let columns = (menu.columns as usize).clamp(1, count);
    let rows = count.div_ceil(columns);
    let fit = |space: f32, cells: usize| {
        (space * (1.0 - 2.0 * SAFE) - 2.0 * PAD - (cells - 1) as f32 * GAP) / cells as f32
    };
    // Below the minimum on a small screen, the grid shrinks rather than overflowing
    let tile = fit(w, columns)
        .min(fit(h, rows))
        .min(TILE_MAX)
        .max(TILE_MIN.min(fit(h, rows)));
    let panel_w = columns as f32 * tile + (columns - 1) as f32 * GAP + 2.0 * PAD;
    let panel_h = rows as f32 * tile + (rows - 1) as f32 * GAP + 2.0 * PAD;
    let (left, top) = ((w - panel_w) / 2.0, (h - panel_h) / 2.0);
    let mut tiles = String::new();
    let focus = menu.focus as usize;
    // The focused tile last, so its ring sits above its neighbours
    let order = (0..menu.items.len())
        .filter(|index| *index != focus)
        .chain((focus < menu.items.len()).then_some(focus));
    for index in order {
        let item = &menu.items[index];
        let x = left + PAD + (index % columns) as f32 * (tile + GAP);
        let y = top + PAD + (index / columns) as f32 * (tile + GAP);
        tiles.push_str(&tile_svg(
            &item.label,
            &item.icon,
            (x, y),
            tile,
            index == focus,
        ));
    }
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{width}" height="{height}" font-family="{FONT}, sans-serif"><rect width="{width}" height="{height}" fill="#000000" fill-opacity="0.55"/><rect x="{left}" y="{top}" width="{panel_w}" height="{panel_h}" rx="28" fill="#12151e" fill-opacity="0.95"/>{tiles}</svg>"##
    )
}

/// One square tile at `(x, y)`; focused, it is scaled up and ringed.
fn tile_svg(
    label: &str,
    icon: &str,
    (x, y): (f32, f32),
    size: f32,
    focused: bool,
) -> String {
    let glyph = GLYPHS
        .iter()
        .find(|(name, _)| *name == icon)
        .map_or("", |(_, path)| path);
    let icon_size = size * 0.42;
    let icon_scale = icon_size / 24.0;
    let icon_x = (size - icon_size) / 2.0;
    let icon_y = size * 0.14;
    let chars = label.chars().count().max(1) as f32;
    let font = (size * 0.9 / (chars * GLYPH_WIDTH)).clamp(LABEL_MIN, LABEL_MAX);
    let (fill, ring) = match focused {
        true => (
            "#34406a",
            format!(
                r##"<rect class="ring" x="{o}" y="{o}" width="{s}" height="{s}" rx="20" fill="none" stroke="#ffffff" stroke-width="{RING}"/>"##,
                o = -RING / 2.0,
                s = size + RING,
            ),
        ),
        false => ("#232838", String::new()),
    };
    let grow = match focused {
        true => format!(
            " translate({c} {c}) scale({FOCUS_SCALE}) translate({m} {m})",
            c = size / 2.0,
            m = -size / 2.0
        ),
        false => String::new(),
    };
    format!(
        r##"<g class="tile" transform="translate({x} {y}){grow}"><rect width="{size}" height="{size}" rx="18" fill="{fill}"/>{ring}<path transform="translate({icon_x} {icon_y}) scale({icon_scale})" d="{glyph}" fill="none" stroke="#ffffff" stroke-width="2" stroke-linecap="round" stroke-linejoin="round"/><text x="{half}" y="{label_y}" font-size="{font}" fill="#ffffff" text-anchor="middle">{label}</text></g>"##,
        half = size / 2.0,
        label_y = size * 0.84,
        label = escape(label),
    )
}

#[cfg(test)]
mod test {
    use yakin_core::menu::ICONS;
    use yakin_core::menu::Menu;

    use super::*;

    fn default_menu() -> IpcMenu {
        IpcMenu::from(&Menu::default())
    }

    #[test]
    fn test_every_icon_has_a_glyph() {
        for icon in ICONS {
            assert!(GLYPHS.iter().any(|(name, _)| *name == icon), "{icon}");
        }
        assert_eq!(GLYPHS.len(), ICONS.len());
    }

    #[test]
    fn test_menu_svg_has_tiles_and_one_ring() {
        let mut menu = default_menu();
        menu.focus = 5;
        let svg = menu_svg(&menu, (960, 540));
        assert_eq!(svg.matches(r#"class="tile""#).count(), 12);
        assert_eq!(svg.matches(r#"class="ring""#).count(), 1);
        // The focused tile is drawn last, ringed and scaled
        let last = svg.rsplit(r#"class="tile""#).next().unwrap();
        assert!(last.contains(r#"class="ring""#) && last.contains("scale(1.08)"));
        assert!(last.contains(">Close<"));
        resvg::usvg::Tree::from_str(&svg, &resvg::usvg::Options::default()).unwrap();
    }

    #[test]
    fn test_tiles_read_at_ten_feet() {
        // 1080p at scale 2: tiles of at least 120 px inside the 5 % safe area
        let svg = menu_svg(&default_menu(), (960, 540));
        let width: f32 = svg
            .split(r#"<rect width=""#)
            .nth(2)
            .and_then(|rest| rest.split('"').next())
            .unwrap()
            .parse()
            .unwrap();
        assert!(width >= TILE_MIN, "{width}");
    }
}
