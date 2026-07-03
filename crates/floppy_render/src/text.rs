//! Tiny 5x7 bitmap font for HUD/menu text (SPEC §3: everything procedural,
//! no bundled asset files — glyphs are `const` data compiled into the
//! binary). Covers ASCII `32..=95` (space, punctuation, digits, uppercase);
//! lowercase letters are folded onto their uppercase glyph and anything else
//! outside the covered range falls back to space, so `draw_text` is total
//! (never panics, never indexes out of bounds).
//!
//! Glyph layout: each glyph is `[u8; 7]`, one byte per row top-to-bottom;
//! within a row byte, bits `4..=0` are columns left-to-right (bit 4 =
//! leftmost column, bit 0 = rightmost), so a row is readable directly as a
//! 5-digit binary literal, e.g. `0b01110` draws `.###.`.

use crate::frame::Frame;

const FIRST_CHAR: u32 = 32;
const LAST_CHAR: u32 = 95;
const GLYPH_COUNT: usize = (LAST_CHAR - FIRST_CHAR + 1) as usize;
const GLYPH_W: usize = 5;
const GLYPH_H: usize = 7;
/// Horizontal advance per glyph in pixels (at scale 1): 5px glyph + 1px gap.
const ADVANCE: usize = GLYPH_W + 1;

