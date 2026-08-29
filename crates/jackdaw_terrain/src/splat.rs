//! How a terrain's control words reach a fragment shader, and the
//! coordinate math both sides must agree on.
//!
//! The control map goes to the GPU as one `R32Uint` texture per terrain,
//! `resolution x resolution` texels, one texel per grid point, holding the
//! raw [`Control`] word. `R32Uint` cannot be filtered, so the shader
//! reads it with `textureLoad` only and a packed word is never
//! interpolated into a different packed word.
//!
//! The shader addresses that texture through the mesh's UV0 attribute,
//! which the mesher writes as `gx / (resolution - 1)`: one coordinate
//! space across the whole terrain, not per chunk. Two chunks meeting at
//! grid line `gx` emit that same value from the same expression for their
//! shared vertices, so the interpolated UV agrees bit for bit along the
//! seam and needs no half-texel offset. [`control_corner`] is the CPU
//! mirror of the shader's lookup.
//!
//! Sizing: 4 bytes per grid point. A 256-resolution terrain is 256 KiB, a
//! 1024 one 4 MiB. One texture per terrain holds until a terrain outgrows
//! `maxTextureDimension2D` (8192 on the wgpu baseline).

use crate::control::Control;
use crate::rect::GridRect;

/// Grid point whose cell contains UV coordinate `uv`.
///
/// `uv` is the mesher's UV0: `0.0` at grid point 0, `1.0` at grid point
/// `resolution - 1`. Scaling back up gives a continuous grid coordinate,
/// and its floor is the cell's lower corner, so a control word covers the
/// cell starting at its own grid point.
///
/// What a brush or a picker reads to answer what is painted under a
/// point; it clamps to the last grid point. It is not the shader's
/// bilinear base corner, which stops one short so its upper neighbour
/// stays in range. See [`control_corner`].
pub fn control_cell(uv: f32, resolution: u32) -> u32 {
    if resolution == 0 {
        return 0;
    }
    let last = resolution - 1;
    let scaled = scaled_grid(uv, resolution);
    if scaled >= last as f32 {
        return last;
    }
    scaled as u32
}

/// The shader's bilinear base corner and the fraction into its cell.
///
/// The CPU mirror of `control_corner` in `terrain_splat.wgsl`: the four
/// texels a fragment mixes are this corner and its `+1` neighbours, so
/// the corner clamps to `resolution - 2` rather than `resolution - 1`.
/// The returned fraction is the bilinear `f`, in `0..=1`.
pub fn control_corner(uv: f32, resolution: u32) -> (u32, f32) {
    if resolution < 2 {
        return (0, 0.0);
    }
    let last_corner = resolution - 2;
    let scaled = scaled_grid(uv, resolution);
    let corner = if scaled >= last_corner as f32 {
        last_corner
    } else {
        scaled as u32
    };
    (corner, (scaled - corner as f32).clamp(0.0, 1.0))
}

/// UV0 scaled back to a continuous grid coordinate, with everything
/// outside the terrain pinned to its near edge.
fn scaled_grid(uv: f32, resolution: u32) -> f32 {
    if !uv.is_finite() || uv <= 0.0 {
        return 0.0;
    }
    uv * (resolution - 1) as f32
}

/// UV0 the mesher writes for grid point `g`, as the mesher computes it.
pub fn grid_uv(g: u32, resolution: u32) -> f32 {
    if resolution < 2 {
        return 0.0;
    }
    g as f32 / (resolution - 1) as f32
}

/// Which of a control word's two layers the shader accumulates.
///
/// The CPU mirror of `accumulate_control`'s two guards in
/// `terrain_splat.wgsl`. Between them they must cover every word: a word
/// neither guard admits accumulates nothing, and the fragment normalizes
/// zero by zero into a black texel and an indeterminate normal. Skipping
/// the overlay when it names the base's own id is an optimization,
/// blending a texture with itself being that texture, so it must not
/// apply at full overlay, where the base is already skipped.
pub fn layers_covered(control: Control) -> (bool, bool) {
    let blend = control.blend() as f32 / 255.0;
    let base = blend < 1.0;
    let overlay = blend >= 1.0 || (blend > 0.0 && control.overlay_id() != control.base_id());
    (base, overlay)
}

/// A terrain's control map as raw texels, ready to upload.
///
/// Row-major, `resolution * resolution` entries, matching the layout the
/// heightmap and every other per-cell array in this crate uses.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ControlTexels {
    pub resolution: u32,
    pub texels: Vec<u32>,
}

