//! The colour layer: a per-cell tint the splat material multiplies its finished
//! albedo by. White is the identity, so an older sidecar carrying no colour
//! layer renders unchanged.
//!
//! Two ways in. [`apply_color_brush`] is a circular brush easing each cell
//! toward a chosen colour, mirroring [`crate::control::apply_control_brush`].
//! [`fill_color_variation`] lays low-frequency noise down from white across the
//! whole layer, which breaks up a large flat field of one texture.

use bevy_math::Vec2;

/// Ease a colour into the tint layer under a circular brush.
///
/// `center` and `radius` are in grid cells. `opacity` is how far a cell crosses
/// toward `tint` per second at full brush strength, scaled here by frame `dt`.
/// `hardness` is the fraction of the radius that gets full strength before the
/// falloff starts; `falloff` shapes what is left, as for every other brush here.
///
/// Alpha is left as it is: the shader reads only RGB.
///
/// Returns the number of cells changed.
#[expect(
    clippy::too_many_arguments,
    reason = "mirrors apply_control_brush's flat parameter list"
)]
pub fn apply_color_brush(
    colors: &mut [[u8; 4]],
    resolution: u32,
    center: Vec2,
    radius: f32,
    falloff: f32,
    hardness: f32,
    opacity: f32,
    dt: f32,
    tint: [u8; 3],
) -> usize {
    // Mirrors `apply_control_brush`'s guard: a non-finite radius, opacity
    // or dt must not poison the layer with NaN-derived colours.
    if resolution == 0
        || !radius.is_finite()
        || radius <= 0.0
        || !opacity.is_finite()
        || opacity <= 0.0
        || !dt.is_finite()
        || dt <= 0.0
    {
        return 0;
    }
    let res = resolution as i32;

    let min_x = ((center.x - radius).floor() as i32).clamp(0, res - 1);
    let max_x = ((center.x + radius).ceil() as i32).clamp(0, res - 1);
    let min_z = ((center.y - radius).floor() as i32).clamp(0, res - 1);
    let max_z = ((center.y + radius).ceil() as i32).clamp(0, res - 1);

    let mut changed = 0;
    for gz in min_z..=max_z {
        for gx in min_x..=max_x {
            let dist = ((gx as f32 - center.x).powi(2) + (gz as f32 - center.y).powi(2)).sqrt();
            let f = brush_weight(dist, radius, falloff, hardness);
            if f <= 0.0 {
                continue;
            }
            let idx = (gz * res + gx) as usize;
            let Some(before) = colors.get(idx).copied() else {
                continue;
            };
            let t = (opacity * f * dt).clamp(0.0, 1.0);
            let mut after = before;
            for channel in 0..3 {
                after[channel] = mix_channel(before[channel], tint[channel], t);
            }
            if after != before {
                colors[idx] = after;
                changed += 1;
            }
        }
    }
    changed
}