#[rustfmt::skip]
const FONT: [[u8; 7]; GLYPH_COUNT] = [
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b00000], // 32 ' '
    [0b00100,0b00100,0b00100,0b00100,0b00100,0b00000,0b00100], // 33 '!'
    [0b01010,0b01010,0b00000,0b00000,0b00000,0b00000,0b00000], // 34 '"'
    [0b01010,0b01010,0b11111,0b01010,0b11111,0b01010,0b01010], // 35 '#'
    [0b00100,0b01111,0b10100,0b01110,0b00101,0b11110,0b00100], // 36 '$'
    [0b11001,0b11010,0b00010,0b00100,0b01000,0b01011,0b10011], // 37 '%'
    [0b01100,0b10010,0b10100,0b01000,0b10101,0b10010,0b01101], // 38 '&'
    [0b00100,0b00100,0b01000,0b00000,0b00000,0b00000,0b00000], // 39 '\''
    [0b00010,0b00100,0b01000,0b01000,0b01000,0b00100,0b00010], // 40 '('
    [0b01000,0b00100,0b00010,0b00010,0b00010,0b00100,0b01000], // 41 ')'
    [0b00000,0b00100,0b10101,0b01110,0b10101,0b00100,0b00000], // 42 '*'
    [0b00000,0b00100,0b00100,0b11111,0b00100,0b00100,0b00000], // 43 '+'
    [0b00000,0b00000,0b00000,0b00000,0b00110,0b00100,0b01000], // 44 ','
    [0b00000,0b00000,0b00000,0b11111,0b00000,0b00000,0b00000], // 45 '-'
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00110,0b00110], // 46 '.'
    [0b00001,0b00010,0b00100,0b00100,0b01000,0b10000,0b00000], // 47 '/'
    [0b01110,0b10001,0b10011,0b10101,0b11001,0b10001,0b01110], // 48 '0'
    [0b00100,0b01100,0b00100,0b00100,0b00100,0b00100,0b01110], // 49 '1'
    [0b01110,0b10001,0b00001,0b00010,0b00100,0b01000,0b11111], // 50 '2'
    [0b11111,0b00010,0b00100,0b00010,0b00001,0b10001,0b01110], // 51 '3'
    [0b00010,0b00110,0b01010,0b10010,0b11111,0b00010,0b00010], // 52 '4'
    [0b11111,0b10000,0b11110,0b00001,0b00001,0b10001,0b01110], // 53 '5'
    [0b00110,0b01000,0b10000,0b11110,0b10001,0b10001,0b01110], // 54 '6'
    [0b11111,0b00001,0b00010,0b00100,0b01000,0b01000,0b01000], // 55 '7'
    [0b01110,0b10001,0b10001,0b01110,0b10001,0b10001,0b01110], // 56 '8'
    [0b01110,0b10001,0b10001,0b01111,0b00001,0b00010,0b01100], // 57 '9'
    [0b00000,0b00110,0b00110,0b00000,0b00110,0b00110,0b00000], // 58 ':'
    [0b00000,0b00110,0b00110,0b00000,0b00110,0b00100,0b01000], // 59 ';'
    [0b00010,0b00100,0b01000,0b10000,0b01000,0b00100,0b00010], // 60 '<'
    [0b00000,0b00000,0b11111,0b00000,0b11111,0b00000,0b00000], // 61 '='
    [0b01000,0b00100,0b00010,0b00001,0b00010,0b00100,0b01000], // 62 '>'
    [0b01110,0b10001,0b00001,0b00010,0b00100,0b00000,0b00100], // 63 '?'
    [0b01110,0b10001,0b10111,0b10101,0b10111,0b10000,0b01110], // 64 '@'
    [0b01110,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001], // 65 'A'
    [0b11110,0b10001,0b10001,0b11110,0b10001,0b10001,0b11110], // 66 'B'
    [0b01110,0b10001,0b10000,0b10000,0b10000,0b10001,0b01110], // 67 'C'
    [0b11100,0b10010,0b10001,0b10001,0b10001,0b10010,0b11100], // 68 'D'
    [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b11111], // 69 'E'
    [0b11111,0b10000,0b10000,0b11110,0b10000,0b10000,0b10000], // 70 'F'
    [0b01110,0b10001,0b10000,0b10111,0b10001,0b10001,0b01111], // 71 'G'
    [0b10001,0b10001,0b10001,0b11111,0b10001,0b10001,0b10001], // 72 'H'
    [0b01110,0b00100,0b00100,0b00100,0b00100,0b00100,0b01110], // 73 'I'
    [0b00001,0b00001,0b00001,0b00001,0b00001,0b10001,0b01110], // 74 'J'
    [0b10001,0b10010,0b10100,0b11000,0b10100,0b10010,0b10001], // 75 'K'
    [0b10000,0b10000,0b10000,0b10000,0b10000,0b10000,0b11111], // 76 'L'
    [0b10001,0b11011,0b10101,0b10101,0b10001,0b10001,0b10001], // 77 'M'
    [0b10001,0b11001,0b10101,0b10011,0b10001,0b10001,0b10001], // 78 'N'
    [0b01110,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110], // 79 'O'
    [0b11110,0b10001,0b10001,0b11110,0b10000,0b10000,0b10000], // 80 'P'
    [0b01110,0b10001,0b10001,0b10001,0b10101,0b10010,0b01101], // 81 'Q'
    [0b11110,0b10001,0b10001,0b11110,0b10100,0b10010,0b10001], // 82 'R'
    [0b01111,0b10000,0b10000,0b01110,0b00001,0b00001,0b11110], // 83 'S'
    [0b11111,0b00100,0b00100,0b00100,0b00100,0b00100,0b00100], // 84 'T'
    [0b10001,0b10001,0b10001,0b10001,0b10001,0b10001,0b01110], // 85 'U'
    [0b10001,0b10001,0b10001,0b10001,0b10001,0b01010,0b00100], // 86 'V'
    [0b10001,0b10001,0b10001,0b10101,0b10101,0b10101,0b01010], // 87 'W'
    [0b10001,0b10001,0b01010,0b00100,0b01010,0b10001,0b10001], // 88 'X'
    [0b10001,0b10001,0b01010,0b00100,0b00100,0b00100,0b00100], // 89 'Y'
    [0b11111,0b00001,0b00010,0b00100,0b01000,0b10000,0b11111], // 90 'Z'
    [0b01110,0b01000,0b01000,0b01000,0b01000,0b01000,0b01110], // 91 '['
    [0b10000,0b01000,0b00100,0b00100,0b00010,0b00001,0b00000], // 92 '\\'
    [0b01110,0b00010,0b00010,0b00010,0b00010,0b00010,0b01110], // 93 ']'
    [0b00100,0b01010,0b10001,0b00000,0b00000,0b00000,0b00000], // 94 '^'
    [0b00000,0b00000,0b00000,0b00000,0b00000,0b00000,0b11111], // 95 '_'
];

