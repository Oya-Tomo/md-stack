//! Math images: sizing on the cell grid, SVG rasterization and Sixel encoding.

use std::collections::HashMap;
use std::collections::hash_map::Entry;
use std::fs;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};
use image::{DynamicImage, RgbaImage};
use ratatui::layout::Size;
use ratatui_image::picker::cap_parser::QueryStdioOptions;
use ratatui_image::picker::{Capability, Picker, ProtocolType};
use ratatui_image::protocol::Protocol;
use ratatui_image::{FontSize, Resize};
use resvg::{tiny_skia, usvg};

use crate::store::MathEntry;

/// Height of one TeX `ex` relative to a terminal cell's pixel height, for inline math.
/// Chosen so that inline math matches the size of the surrounding text.
const EX_PER_CELL_HEIGHT: f32 = 0.42;
/// Display math is drawn slightly larger than inline math.
const DISPLAY_SCALE: f32 = 1.1;
/// Position of the text baseline within a cell, as a fraction of the cell height from the top.
const BASELINE_IN_CELL: f32 = 0.75;

/// Where and how large a math image is drawn.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct MathGeometry {
    pub cols: u16,
    pub rows: u16,
    /// The image row whose baseline the expression's baseline sits on; it lines up with the
    /// row holding the surrounding text.
    pub baseline_row: u16,
    pub px_per_ex: f32,
}

impl MathGeometry {
    /// Fits the expression on the cell grid, shrinking it to at most `max_cols` columns.
    // Cell counts are small and positive, and float-to-int `as` saturates.
    #[allow(clippy::cast_possible_truncation, clippy::cast_sign_loss)]
    pub fn fit(entry: &MathEntry, font: FontSize, max_cols: u16) -> Self {
        let (cell_w, cell_h) = (f32::from(font.width), f32::from(font.height));
        let natural = cell_h * EX_PER_CELL_HEIGHT * if entry.display { DISPLAY_SCALE } else { 1.0 };
        let max_width_px = f32::from(max_cols.max(1)) * cell_w;
        let px_per_ex = natural.min(max_width_px / entry.width_ex.max(f32::EPSILON));
        // Rows needed for the part of the expression that overflows the baseline row.
        let overflow_rows = |px: f32| (px / cell_h).ceil().max(0.0) as u16;
        let ascent = (entry.height_ex - entry.depth_ex) * px_per_ex;
        let descent = entry.depth_ex * px_per_ex;
        let above = overflow_rows(ascent - BASELINE_IN_CELL * cell_h);
        let below = overflow_rows(descent - (1.0 - BASELINE_IN_CELL) * cell_h);
        let cols = ((entry.width_ex * px_per_ex / cell_w).ceil() as u16).max(1);
        Self {
            cols: cols.min(max_cols.max(1)),
            rows: above + 1 + below,
            baseline_row: above,
            px_per_ex,
        }
    }
}

pub struct Graphics {
    rasterizer: Rasterizer,
    dark: bool,
    cache: HashMap<CacheKey, Protocol>,
}

#[derive(PartialEq, Eq, Hash)]
struct CacheKey {
    svg_path: PathBuf,
    cols: u16,
    rows: u16,
    px_per_ex_bits: u32,
}

impl Graphics {
    /// Queries the terminal for its cell size and background color.
    ///
    /// Must run after entering the alternate screen and before input events are read.
    pub fn query() -> Result<Self> {
        let mut picker = Picker::from_query_stdio_with_options(QueryStdioOptions {
            terminal_background_color_osc: true,
            ..Default::default()
        })
        .context("querying terminal graphics capabilities")?;
        // Sixel is the only supported protocol (docs/SPEC.md §1.2), so it is not auto-detected.
        picker.set_protocol_type(ProtocolType::Sixel);

        let (r, g, b) = picker
            .capabilities()
            .iter()
            .find_map(|c| match c {
                Capability::Background(r, g, b) => Some((*r, *g, *b)),
                _ => None,
            })
            .unwrap_or((0, 0, 0));
        let luminance = 0.2126 * f32::from(r) + 0.7152 * f32::from(g) + 0.0722 * f32::from(b);
        let dark = luminance < 128.0;
        let foreground = if dark { 230 } else { 30 };

        let mut svg_options = usvg::Options::default();
        // MathJax draws known glyphs as paths but falls back to <text> for other characters.
        svg_options.fontdb_mut().load_system_fonts();

        Ok(Self {
            rasterizer: Rasterizer {
                picker,
                foreground: tiny_skia::Color::from_rgba8(foreground, foreground, foreground, 255),
                background: tiny_skia::Color::from_rgba8(r, g, b, 255),
                svg_options,
            },
            dark,
            cache: HashMap::new(),
        })
    }