impl ControlTexels {
    /// Build from a dense control layer.
    ///
    /// A layer shorter than `resolution^2`, which is what an unpainted
    /// terrain has, pads with the default word, so such a terrain uploads
    /// a texture of texture id 0 rather than failing.
    pub fn from_control(control: &[Control], resolution: u32) -> Self {
        let cells = (resolution as usize) * (resolution as usize);
        let mut texels = Vec::with_capacity(cells);
        texels.extend(control.iter().take(cells).map(|c| c.to_raw()));
        texels.resize(cells, Control::default().to_raw());
        Self { resolution, texels }
    }

    /// The word at a grid point, or the default outside the map.
    pub fn get(&self, gx: u32, gz: u32) -> Control {
        if gx >= self.resolution || gz >= self.resolution {
            return Control::default();
        }
        let index = (gz as usize) * (self.resolution as usize) + gx as usize;
        Control::from_raw(self.texels[index])
    }

    /// Bytes to upload, little-endian, matching `R32Uint`.
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.texels.len() * 4);
        for texel in &self.texels {
            bytes.extend_from_slice(&texel.to_le_bytes());
        }
        bytes
    }
}

/// Rewrite one rectangle of an already-built texel buffer.
///
/// `bytes` is the row-major, little-endian `R32Uint` buffer
/// [`ControlTexels::to_bytes`] produces. An editor that keeps the buffer
/// between frames pays the brush footprint to keep it current rather than
/// walking every control word in the terrain back into bytes.
///
/// Cells the rect names that either buffer is too short for are skipped.
pub fn write_control_rect(bytes: &mut [u8], resolution: u32, rect: GridRect, control: &[Control]) {
    for row in rect.rows(resolution) {
        for index in row {
            let at = index * 4;
            if at + 4 > bytes.len() {
                break;
            }
            let word = control.get(index).copied().unwrap_or_default().to_raw();
            bytes[at..at + 4].copy_from_slice(&word.to_le_bytes());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_grid_points_uv_reads_back_its_own_cell() {
        for resolution in [2u32, 3, 8, 33, 256] {
            for g in 0..resolution {
                let uv = grid_uv(g, resolution);
                assert_eq!(
                    control_cell(uv, resolution),
                    g.min(resolution - 1),
                    "resolution {resolution}, grid point {g}, uv {uv}"
                );
            }
        }
    }

    #[test]
    fn a_fragment_inside_a_cell_reads_the_cells_own_lower_corner() {
        let resolution = 9;
        for g in 0..resolution - 1 {
            let lo = grid_uv(g, resolution);
            let hi = grid_uv(g + 1, resolution);
            for t in [0.01, 0.25, 0.5, 0.75, 0.99] {
                let uv = lo + (hi - lo) * t;
                assert_eq!(
                    control_cell(uv, resolution),
                    g,
                    "t {t} inside cell {g} should stay in cell {g}"
                );
            }
        }
    }

    #[test]
    fn the_far_edge_clamps_instead_of_running_off_the_texture() {
        let resolution = 16;
        assert_eq!(control_cell(1.0, resolution), resolution - 1);
        assert_eq!(control_cell(1.5, resolution), resolution - 1);
        assert_eq!(control_cell(-0.5, resolution), 0);
        assert_eq!(control_cell(f32::NAN, resolution), 0);
    }

    /// The bilinear base corner stops one short of the last grid point so
    /// its `+1` neighbour is still on the texture. Its fraction then runs
    /// up to 1.0 across the final cell rather than restarting.
    #[test]
    fn the_bilinear_corner_stops_one_short_so_its_neighbour_stays_in_range() {
        let resolution = 8;
        assert_eq!(control_corner(0.0, resolution), (0, 0.0));
        let (corner, frac) = control_corner(1.0, resolution);
        assert_eq!(corner, resolution - 2);
        assert!((frac - 1.0).abs() < 1e-5, "frac {frac}");
        for g in 0..resolution {
            let (corner, _) = control_corner(grid_uv(g, resolution), resolution);
            assert!(corner + 1 < resolution, "corner {corner} ran off");
        }
    }

    #[test]
    fn the_corner_fraction_walks_zero_to_one_across_a_cell() {
        let resolution = 9;
        let lo = grid_uv(3, resolution);
        let hi = grid_uv(4, resolution);
        for t in [0.0, 0.25, 0.5, 0.75] {
            let (corner, frac) = control_corner(lo + (hi - lo) * t, resolution);
            assert_eq!(corner, 3);
            assert!((frac - t).abs() < 1e-4, "t {t} gave frac {frac}");
        }
    }

    /// The vertex two chunks share is one grid point, so both chunks feed
    /// the shader the same UV for it and the fragments either side of the
    /// join read adjacent texels: never the same texel twice, never a
    /// texel skipped.
    #[test]
    fn a_chunk_boundary_grid_line_maps_to_adjacent_texels_from_both_sides() {
        let resolution = 65;
        let chunk_size = 32;
        let boundary = chunk_size; // last vertex of chunk 0, first of chunk 1
        let uv = grid_uv(boundary, resolution);

        // Just inside the last cell of the left chunk.
        let step = grid_uv(1, resolution) - grid_uv(0, resolution);
        assert_eq!(control_cell(uv - step * 0.01, resolution), boundary - 1);
        // Just inside the first cell of the right chunk.
        assert_eq!(control_cell(uv + step * 0.01, resolution), boundary);
        // And exactly on the line, deterministically the right-hand cell.
        assert_eq!(control_cell(uv, resolution), boundary);
    }

    /// Every LOD level computes UV0 from the same expression, so a grid
    /// point two levels both have carries the identical float in both and
    /// the join between them needs no half-texel offset.
    #[test]
    fn two_levels_emit_the_same_uv_bits_for_a_grid_point_they_share() {
        use crate::clipmap::{SurfaceMeshData, build_clipmap_mesh_data, clipmap_levels};
        use crate::heightmap::Heightmap;
        use bevy_math::Vec2;

        let resolution = 257;
        let heightmap = Heightmap::new(resolution, Vec2::splat(256.0), 10.0);
        let levels = clipmap_levels(resolution, Vec2::splat(128.0));
        assert!(
            levels.len() >= 2,
            "this terrain needs two levels to compare"
        );
        let fine = build_clipmap_mesh_data(&heightmap, &levels[0], |_, _| true);
        let coarse = build_clipmap_mesh_data(&heightmap, &levels[1], |_, _| true);

        let uv_at = |data: &SurfaceMeshData, gx: u32, gz: u32| {
            data.grid
                .iter()
                .position(|g| *g == [gx, gz])
                .map(|i| data.uvs[i])
        };

        let mut compared = 0;
        for gz in (112..144).step_by(2) {
            for gx in (112..144).step_by(2) {
                let (Some(from_fine), Some(from_coarse)) =
                    (uv_at(&fine, gx, gz), uv_at(&coarse, gx, gz))
                else {
                    continue;
                };
                assert_eq!(
                    from_fine.map(f32::to_bits),
                    from_coarse.map(f32::to_bits),
                    "shared vertex ({gx}, {gz}) must carry identical UV bits"
                );
                assert_eq!(
                    control_cell(from_fine[0], resolution),
                    control_cell(from_coarse[0], resolution),
                    "and must resolve to the same control cell"
                );
                assert_eq!(
                    control_corner(from_fine[0], resolution),
                    control_corner(from_coarse[0], resolution),
                    "and to the same bilinear corner and fraction"
                );
                compared += 1;
            }
        }
        assert!(compared > 0, "the two levels shared no grid point");
    }

    /// Every word the format can encode must reach at least one layer.
    /// Exhaustive over the whole id and blend space: the case of one id
    /// named twice at full overlay is a single plane through it.
    #[test]
    fn every_encodable_control_word_accumulates_at_least_one_layer() {
        for base in 0..=crate::control::MAX_TEXTURE_ID {
            for overlay in 0..=crate::control::MAX_TEXTURE_ID {
                for blend in 0..=u8::MAX {
                    let word = Control::default()
                        .with_base_id(base)
                        .with_overlay_id(overlay)
                        .with_blend(blend);
                    let (uses_base, uses_overlay) = layers_covered(word);
                    assert!(
                        uses_base || uses_overlay,
                        "base {base}, overlay {overlay}, blend {blend} accumulates nothing"
                    );
                }
            }
        }
    }

    /// One id named as both layers at full overlay.
    /// `Control::default().with_blend(255)` is the shortest way to write
    /// it, and a paint tool that sets blend without also setting an
    /// overlay id produces it by accident.
    #[test]
    fn one_id_named_twice_at_full_overlay_draws_that_id() {
        let word = Control::default().with_blend(255);
        assert_eq!(word.base_id(), word.overlay_id());
        assert_eq!(word.blend(), 255);
        assert_eq!(
            layers_covered(word),
            (false, true),
            "the overlay must carry it, since the base is skipped at full blend"
        );

        let explicit = Control::default()
            .with_base_id(7)
            .with_overlay_id(7)
            .with_blend(255);
        assert_eq!(layers_covered(explicit), (false, true));
    }

    #[test]
    fn a_repeated_id_below_full_overlay_is_carried_by_the_base_alone() {
        let word = Control::default()
            .with_base_id(3)
            .with_overlay_id(3)
            .with_blend(128);
        assert_eq!(
            layers_covered(word),
            (true, false),
            "blending a texture with itself is that texture; one sample is enough"
        );
    }

    #[test]
    fn an_unpainted_word_uses_only_its_base() {
        assert_eq!(layers_covered(Control::default()), (true, false));
    }

    #[test]
    fn distinct_ids_mid_blend_use_both_layers() {
        let word = Control::default()
            .with_base_id(1)
            .with_overlay_id(2)
            .with_blend(100);
        assert_eq!(layers_covered(word), (true, true));
    }

    #[test]
    fn an_unpainted_terrain_uploads_all_zero_words() {
        let texels = ControlTexels::from_control(&[], 4);
        assert_eq!(texels.texels, vec![0u32; 16]);
        assert_eq!(texels.get(0, 0), Control::default());
        assert_eq!(texels.get(3, 3).base_id(), 0);
    }

    #[test]
    fn control_words_land_row_major_and_read_back_by_grid_point() {
        let resolution = 3;
        let mut control = vec![Control::default(); 9];
        control[0] = Control::default().with_base_id(1);
        control[5] = Control::default().with_base_id(2); // gx 2, gz 1
        control[8] = Control::default().with_base_id(3); // gx 2, gz 2

        let texels = ControlTexels::from_control(&control, resolution);
        assert_eq!(texels.get(0, 0).base_id(), 1);
        assert_eq!(texels.get(2, 1).base_id(), 2);
        assert_eq!(texels.get(2, 2).base_id(), 3);
        assert_eq!(texels.get(1, 1).base_id(), 0);
    }

    #[test]
    fn a_control_layer_longer_than_the_terrain_is_truncated_not_wrapped() {
        let control = vec![Control::default().with_base_id(7); 100];
        let texels = ControlTexels::from_control(&control, 4);
        assert_eq!(texels.texels.len(), 16);
    }

    /// Patching the rows a brush touched leaves the buffer as a whole
    /// rebuild would, so a terrain painted a stroke at a time does not
    /// drift from what is uploaded.
    #[test]
    fn patching_a_rect_matches_rebuilding_the_whole_buffer() {
        let resolution = 8;
        let mut control = vec![Control::default(); 64];
        let mut bytes = ControlTexels::from_control(&control, resolution).to_bytes();

        let rect = GridRect {
            x: 2,
            z: 3,
            width: 3,
            height: 2,
        };
        for row in rect.rows(resolution) {
            for index in row {
                control[index] = Control::default().with_base_id(5).with_blend(200);
            }
        }
        write_control_rect(&mut bytes, resolution, rect, &control);

        assert_eq!(
            bytes,
            ControlTexels::from_control(&control, resolution).to_bytes(),
        );
    }

    #[test]
    fn bytes_are_little_endian_u32_in_texel_order() {
        let control = vec![
            Control::from_raw(0x0000_0001),
            Control::from_raw(0xDEAD_BEEF),
            Control::default(),
            Control::default(),
        ];
        let bytes = ControlTexels::from_control(&control, 2).to_bytes();
        assert_eq!(bytes.len(), 16);
        assert_eq!(&bytes[0..4], &[0x01, 0x00, 0x00, 0x00]);
        assert_eq!(&bytes[4..8], &[0xEF, 0xBE, 0xAD, 0xDE]);
    }

    /// The manual bit and the reserved bits above it ride to the GPU
    /// untouched: the shader reads the first and ignores the rest, and
    /// nothing on this path may reject or strip either.
    #[test]
    fn the_manual_bit_and_reserved_bits_survive_the_trip_to_texels() {
        let word = Control::from_raw(0x1FFF << 19)
            .with_base_id(3)
            .with_blend(9)
            .with_manual(true);
        let texels = ControlTexels::from_control(&[word], 1);
        assert_eq!(texels.texels[0], word.to_raw());
        assert_eq!(texels.get(0, 0).reserved(), 0x1FFF);
        assert!(texels.get(0, 0).manual());
    }
}
