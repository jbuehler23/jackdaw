//! Frame strips: a sheet `N` times as wide as another whose first block
//! matches it, found by comparing pixels over the other sheet's grid.

use glam::UVec2;

use crate::slice::{GridLayout, RgbaImage, canonical};

/// A mismatch share that tolerates highlight differences between a still
/// sheet and frame 0 of its strip.
pub const LOOSE_FRAME_MATCH: f32 = 0.02;

/// The frames of one base tile as tile ids in the strip.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRun {
    pub base: u32,
    pub frames: Vec<u32>,
}

/// A sheet made of `frames` copies of a base sheet placed side by side, each
/// `block_width` wide, the first matching the base. Strip tile ids number
/// the strip row by row with `frames * base columns` ids per row; slice
/// frames with [`AnimationStrip::frame_rect`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AnimationStrip {
    pub frames: u32,
    pub base_layout: GridLayout,
    pub block_width: u32,
    pub runs: Vec<FrameRun>,
}

impl AnimationStrip {
    /// Pixel origin and size of strip tile `index`.
    pub fn frame_rect(&self, index: u32) -> (UVec2, UVec2) {
        let columns = self.base_layout.columns.max(1);
        let strip_columns = columns * self.frames.max(1);
        let (row, column) = (index / strip_columns, index % strip_columns);
        let (frame, base_column) = (column / columns, column % columns);
        let (origin, size) = self.base_layout.tile_rect(row * columns + base_column);
        (origin + UVec2::new(frame * self.block_width, 0), size)
    }
}

/// `strip` as an animation of `base`: its width is `N >= 2` times the base
/// width, the heights match, and of the pixels opaque in either the base or
/// the strip's first block, at most a `max_mismatch` share (in `0..=1`)
/// differ. `None` when the base is fully transparent or `base_layout` does
/// not fit the base. Use `0.0` for an exact match; sheets whose still
/// version has different highlights than frame 0 need a small share such as
/// [`LOOSE_FRAME_MATCH`].
pub fn detect_animation(
    base: &RgbaImage,
    base_layout: &GridLayout,
    strip: &RgbaImage,
    max_mismatch: f32,
) -> Option<AnimationStrip> {
    let tolerance_valid = max_mismatch.is_finite() && (0.0..=1.0).contains(&max_mismatch);
    let shape_valid = base.width > 0
        && strip.height == base.height
        && strip.width.is_multiple_of(base.width)
        && strip.width / base.width >= 2;
    if !tolerance_valid || !shape_valid || !base_layout.fits(UVec2::new(base.width, base.height)) {
        return None;
    }
    let frames = strip.width / base.width;
    let limit = max_mismatch * base.width as f32 * base.height as f32;
    let (mut base_opaque, mut opaque, mut mismatched) = (0u64, 0u64, 0u64);
    for (base_row, strip_row) in base.rows().zip(strip.rows()) {
        for (a, b) in base_row
            .as_chunks::<4>()
            .0
            .iter()
            .zip(strip_row.as_chunks::<4>().0)
        {
            let (a, b) = (canonical(a), canonical(b));
            base_opaque += u64::from(a[3] != 0);
            if a[3] == 0 && b[3] == 0 {
                continue;
            }
            opaque += 1;
            if a != b {
                mismatched += 1;
                if mismatched as f32 > limit {
                    return None;
                }
            }
        }
    }
    if base_opaque == 0 || mismatched as f32 > max_mismatch * opaque as f32 {
        return None;
    }
    let columns = base_layout.columns;
    let runs = (0..base_layout.tile_count())
        .map(|index| {
            let (row, column) = (index / columns, index % columns);
            FrameRun {
                base: index,
                frames: (0..frames)
                    .map(|f| (row * frames + f) * columns + column)
                    .collect(),
            }
        })
        .collect();
    Some(AnimationStrip {
        frames,
        base_layout: *base_layout,
        block_width: base.width,
        runs,
    })
}