    pub fn font(&self) -> FontSize {
        self.rasterizer.picker.font_size()
    }

    pub fn is_dark(&self) -> bool {
        self.dark
    }

    pub fn clear_cache(&mut self) {
        self.cache.clear();
    }

    /// The encoded image for a math expression, rendering it on first use.
    pub fn math_image(
        &mut self,
        svg_path: &Path,
        entry: &MathEntry,
        geometry: MathGeometry,
    ) -> Result<&Protocol> {
        let key = CacheKey {
            svg_path: svg_path.to_owned(),
            cols: geometry.cols,
            rows: geometry.rows,
            px_per_ex_bits: geometry.px_per_ex.to_bits(),
        };
        match self.cache.entry(key) {
            Entry::Occupied(slot) => Ok(slot.into_mut()),
            Entry::Vacant(slot) => {
                Ok(slot.insert(self.rasterizer.render(svg_path, entry, geometry)?))
            }
        }
    }
}

struct Rasterizer {
    picker: Picker,
    foreground: tiny_skia::Color,
    background: tiny_skia::Color,
    svg_options: usvg::Options<'static>,
}

impl Rasterizer {
    /// Draws the SVG on an opaque canvas exactly covering its cells, centered horizontally and
    /// with its baseline on the baseline of [`MathGeometry::baseline_row`], and encodes it.
    fn render(
        &self,
        svg_path: &Path,
        entry: &MathEntry,
        geometry: MathGeometry,
    ) -> Result<Protocol> {
        let svg = fs::read_to_string(svg_path)
            .with_context(|| format!("reading {}", svg_path.display()))?
            .replace("currentColor", &css_color(self.foreground));
        let tree = usvg::Tree::from_str(&svg, &self.svg_options).context("parsing math SVG")?;

        let font = self.picker.font_size();
        let width = u32::from(geometry.cols) * u32::from(font.width);
        let height = u32::from(geometry.rows) * u32::from(font.height);
        let mut pixmap = tiny_skia::Pixmap::new(width, height).context("empty math image")?;
        pixmap.fill(self.background);

        let content_width = entry.width_ex * geometry.px_per_ex;
        let ascent = (entry.height_ex - entry.depth_ex) * geometry.px_per_ex;
        let baseline_y =
            (f32::from(geometry.baseline_row) + BASELINE_IN_CELL) * f32::from(font.height);
        let scale = content_width / tree.size().width();
        let transform = tiny_skia::Transform::from_row(
            scale,
            0.0,
            0.0,
            scale,
            (f32::from(geometry.cols) * f32::from(font.width) - content_width) / 2.0,
            baseline_y - ascent,
        );
        resvg::render(&tree, transform, &mut pixmap.as_mut());

        // The canvas is opaque, so premultiplied and straight alpha coincide.
        let image =
            RgbaImage::from_raw(width, height, pixmap.take()).context("converting math image")?;
        self.picker
            .new_protocol(
                DynamicImage::ImageRgba8(image),
                Size::new(geometry.cols, geometry.rows),
                Resize::Fit(None),
            )
            .context("encoding math image")
    }
}

fn css_color(color: tiny_skia::Color) -> String {
    let c = color.to_color_u8();
    format!("#{:02x}{:02x}{:02x}", c.red(), c.green(), c.blue())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(display: bool, width_ex: f32, height_ex: f32, depth_ex: f32) -> MathEntry {
        MathEntry {
            display,
            tex: String::new(),
            width_ex,
            height_ex,
            depth_ex,
        }
    }

    #[test]
    fn simple_inline_math_takes_one_row() {
        // `a \neq 0` as measured by MathJax.
        let g = MathGeometry::fit(
            &entry(false, 5.345, 2.106, 0.486),
            FontSize::new(10, 20),
            80,
        );
        assert_eq!((g.cols, g.rows, g.baseline_row), (5, 1, 0));
    }

    #[test]
    fn tall_math_extends_around_the_baseline() {
        // `\left(\frac{b}{2a}\right)^2`: rises well above and drops below the baseline.
        let g = MathGeometry::fit(&entry(false, 6.0, 5.5, 2.0), FontSize::new(10, 20), 80);
        assert_eq!((g.rows, g.baseline_row), (3, 1));
    }

    #[test]
    fn wide_math_shrinks_to_fit() {
        let g = MathGeometry::fit(&entry(true, 200.0, 5.0, 1.0), FontSize::new(10, 20), 40);
        assert_eq!(g.cols, 40);
        assert!(200.0 * g.px_per_ex <= 400.0 + 1e-3);
    }
}
