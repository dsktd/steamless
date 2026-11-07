//! A toast as an SVG panel, rendered by resvg and copied into a wl_shm buffer.

use std::sync::Arc;

use resvg::tiny_skia::Pixmap;
use resvg::tiny_skia::Transform;
use resvg::usvg::Options;
use resvg::usvg::Tree;
use resvg::usvg::fontdb::Database;

use crate::toast::Toast;

/// Logical size of the panel; the buffer is this times the output's scale.
pub const WIDTH: u32 = 420;
pub const HEIGHT: u32 = 88;

const ICON: &str = include_str!("../../../ui/assets/icon.svg");

pub const FONT: &str = "DejaVu Sans";

pub struct Painter {
    options: Options<'static>,
}

impl Painter {
    /// System fonts as fontconfig lists them (FONTCONFIG_FILE is honoured).
    pub fn new() -> Self {
        let mut fontdb = Database::new();
        fontdb.load_system_fonts();
        fontdb.set_sans_serif_family(FONT);
        let options = Options {
            fontdb: Arc::new(fontdb),
            ..Options::default()
        };
        Self { options }
    }

    pub fn render(
        &self,
        toast: &Toast,
        scale: u32,
    ) -> anyhow::Result<Pixmap> {
        self.render_svg(&svg(toast), (WIDTH, HEIGHT), scale)
    }

    /// `svg`, `size` logical pixels, at `scale`.
    pub fn render_svg(
        &self,
        svg: &str,
        (width, height): (u32, u32),
        scale: u32,
    ) -> anyhow::Result<Pixmap> {
        let tree = Tree::from_str(svg, &self.options)?;
        let mut pixmap =
            Pixmap::new(width * scale, height * scale).ok_or(anyhow::anyhow!("Empty pixmap"))?;
        let scale = scale as f32;
        resvg::render(
            &tree,
            Transform::from_scale(scale, scale),
            &mut pixmap.as_mut(),
        );
        Ok(pixmap)
    }
}

fn svg(toast: &Toast) -> String {
    let icon = ICON.trim().replacen(
        r#"width="512" height="512""#,
        r#"x="16" y="16" width="56" height="56""#,
        1,
    );
    let title = escape(&toast.title);
    let text = match &toast.subtitle {
        Some(subtitle) => format!(
            r##"<text x="88" y="42" font-size="22" fill="#ffffff">{title}</text><text x="88" y="68" font-size="16" fill="#a0a6b8">{}</text>"##,
            escape(subtitle)
        ),
        None => format!(r##"<text x="88" y="52" font-size="22" fill="#ffffff">{title}</text>"##),
    };
    format!(
        r##"<svg xmlns="http://www.w3.org/2000/svg" width="{WIDTH}" height="{HEIGHT}" font-family="{FONT}, sans-serif"><rect width="{WIDTH}" height="{HEIGHT}" rx="20" fill="#12151e" fill-opacity="0.92"/>{icon}{text}</svg>"##
    )
}

pub fn escape(text: &str) -> String {
    text.replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
}

/// Premultiplied RGBA to wl_shm ARGB8888 (little endian: B, G, R, A in memory), faded by `alpha`.
pub fn copy(
    pixmap: &Pixmap,
    canvas: &mut [u8],
    alpha: f32,
) {
    let alpha = (alpha.clamp(0.0, 1.0) * 256.0) as u32;
    let (src, _) = pixmap.data().as_chunks::<4>();
    let (dst, _) = canvas.as_chunks_mut::<4>();
    for (src, dst) in src.iter().zip(dst) {
        let fade = |channel: u8| ((channel as u32 * alpha) >> 8) as u8;
        dst[0] = fade(src[2]);
        dst[1] = fade(src[1]);
        dst[2] = fade(src[0]);
        dst[3] = fade(src[3]);
    }
}

#[cfg(test)]
mod test {
    use super::*;

    #[test]
    fn test_svg_parses() {
        let toast = Toast::new("A & <B>", Some("profile"));
        let painter = Painter {
            options: Options::default(),
        };
        let pixmap = painter.render(&toast, 2).unwrap();
        assert_eq!(pixmap.width(), WIDTH * 2);
        // the panel's corner is transparent, its middle is not
        assert_eq!(pixmap.pixel(0, 0).unwrap().alpha(), 0);
        assert!(pixmap.pixel(WIDTH, HEIGHT).unwrap().alpha() > 200);
    }

    #[test]
    fn test_copy_swaps_and_fades() {
        let mut pixmap = Pixmap::new(1, 1).unwrap();
        pixmap.data_mut().copy_from_slice(&[10, 20, 30, 255]);
        let mut canvas = [0_u8; 4];
        copy(&pixmap, &mut canvas, 1.0);
        assert_eq!(canvas, [30, 20, 10, 255]);
        copy(&pixmap, &mut canvas, 0.5);
        assert_eq!(canvas, [15, 10, 5, 127]);
    }
}