/// Map a `char` to a `FONT` index; lowercase `a..=z` folds onto uppercase,
/// anything else outside `32..=95` falls back to the space glyph (index 0).
fn glyph_index(ch: char) -> usize {
    let c = ch as u32;
    let upper = if (97..=122).contains(&c) { c - 32 } else { c };
    if (FIRST_CHAR..=LAST_CHAR).contains(&upper) {
        (upper - FIRST_CHAR) as usize
    } else {
        0
    }
}

/// Draw `s` starting at pixel `(x, y)` (top-left of the first glyph), each
/// glyph pixel expanded to a `scale x scale` block. Solid color, no
/// blending. Bounds-checked via `Frame::set` — never panics for any `scale`
/// (including `0`, which draws nothing) or off-screen position.
pub fn draw_text(frame: &mut Frame, x: i32, y: i32, scale: usize, color: u32, s: &str) {
    let mut cursor_x = x;
    for ch in s.chars() {
        let glyph = &FONT[glyph_index(ch)];
        for (row, &bits) in glyph.iter().enumerate() {
            for col in 0..GLYPH_W {
                let bit = (bits >> (GLYPH_W - 1 - col)) & 1;
                if bit == 1 {
                    for sy in 0..scale {
                        for sx in 0..scale {
                            let px = cursor_x + (col * scale + sx) as i32;
                            let py = y + (row * scale + sy) as i32;
                            frame.set(px, py, color);
                        }
                    }
                }
            }
        }
        cursor_x += (ADVANCE * scale) as i32;
    }
}

/// `(width, height)` in pixels that [`draw_text`] would occupy for `s` at
/// `scale`, consistent with `draw_text`'s own advance/row math.
pub fn measure(s: &str, scale: usize) -> (usize, usize) {
    (s.chars().count() * ADVANCE * scale, GLYPH_H * scale)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn draw_text_paints_nonzero_pixels() {
        let mut f = Frame::new(200, 40);
        draw_text(&mut f, 2, 2, 2, 0x00FFFFFF, "SPIN 42");
        let painted = f.px.iter().filter(|&&p| p != 0).count();
        assert!(painted > 0, "expected some painted pixels");
    }

    #[test]
    fn measure_matches_draw_text_advance() {
        let (w, h) = measure("SPIN 42", 2);
        assert_eq!(w, "SPIN 42".chars().count() * ADVANCE * 2);
        assert_eq!(h, GLYPH_H * 2);

        // Painted pixels must stay within the measured bounding box.
        let mut f = Frame::new(300, 60);
        let (x0, y0) = (5, 5);
        draw_text(&mut f, x0, y0, 2, 0x00FFFFFF, "SPIN 42");
        for yy in 0..f.h {
            for xx in 0..f.w {
                if f.px[yy * f.w + xx] != 0 {
                    assert!((xx as i32) < x0 + w as i32);
                    assert!((yy as i32) < y0 + h as i32);
                }
            }
        }
    }

    #[test]
    fn scale_zero_or_offscreen_never_panics() {
        let mut f = Frame::new(10, 10);
        draw_text(&mut f, 0, 0, 0, 0xFFFFFF, "TEST");
        draw_text(&mut f, -1000, -1000, 3, 0xFFFFFF, "OFFSCREEN 123");
        draw_text(&mut f, 5, 5, 2, 0xFFFFFF, "lowercase~unmapped");
    }

    #[test]
    fn lowercase_maps_to_same_glyph_as_uppercase() {
        assert_eq!(glyph_index('a'), glyph_index('A'));
        assert_eq!(glyph_index('z'), glyph_index('Z'));
        assert_eq!(glyph_index('{'), 0); // outside range -> space
    }
}