/// Brush strength at `dist`, with a hard plateau out to `hardness` of the
/// radius and [`crate::control`]'s falloff shape over the rest.
fn brush_weight(dist: f32, radius: f32, falloff: f32, hardness: f32) -> f32 {
    if dist >= radius {
        return 0.0;
    }
    let hardness = if hardness.is_finite() {
        hardness.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let plateau = radius * hardness;
    if dist <= plateau {
        return 1.0;
    }
    // Everything past the plateau falls off across what is left of the radius, so
    // raising hardness widens the flat middle rather than steepening the edge.
    let band = radius - plateau;
    if band <= 0.0 {
        return 1.0;
    }
    let t = 1.0 - (dist - plateau) / band;
    let falloff = if falloff.is_finite() && falloff > 0.0 {
        falloff
    } else {
        1.0
    };
    t.clamp(0.0, 1.0).powf(falloff)
}

/// One channel eased `t` of the way from `from` to `to`, rounded.
///
/// A step that rounds back to `from` is nudged one level toward `to` instead, so
/// a brush held on a cell converges rather than stalling short of it. The result
/// never passes `to`, so an eraser stroke restores white exactly.
fn mix_channel(from: u8, to: u8, t: f32) -> u8 {
    if from == to || !t.is_finite() || t <= 0.0 {
        return from;
    }
    let from_f = from as f32;
    let to_f = to as f32;
    let mut value = (from_f + (to_f - from_f) * t).round();
    if value == from_f {
        value += if to > from { 1.0 } else { -1.0 };
    }
    value.clamp(from_f.min(to_f), from_f.max(to_f)) as u8
}

/// Lay low-frequency noise over the whole colour layer: a slow wander of light
/// and shade that stops a large field of one texture reading flat. Every cell of
/// the dense `resolution`-per-edge grid is written, so the layer is replaced
/// rather than blended into.
///
/// The wash runs from white down, not around it: the layer multiplies the albedo,
/// so a wash centred on white would flatten its bright half against that ceiling
/// and read as one-sided blotching. `amount` is how far the darkest cell falls,
/// `0..1` of the full range; `frequency` is noise cycles per cell.
///
/// Deterministic in `seed`.
#[cfg(feature = "procgen")]
pub fn fill_color_variation(
    regions: &mut crate::region::TerrainRegions,
    resolution: u32,
    seed: u32,
    frequency: f32,
    amount: f32,
) {
    let grid = color_variation(resolution, seed, frequency, amount);
    regions.write_grid_color(resolution, &grid);
}

/// [`fill_color_variation`] as a dense grid, for a caller that already holds
/// one: the editor writes the layer through its own dirty-rect plumbing and
/// wants the texels rather than a second copy of the regions.
#[cfg(feature = "procgen")]
pub fn color_variation(resolution: u32, seed: u32, frequency: f32, amount: f32) -> Vec<[u8; 4]> {
    use noise::{Fbm, MultiFractal, NoiseFn, Perlin};

    if resolution == 0 {
        return Vec::new();
    }
    let amount = if amount.is_finite() {
        amount.clamp(0.0, 1.0)
    } else {
        0.0
    };
    let frequency = if frequency.is_finite() && frequency > 0.0 {
        frequency as f64
    } else {
        0.01
    };
    // Two octaves: enough that the wash does not read as one smooth gradient,
    // few enough that it stays large-scale variation rather than a texture.
    let noise = Fbm::<Perlin>::new(seed)
        .set_frequency(frequency)
        .set_octaves(2)
        .set_lacunarity(2.0)
        .set_persistence(0.5);

    let mut grid = Vec::with_capacity((resolution as usize) * (resolution as usize));
    for gz in 0..resolution {
        for gx in 0..resolution {
            let value = noise.get([gx as f64, gz as f64]) as f32;
            let mut texel = crate::region::DEFAULT_COLOR;
            // Noise arrives in -1..1 and is mapped onto 1-amount..1 of white, so
            // both lobes are drawn rather than the bright one clamping flat.
            let lit = (value.clamp(-1.0, 1.0) + 1.0) * 0.5;
            let level = (255.0 * (1.0 - amount * (1.0 - lit)))
                .round()
                .clamp(0.0, 255.0) as u8;
            texel[0] = level;
            texel[1] = level;
            texel[2] = level;
            grid.push(texel);
        }
    }
    grid
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::region::DEFAULT_COLOR;

    fn white_layer(resolution: u32) -> Vec<[u8; 4]> {
        vec![DEFAULT_COLOR; (resolution as usize) * (resolution as usize)]
    }

    /// Painting white onto white changes nothing, which is what makes an
    /// untinted terrain render exactly as it did before the layer existed.
    #[test]
    fn painting_white_onto_white_writes_no_cell() {
        let mut colors = white_layer(16);
        let changed = apply_color_brush(
            &mut colors,
            16,
            Vec2::splat(8.0),
            6.0,
            2.0,
            0.5,
            1.0,
            1.0,
            [255, 255, 255],
        );
        assert_eq!(changed, 0);
        assert_eq!(colors, white_layer(16));
    }

    /// The brush is a rate, so half the frame time crosses less of the
    /// way toward the tint. Two half-steps also land short of one whole
    /// one, the same easing every accumulating brush here has.
    #[test]
    fn opacity_scales_with_frame_time() {
        let paint = |dt: f32| {
            let mut colors = white_layer(8);
            apply_color_brush(
                &mut colors,
                8,
                Vec2::splat(4.0),
                3.0,
                2.0,
                1.0,
                1.0,
                dt,
                [0, 0, 0],
            );
            colors[4 * 8 + 4][0]
        };
        let half = paint(0.5);
        let whole = paint(1.0);
        assert!(
            half > whole,
            "a half-length step must land short of a whole one: {half} vs {whole}"
        );
        assert_eq!(whole, 0, "a full step reaches the tint");
        assert_eq!(half, 128, "a half step lands halfway");
    }

    /// A brush held on a cell has to arrive: rounding a sub-level step
    /// back to where it started leaves a stroke stuck at a haze and
    /// Ctrl-erase unable to restore white.
    #[test]
    fn holding_the_brush_converges_on_the_colour_it_paints() {
        // The default paint opacity at 60 fps: about 0.008 of the way per
        // frame, which is under one level for every channel within about
        // 60 of its target: the range a rounded step stalls in.
        let mut colors = white_layer(8);
        let center = Vec2::splat(4.0);
        let hold = |colors: &mut Vec<[u8; 4]>, frames: usize, tint: [u8; 3]| {
            for _ in 0..frames {
                apply_color_brush(colors, 8, center, 3.0, 2.0, 1.0, 0.5, 1.0 / 60.0, tint);
            }
        };

        hold(&mut colors, 200, [255, 0, 0]);
        let after_200 = colors[4 * 8 + 4];
        assert!(
            after_200[1] < 30,
            "200 frames must be well past the ~60-level rounding stall: {after_200:?}"
        );

        hold(&mut colors, 800, [255, 0, 0]);
        assert_eq!(
            colors[4 * 8 + 4],
            [255, 0, 0, 255],
            "a brush held on a cell has to reach the tint, not stall short of it"
        );

        hold(&mut colors, 1000, [255, 255, 255]);
        assert_eq!(
            colors[4 * 8 + 4],
            DEFAULT_COLOR,
            "the eraser must restore exactly white, or the layer keeps a haze"
        );
    }

    /// A step never passes the colour it is easing toward, so the cell
    /// settles on the tint rather than oscillating around it.
    #[test]
    fn a_step_never_overshoots_the_tint() {
        assert_eq!(mix_channel(254, 255, 0.001), 255);
        assert_eq!(mix_channel(1, 0, 0.001), 0);
        assert_eq!(mix_channel(0, 255, 0.001), 1);
        assert_eq!(mix_channel(255, 255, 1.0), 255);
        assert_eq!(mix_channel(10, 20, 0.0), 10);
    }

    /// Hardness is a plateau, not a curve: every cell inside
    /// `radius * hardness` gets the same full-strength write, and the
    /// falloff shapes only what is outside it.
    #[test]
    fn hardness_holds_the_middle_of_the_brush_flat() {
        let mut colors = white_layer(32);
        apply_color_brush(
            &mut colors,
            32,
            Vec2::splat(16.0),
            10.0,
            2.0,
            0.8,
            1.0,
            1.0,
            [0, 0, 0],
        );
        let at = |x: usize, z: usize| colors[z * 32 + x][0];
        // Inside the 8-cell plateau, every cell is fully tinted.
        for offset in 0..=7 {
            assert_eq!(at(16 + offset, 16), 0, "cell {offset} inside the plateau");
        }
        // Past it the edge softens rather than cutting off.
        assert!(at(16 + 9, 16) > 0, "the edge outside the plateau is soft");
        assert_eq!(
            at(16 + 10, 16),
            255,
            "nothing outside the radius is touched"
        );
    }

    /// A brush wider than one cell of falloff still eases: without a
    /// plateau the very centre is the only full-strength cell.
    #[test]
    fn a_soft_brush_falls_off_from_its_centre() {
        let mut colors = white_layer(32);
        apply_color_brush(
            &mut colors,
            32,
            Vec2::splat(16.0),
            8.0,
            2.0,
            0.0,
            1.0,
            1.0,
            [0, 0, 0],
        );
        let at = |x: usize| colors[16 * 32 + x][0];
        assert_eq!(at(16), 0);
        assert!(
            at(18) > at(17),
            "the tint weakens outward: {} {}",
            at(17),
            at(18)
        );
    }

    #[cfg(feature = "procgen")]
    mod variation {
        use super::*;
        use crate::region::RegionSize;

        fn varied(seed: u32, amount: f32) -> crate::region::TerrainRegions {
            let mut regions = crate::region::TerrainRegions::new(RegionSize::new(16).unwrap());
            regions.ensure_grid(16).expect("a 16-cell grid fits");
            fill_color_variation(&mut regions, 16, seed, 0.08, amount);
            regions
        }

        /// The same seed writes the same layer, so a scripted zone can be
        /// regenerated rather than kept as bytes.
        #[test]
        fn the_same_seed_writes_the_same_layer() {
            assert_eq!(
                varied(7, 0.2).read_grid_color(16),
                varied(7, 0.2).read_grid_color(16)
            );
            assert_ne!(
                varied(7, 0.2).read_grid_color(16),
                varied(8, 0.2).read_grid_color(16)
            );
        }

        /// Every cell stays inside `amount` of white, so the dial means
        /// what it says and the ground never turns a colour nobody asked
        /// for.
        #[test]
        fn the_variation_stays_within_the_amount_asked_for() {
            let amount = 0.15_f32;
            let bound = (amount * 255.0).ceil() as i32;
            let grid = varied(3, amount).read_grid_color(16);
            let mut spread = 0;
            for texel in &grid {
                for channel in &texel[..3] {
                    let off = 255 - i32::from(*channel);
                    assert!(off <= bound, "channel {channel} is {off} from white");
                    spread = spread.max(off);
                }
                assert_eq!(texel[3], 255, "alpha is left opaque");
            }
            assert!(spread > 0, "the variation has to vary");
        }

        /// The wash spreads across the band rather than piling up against white:
        /// a wash centred on white loses its whole bright lobe to the 255
        /// ceiling.
        #[test]
        fn the_variation_spreads_across_the_band_rather_than_clamping_flat() {
            let amount = 0.4_f32;
            let grid = varied(5, amount).read_grid_color(16);
            let levels: Vec<i32> = grid.iter().map(|texel| i32::from(texel[0])).collect();
            let floor = (255.0 * (1.0 - amount)) as i32;
            let middle = (floor + 255) / 2;
            let darkest = *levels.iter().min().expect("the grid has cells");
            let brightest = *levels.iter().max().expect("the grid has cells");
            assert!(
                darkest < middle && brightest > middle,
                "the wash runs {darkest}..{brightest} and never crosses the band's \
                 middle at {middle}"
            );

            // A wash centred on white loses its bright half to the 255
            // ceiling, which shows up as a pile of cells at exactly white
            // and an average well above the middle of the band.
            let at_white = levels.iter().filter(|level| **level == 255).count();
            assert!(
                at_white * 8 < levels.len(),
                "{at_white} of {} cells are flat white, so the bright half was clamped away",
                levels.len()
            );
            let mean = levels.iter().sum::<i32>() / levels.len() as i32;
            assert!(
                (mean - middle).abs() * 6 < 255 - floor,
                "the wash averages {mean}, not the band's middle at {middle}"
            );
        }

        /// Zero amount is plain white: the identity, so turning the dial
        /// down undoes the look without touching the layer's presence.
        #[test]
        fn a_zero_amount_writes_plain_white() {
            assert!(
                varied(11, 0.0)
                    .read_grid_color(16)
                    .iter()
                    .all(|texel| *texel == DEFAULT_COLOR)
            );
        }
    }
}
