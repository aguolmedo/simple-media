use image::RgbImage;
use image::imageops::FilterType;
use ratatui::style::Color;

pub type Cell = (char, Color, Color);

type Pixel = [u8; 3];

pub struct Blocks {
    columns: u32,
    rows: u32,
    glyph: fn(u8) -> char,
}

pub const SEXTANTS: Blocks = Blocks { columns: 2, rows: 3, glyph: sextant };
pub const OCTANTS: Blocks = Blocks { columns: 2, rows: 4, glyph: octant };

impl Blocks {
    pub fn cells(&self, image: &RgbImage, width: u16, height: u16) -> Vec<Cell> {
        let (pixels_wide, pixels_tall) = (u32::from(width) * self.columns, u32::from(height) * self.rows);
        let small = image::imageops::resize(image, pixels_wide, pixels_tall, FilterType::Lanczos3);
        let cell = |column: u16, row: u16| {
            let (x, y) = (u32::from(column) * self.columns, u32::from(row) * self.rows);
            let pixels: Vec<Pixel> = (0..self.columns * self.rows)
                .map(|i| small.get_pixel(x + i % self.columns, y + i / self.columns).0)
                .collect();
            self.best_split(&pixels)
        };
        (0..height).flat_map(|row| (0..width).map(move |column| cell(column, row))).collect()
    }

    fn best_split(&self, pixels: &[Pixel]) -> Cell {
        let full = u8::MAX >> (8 - pixels.len());
        let ((_, fg, bg), mask) = (1..=full)
            .map(|mask| (split(pixels, mask), mask))
            .min_by_key(|((error, _, _), _)| *error)
            .expect("there are masks");
        ((self.glyph)(mask), rgb(fg), rgb(bg))
    }
}

fn split(pixels: &[Pixel], mask: u8) -> (u32, Pixel, Pixel) {
    let group = |inside: bool| {
        pixels.iter().enumerate().filter(move |(i, _)| (mask >> i & 1 == 1) == inside).map(|(_, pixel)| *pixel)
    };
    let (fg, bg) = (mean(group(true)), mean(group(false)));
    let error = distance_sum(group(true), fg) + distance_sum(group(false), bg);
    (error, fg, bg)
}

fn mean(pixels: impl Iterator<Item = Pixel>) -> Pixel {
    let (mut sum, mut count) = ([0u32; 3], 0u32);
    for pixel in pixels {
        sum = std::array::from_fn(|c| sum[c] + u32::from(pixel[c]));
        count += 1;
    }
    sum.map(|total| total.checked_div(count).unwrap_or(0) as u8)
}

fn distance_sum(pixels: impl Iterator<Item = Pixel>, center: Pixel) -> u32 {
    pixels.flat_map(|p| (0..3).map(move |c| u32::from(p[c].abs_diff(center[c])).pow(2))).sum()
}

fn rgb([r, g, b]: Pixel) -> Color {
    Color::Rgb(r, g, b)
}

fn sextant(mask: u8) -> char {
    const LEFT_HALF: u8 = 0b01_0101;
    const RIGHT_HALF: u8 = 0b10_1010;
    match mask {
        LEFT_HALF => '▌',
        RIGHT_HALF => '▐',
        0b11_1111 => '█',
        _ => {
            let skipped = u32::from(mask > LEFT_HALF) + u32::from(mask > RIGHT_HALF);
            char::from_u32(0x1FB00 + u32::from(mask) - 1 - skipped).unwrap_or('█')
        }
    }
}

fn octant(mask: u8) -> char {
    OCTANT_GLYPHS[usize::from(mask)]
}

const OCTANT_GLYPHS: [char; 256] = [
    ' ', '𜺨', '𜺫', '🮂', '𜴀', '▘', '𜴁', '𜴂',
    '𜴃', '𜴄', '▝', '𜴅', '𜴆', '𜴇', '𜴈', '▀',
    '𜴉', '𜴊', '𜴋', '𜴌', '🯦', '𜴍', '𜴎', '𜴏',
    '𜴐', '𜴑', '𜴒', '𜴓', '𜴔', '𜴕', '𜴖', '𜴗',
    '𜴘', '𜴙', '𜴚', '𜴛', '𜴜', '𜴝', '𜴞', '𜴟',
    '🯧', '𜴠', '𜴡', '𜴢', '𜴣', '𜴤', '𜴥', '𜴦',
    '𜴧', '𜴨', '𜴩', '𜴪', '𜴫', '𜴬', '𜴭', '𜴮',
    '𜴯', '𜴰', '𜴱', '𜴲', '𜴳', '𜴴', '𜴵', '🮅',
    '𜺣', '𜴶', '𜴷', '𜴸', '𜴹', '𜴺', '𜴻', '𜴼',
    '𜴽', '𜴾', '𜴿', '𜵀', '𜵁', '𜵂', '𜵃', '𜵄',
    '▖', '𜵅', '𜵆', '𜵇', '𜵈', '▌', '𜵉', '𜵊',
    '𜵋', '𜵌', '▞', '𜵍', '𜵎', '𜵏', '𜵐', '▛',
    '𜵑', '𜵒', '𜵓', '𜵔', '𜵕', '𜵖', '𜵗', '𜵘',
    '𜵙', '𜵚', '𜵛', '𜵜', '𜵝', '𜵞', '𜵟', '𜵠',
    '𜵡', '𜵢', '𜵣', '𜵤', '𜵥', '𜵦', '𜵧', '𜵨',
    '𜵩', '𜵪', '𜵫', '𜵬', '𜵭', '𜵮', '𜵯', '𜵰',
    '𜺠', '𜵱', '𜵲', '𜵳', '𜵴', '𜵵', '𜵶', '𜵷',
    '𜵸', '𜵹', '𜵺', '𜵻', '𜵼', '𜵽', '𜵾', '𜵿',
    '𜶀', '𜶁', '𜶂', '𜶃', '𜶄', '𜶅', '𜶆', '𜶇',
    '𜶈', '𜶉', '𜶊', '𜶋', '𜶌', '𜶍', '𜶎', '𜶏',
    '▗', '𜶐', '𜶑', '𜶒', '𜶓', '▚', '𜶔', '𜶕',
    '𜶖', '𜶗', '▐', '𜶘', '𜶙', '𜶚', '𜶛', '▜',
    '𜶜', '𜶝', '𜶞', '𜶟', '𜶠', '𜶡', '𜶢', '𜶣',
    '𜶤', '𜶥', '𜶦', '𜶧', '𜶨', '𜶩', '𜶪', '𜶫',
    '▂', '𜶬', '𜶭', '𜶮', '𜶯', '𜶰', '𜶱', '𜶲',
    '𜶳', '𜶴', '𜶵', '𜶶', '𜶷', '𜶸', '𜶹', '𜶺',
    '𜶻', '𜶼', '𜶽', '𜶾', '𜶿', '𜷀', '𜷁', '𜷂',
    '𜷃', '𜷄', '𜷅', '𜷆', '𜷇', '𜷈', '𜷉', '𜷊',
    '𜷋', '𜷌', '𜷍', '𜷎', '𜷏', '𜷐', '𜷑', '𜷒',
    '𜷓', '𜷔', '𜷕', '𜷖', '𜷗', '𜷘', '𜷙', '𜷚',
    '▄', '𜷛', '𜷜', '𜷝', '𜷞', '▙', '𜷟', '𜷠',
    '𜷡', '𜷢', '▟', '𜷣', '▆', '𜷤', '𜷥', '█',
];