/// `(base, strip, animation)` for every image that animates another, in
/// strip order. Each strip pairs with its narrowest base (lowest index on a
/// tie), and an image that is itself a strip is never a base: for sheets of
/// width W, 2W and 4W this gives (W, 2W) and (W, 4W). `layouts[i]` is the
/// grid of `images[i]`.
pub fn pair_animations(
    images: &[RgbaImage],
    layouts: &[GridLayout],
    max_mismatch: f32,
) -> Vec<(usize, usize, AnimationStrip)> {
    let n = images.len().min(layouts.len());
    let mut best: Vec<Option<(usize, AnimationStrip)>> = (0..n).map(|_| None).collect();
    for (s, slot) in best.iter_mut().enumerate() {
        for b in (0..n).filter(|&b| b != s) {
            let narrower = slot
                .as_ref()
                .is_none_or(|(current, _)| images[b].width < images[*current].width);
            if !narrower {
                continue;
            }
            if let Some(found) = detect_animation(&images[b], &layouts[b], &images[s], max_mismatch)
            {
                *slot = Some((b, found));
            }
        }
    }
    let is_strip: Vec<bool> = best.iter().map(Option::is_some).collect();
    best.into_iter()
        .enumerate()
        .filter_map(|(s, found)| found.map(|(b, a)| (b, s, a)))
        .filter(|(b, _, _)| !is_strip[*b])
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Sheet {
        width: u32,
        height: u32,
        pixels: Vec<u8>,
    }

    fn hash(a: u32, b: u32, c: u32) -> u32 {
        let mut h =
            a.wrapping_mul(0x9E37_79B1) ^ b.wrapping_mul(0x85EB_CA77) ^ c.wrapping_mul(0xC2B2_AE3D);
        h ^= h >> 15;
        h = h.wrapping_mul(0x2C1B_3C6D);
        h ^ (h >> 12)
    }

    impl Sheet {
        fn new(width: u32, height: u32, fill: [u8; 4]) -> Self {
            Self {
                width,
                height,
                pixels: fill.repeat((width * height) as usize),
            }
        }

        fn set(&mut self, x: u32, y: u32, c: [u8; 4]) {
            let at = ((y * self.width + x) * 4) as usize;
            self.pixels[at..at + 4].copy_from_slice(&c);
        }

        /// Fills each cell of `layout` with a colour per `block` of pixels
        /// (measured from the first cell) plus per-pixel noise. `spread` is
        /// the range of the block colours, `noise` the range of the noise.
        fn cells(&mut self, layout: &GridLayout, block: UVec2, seed: u32, spread: u32, noise: u32) {
            let origin = layout.tile_rect(0).0;
            for index in 0..layout.tile_count() {
                let (o, s) = layout.tile_rect(index);
                for y in o.y..o.y + s.y {
                    for x in o.x..o.x + s.x {
                        let b = (UVec2::new(x, y) - origin) / block;
                        let base = hash(b.x, b.y, seed);
                        let n = hash(x, y, seed + 1) % noise;
                        let channel = |shift: u32| (40 + (base >> shift) % spread + n) as u8;
                        self.set(x, y, [channel(0), channel(8), channel(16), 255]);
                    }
                }
            }
        }

        fn noisy_cells(&mut self, layout: &GridLayout, block: UVec2, seed: u32) {
            self.cells(layout, block, seed, 180, 12);
        }

        fn image(&self) -> RgbaImage<'_> {
            RgbaImage::new(self.width, self.height, &self.pixels).expect("sized")
        }
    }

    fn layout(tile: u32, margin: u32, spacing: u32, columns: u32, rows: u32) -> GridLayout {
        GridLayout {
            tile: UVec2::splat(tile),
            margin: UVec2::splat(margin),
            spacing: UVec2::splat(spacing),
            columns,
            rows,
        }
    }

    /// A sheet of `size` filled with `background` and noisy cells of `l`.
    fn sheet(size: UVec2, l: &GridLayout, background: [u8; 4], seed: u32) -> Sheet {
        let mut sheet = Sheet::new(size.x, size.y, background);
        sheet.noisy_cells(l, l.tile, seed);
        sheet
    }

    fn strip_of(base: &Sheet, frames: u32) -> Sheet {
        let mut strip = Sheet::new(base.width * frames, base.height, [0; 4]);
        for f in 0..frames {
            for y in 0..base.height {
                for x in 0..base.width {
                    let mut c = base.image().pixel(x, y).expect("in range");
                    if f > 0 && c[3] != 0 {
                        c[0] = c[0].wrapping_add(f as u8 * 9);
                    }
                    strip.set(f * base.width + x, y, c);
                }
            }
        }
        strip
    }

    fn base_sheet() -> (GridLayout, Sheet) {
        let l = layout(16, 0, 0, 3, 2);
        let s = sheet(l.extent(), &l, [0; 4], 21);
        (l, s)
    }

    #[test]
    fn animation_found_for_2_4_8_frames() {
        let (l, base) = base_sheet();
        for frames in [2, 4, 8] {
            let strip = strip_of(&base, frames);
            let found = detect_animation(&base.image(), &l, &strip.image(), 0.0).expect("strip");
            assert_eq!((found.frames, found.runs.len()), (frames, 6));
            let last: Vec<u32> = (0..frames).map(|f| 3 * frames + f * 3 + 2).collect();
            assert_eq!(
                found.runs[5],
                FrameRun {
                    base: 5,
                    frames: last
                }
            );
        }
    }

    #[test]
    fn animation_rejected_on_mismatch() {
        let (l, base) = base_sheet();
        let b = base.image();
        let off_width = Sheet::new(base.width * 2 + 16, base.height, [0; 4]);
        assert!(detect_animation(&b, &l, &off_width.image(), 0.0).is_none());
        let mut changed = strip_of(&base, 4);
        let p = changed.image().pixel(5, 5).expect("in range");
        changed.set(5, 5, [p[0].wrapping_add(2), p[1], p[2], p[3]]);
        assert!(detect_animation(&b, &l, &changed.image(), 0.0).is_none());
        assert!(detect_animation(&b, &l, &changed.image(), 0.001).is_some());
        let tall = Sheet::new(base.width * 4, base.height + 16, [0; 4]);
        assert!(detect_animation(&b, &l, &tall.image(), 0.0).is_none());
        assert!(detect_animation(&b, &l, &b, 0.0).is_none());
    }

    #[test]
    fn animation_rejects_bad_layout_and_tolerance() {
        let (l, base) = base_sheet();
        let strip = strip_of(&base, 2);
        let (b, s) = (base.image(), strip.image());
        assert!(detect_animation(&b, &l, &s, 0.0).is_some());
        let too_wide = GridLayout { columns: 4, ..l };
        assert!(detect_animation(&b, &too_wide, &s, 0.0).is_none());
        let empty = GridLayout { rows: 0, ..l };
        assert!(detect_animation(&b, &empty, &s, 0.0).is_none());
        for bad in [f32::NAN, f32::INFINITY, -0.1, 1.5] {
            assert!(detect_animation(&b, &l, &s, bad).is_none());
        }
    }

    #[test]
    fn mismatch_share_counts_only_opaque_pixels() {
        let l = layout(16, 0, 0, 2, 1);
        let mut base = Sheet::new(32, 16, [0; 4]);
        for x in 0..10 {
            base.set(x, 3, [200, 10, 10, 255]);
        }
        let mut strip = strip_of(&base, 2);
        strip.set(0, 3, [10, 200, 10, 255]);
        let (b, s) = (base.image(), strip.image());
        assert!(detect_animation(&b, &l, &s, 0.05).is_none());
        assert!(detect_animation(&b, &l, &s, 0.1).is_some());
    }

    #[test]
    fn transparent_base_never_pairs() {
        let l = layout(16, 0, 0, 2, 1);
        let base = Sheet::new(32, 16, [0; 4]);
        let strip = Sheet::new(64, 16, [0; 4]);
        assert!(detect_animation(&base.image(), &l, &strip.image(), 1.0).is_none());
    }

    #[test]
    fn frame_runs_index_and_locate_with_spacing() {
        let l = layout(8, 0, 1, 2, 2);
        let base = sheet(l.extent(), &l, [0; 4], 4);
        let strip = strip_of(&base, 3);
        let found = detect_animation(&base.image(), &l, &strip.image(), 0.0).expect("strip");
        let runs: Vec<Vec<u32>> = found.runs.iter().map(|r| r.frames.clone()).collect();
        assert_eq!(
            runs,
            [vec![0, 2, 4], vec![1, 3, 5], vec![6, 8, 10], vec![7, 9, 11]]
        );
        let (origin, size) = found.frame_rect(9);
        assert_eq!((origin, size), (UVec2::new(26, 9), UVec2::splat(8)));
        let (base_origin, _) = l.tile_rect(3);
        for y in 0..size.y {
            for x in 0..size.x {
                let a = base
                    .image()
                    .pixel(base_origin.x + x, base_origin.y + y)
                    .expect("base");
                let b = strip
                    .image()
                    .pixel(origin.x + x, origin.y + y)
                    .expect("strip");
                assert_eq!([a[1], a[2], a[3]], [b[1], b[2], b[3]]);
            }
        }
    }

    #[test]
    fn pairs_pick_the_smallest_base_and_skip_strips_as_bases() {
        let (l, base) = base_sheet();
        let double = strip_of(&base, 2);
        let quad = Sheet {
            width: base.width * 4,
            height: base.height,
            pixels: strip_of(&double, 2).pixels,
        };
        let other = sheet(l.extent(), &l, [0; 4], 99);
        let images = [quad.image(), base.image(), double.image(), other.image()];
        let wide = |n: u32| GridLayout {
            columns: 3 * n,
            ..l
        };
        let found = pair_animations(&images, &[wide(4), l, wide(2), l], 0.0);
        let pairs: Vec<(usize, usize, u32)> =
            found.iter().map(|(b, s, a)| (*b, *s, a.frames)).collect();
        assert_eq!(pairs, [(1, 0, 4), (1, 2, 2)]);
    }
}
