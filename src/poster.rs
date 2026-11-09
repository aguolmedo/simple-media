use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use anyhow::Result;
use image::RgbImage;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::widgets::Widget;

use crate::blocks::{Blocks, Cell, OCTANTS, SEXTANTS};
use crate::http;
use crate::storage::cache_dir;

const BACKDROP_BRIGHTNESS: f32 = 0.3;
const BACKDROP_MAX_WIDTH: u32 = 480;

pub type Picture = Arc<Image>;

#[derive(Clone, Copy, PartialEq)]
pub enum Art {
    Poster,
    Backdrop,
}

impl Art {
    fn url(self, title_id: &str) -> String {
        let kind = match self {
            Art::Poster => "poster",
            Art::Backdrop => "background",
        };
        format!("https://images.metahub.space/{kind}/medium/{title_id}/img")
    }

    fn blocks(self) -> &'static Blocks {
        match self {
            Art::Poster => &OCTANTS,
            Art::Backdrop => &SEXTANTS,
        }
    }

    fn cache(self) -> PathBuf {
        let folder = match self {
            Art::Poster => "posters-medium",
            Art::Backdrop => "backdrops-medium",
        };
        cache_dir().join(folder)
    }
}

pub struct Image {
    art: Art,
    pixels: RgbImage,
    mosaic: Mutex<Option<(u16, u16, Vec<Cell>)>>,
}

impl Image {
    pub fn width_for(&self, height: u16) -> u16 {
        let pixels_tall = f64::from(height) * 2.0;
        (pixels_tall * f64::from(self.pixels.width()) / f64::from(self.pixels.height())).round() as u16
    }

    fn cells(&self, width: u16, height: u16) -> Vec<Cell> {
        let mut mosaic = self.mosaic.lock().unwrap();
        match mosaic.as_ref() {
            Some((w, h, cells)) if (*w, *h) == (width, height) => cells.clone(),
            _ => {
                let blocks = self.art.blocks();
                let cells = match self.art {
                    Art::Poster => blocks.cells(&self.pixels, width, height),
                    Art::Backdrop => blocks.cells(&darken(cover(&self.pixels, width, height)), width, height),
                };
                *mosaic = Some((width, height, cells.clone()));
                cells
            }
        }
    }
}

pub async fn fetch(art: Art, title_id: &str) -> Result<Picture> {
    let path = art.cache().join(title_id);
    let bytes = match std::fs::read(&path) {
        Ok(bytes) => bytes,
        Err(_) => {
            let bytes = http::get_bytes(&art.url(title_id)).await?;
            let _ = std::fs::create_dir_all(art.cache());
            let _ = std::fs::write(&path, &bytes);
            bytes
        }
    };
    let decoded = image::load_from_memory(&bytes)?;
    let pixels = match art {
        Art::Poster => decoded.to_rgb8(),
        Art::Backdrop => decoded.thumbnail(BACKDROP_MAX_WIDTH, BACKDROP_MAX_WIDTH).to_rgb8(),
    };
    Ok(Arc::new(Image { art, pixels, mosaic: Mutex::new(None) }))
}

fn cover(image: &RgbImage, width: u16, height: u16) -> RgbImage {
    let target = f64::from(width) / (f64::from(height) * 2.0);
    let (w, h) = (f64::from(image.width()), f64::from(image.height()));
    let (crop_w, crop_h) = if w / h > target { (h * target, h) } else { (w, w / target) };
    let (x, y) = ((w - crop_w) / 2.0, (h - crop_h) / 2.0);
    image::imageops::crop_imm(image, x as u32, y as u32, crop_w as u32, crop_h as u32).to_image()
}

pub struct Mosaic<'a>(pub &'a Image);

impl Widget for Mosaic<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let cells = self.0.cells(area.width, area.height);
        for (i, (symbol, fg, bg)) in cells.into_iter().enumerate() {
            let (column, row) = (i as u16 % area.width, i as u16 / area.width);
            buf[(area.x + column, area.y + row)].set_char(symbol).set_fg(fg).set_bg(bg);
        }
    }
}

pub struct Backdrop<'a>(pub &'a Image);

impl Widget for Backdrop<'_> {
    fn render(self, area: Rect, buf: &mut Buffer) {
        if area.is_empty() {
            return;
        }
        let cells = self.0.cells(area.width, area.height);
        for (i, (symbol, fg, bg)) in cells.into_iter().enumerate() {
            let (x, y) = (area.x + i as u16 % area.width, area.y + i as u16 / area.width);
            let cell = &mut buf[(x, y)];
            if cell.bg != Color::Reset {
                continue;
            }
            match (cell.symbol() == " ", corner(area, x, y)) {
                (true, Some(rounded)) => cell.set_char(rounded).set_fg(blend(fg, bg)),
                (true, None) => cell.set_char(symbol).set_fg(fg).set_bg(bg),
                (false, _) => cell.set_bg(blend(fg, bg)),
            };
        }
    }
}

fn corner(area: Rect, x: u16, y: u16) -> Option<char> {
    let (left, top) = (x == area.left(), y == area.top());
    let (right, bottom) = (x + 1 == area.right(), y + 1 == area.bottom());
    match (left, right, top, bottom) {
        (true, _, true, _) => Some('▟'),
        (_, true, true, _) => Some('▙'),
        (true, _, _, true) => Some('▜'),
        (_, true, _, true) => Some('▛'),
        _ => None,
    }
}

fn darken(mut image: RgbImage) -> RgbImage {
    for pixel in image.pixels_mut() {
        pixel.0 = pixel.0.map(|c| (f32::from(c) * BACKDROP_BRIGHTNESS) as u8);
    }
    image
}

fn blend(a: Color, b: Color) -> Color {
    match (a, b) {
        (Color::Rgb(r1, g1, b1), Color::Rgb(r2, g2, b2)) => {
            let mid = |x: u8, y: u8| ((u16::from(x) + u16::from(y)) / 2) as u8;
            Color::Rgb(mid(r1, r2), mid(g1, g2), mid(b1, b2))
        }
        _ => b,
    }
}
