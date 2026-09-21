//! Turning images into terrain: heights from a greyscale heightmap,
//! control words from one greyscale weight image per material slot, and
//! per-cell channel values from one image per paint channel or mask.
//!
//! Plain data with no engine types in it: a host decodes the file and hands
//! the samples here, so the mapping from pixels to ground is testable
//! without a renderer or an asset server.
//!
//! An image is addressed from its top-left corner, and its width and height
//! are stretched across the terrain's grid whatever they are: a row of
//! pixels is a row of grid points along +X, and the first row lands on
//! `z = 0`.

use crate::control::{Control, MAX_BLEND, MAX_TEXTURE_ID};

/// A greyscale image as samples in `0..1`, row-major from the top-left.
#[derive(Clone, Debug, PartialEq)]
pub struct GreyImage {
    width: u32,
    height: u32,
    samples: Vec<f32>,
}

/// Why an image could not be read as a terrain layer.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ImportError {
    /// The image has no pixels on one of its axes.
    Empty,
    /// The sample count does not match the stated width and height.
    SampleCount { expected: usize, found: usize },
}

impl core::fmt::Display for ImportError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Empty => write!(f, "the image has no pixels"),
            Self::SampleCount { expected, found } => write!(
                f,
                "the image states {expected} pixels but carries {found} samples"
            ),
        }
    }
}

impl core::error::Error for ImportError {}

impl GreyImage {
    /// Wrap decoded samples, checking they fill the stated rectangle.
    ///
    /// Samples outside `0..1` are clamped and a non-finite one reads as
    /// black, so nothing downstream divides by or lerps through a NaN.
    pub fn new(width: u32, height: u32, samples: Vec<f32>) -> Result<Self, ImportError> {
        if width == 0 || height == 0 {
            return Err(ImportError::Empty);
        }
        let expected = width as usize * height as usize;
        if samples.len() != expected {
            return Err(ImportError::SampleCount {
                expected,
                found: samples.len(),
            });
        }
        let samples = samples
            .into_iter()
            .map(|s| {
                if s.is_finite() {
                    s.clamp(0.0, 1.0)
                } else {
                    0.0
                }
            })
            .collect();
        Ok(Self {
            width,
            height,
            samples,
        })
    }

    /// Pixels across.
    pub fn width(&self) -> u32 {
        self.width
    }

    /// Pixels down.
    pub fn height(&self) -> u32 {
        self.height
    }

    /// The sample at a pixel, clamped to the edge outside the image.
    fn texel(&self, x: i64, y: i64) -> f32 {
        let x = x.clamp(0, self.width as i64 - 1) as usize;
        let y = y.clamp(0, self.height as i64 - 1) as usize;
        self.samples[y * self.width as usize + x]
    }

    /// Bilinear sample at normalised coordinates, both `0..1`.
    fn sample(&self, u: f32, v: f32) -> f32 {
        let x = u.clamp(0.0, 1.0) * (self.width - 1) as f32;
        let y = v.clamp(0.0, 1.0) * (self.height - 1) as f32;
        let x0 = x.floor();
        let y0 = y.floor();
        let fx = x - x0;
        let fy = y - y0;
        let (x0, y0) = (x0 as i64, y0 as i64);
        let top = self.texel(x0, y0) * (1.0 - fx) + self.texel(x0 + 1, y0) * fx;
        let bottom = self.texel(x0, y0 + 1) * (1.0 - fx) + self.texel(x0 + 1, y0 + 1) * fx;
        top * (1.0 - fy) + bottom * fy
    }

    /// The whole texel nearest normalised coordinates, both `0..1`.
    fn nearest(&self, u: f32, v: f32) -> f32 {
        let x = (u.clamp(0.0, 1.0) * (self.width - 1) as f32).round();
        let y = (v.clamp(0.0, 1.0) * (self.height - 1) as f32).round();
        self.texel(x as i64, y as i64)
    }

    /// The image stretched across a square grid of `resolution` points a
    /// side, row-major along +X then +Z.
    ///
    /// An image already at the grid's resolution comes back sample for
    /// sample: the corners land on the corners and every step between is a
    /// whole texel.
    pub fn resample(&self, resolution: u32) -> Vec<f32> {
        self.grid(resolution, Self::sample)
    }

    /// The image stretched across the same grid taking whole texels rather
    /// than blending between them.
    ///
    /// What a mask of discrete values wants: a blend between two of them
    /// means a third value nothing drew, and a drawn edge walks a cell or
    /// so outward as the blend fades. The nearest texel keeps the shape
    /// that was drawn.
    pub fn resample_nearest(&self, resolution: u32) -> Vec<f32> {
        self.grid(resolution, Self::nearest)
    }

    /// Walk the grid, reading the image at each point however `read` reads
    /// it.
    fn grid(&self, resolution: u32, read: impl Fn(&Self, f32, f32) -> f32) -> Vec<f32> {
        if resolution == 0 {
            return Vec::new();
        }
        if resolution == 1 {
            return vec![read(self, 0.0, 0.0)];
        }
        let last = (resolution - 1) as f32;
        let mut out = Vec::with_capacity(resolution as usize * resolution as usize);
        for z in 0..resolution {
            let v = z as f32 / last;
            for x in 0..resolution {
                out.push(read(self, x as f32 / last, v));
            }
        }
        out
    }
}

/// World heights black and white map onto.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct HeightRange {
    /// The height a black pixel stands at.
    pub min: f32,
    /// The height a white pixel stands at.
    pub max: f32,
}

impl HeightRange {
    /// A range from the height its black end stands at and the height its
    /// white end does.
    pub fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    /// The world height a `0..1` sample stands at.
    fn height_of(&self, sample: f32) -> f32 {
        self.min + (self.max - self.min) * sample
    }
}

/// Heights for a square grid of `resolution` points a side, read off a
/// heightmap stretched across it.
pub fn heights_from_image(image: &GreyImage, resolution: u32, range: HeightRange) -> Vec<f32> {
    image
        .resample(resolution)
        .into_iter()
        .map(|sample| range.height_of(sample))
        .collect()
}

/// One material slot's weight image: the id it paints and its samples
/// already stretched across the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct SlotWeights {
    /// Texture id this image paints.
    pub slot: u8,
    /// Weight per grid point, row-major, `0..1`.
    pub weights: Vec<f32>,
}

/// Paint control words from per-slot weights.
///
/// A cell's listed weights are normalised where they sum past one, and the
/// two strongest become its base and overlay with the blend between them;
/// nothing else in the word changes. A cell no listed image claims at all
/// keeps the word it had, so importing one slot's weights paints only where
/// that image is not black and leaves the rest of the terrain as it was.
///
/// Every painted cell is claimed by hand ([`Control::manual`]): the weights
/// came from outside and autoterrain must not overrule them.
pub fn paint_weights(control: &mut [Control], layers: &[SlotWeights]) {
    for (cell, word) in control.iter_mut().enumerate() {
        let mut first: Option<(u8, f32)> = None;
        let mut second: Option<(u8, f32)> = None;
        let mut sum = 0.0;
        for layer in layers {
            let Some(weight) = layer.weights.get(cell).copied() else {
                continue;
            };
            if !weight.is_finite() || weight <= 0.0 {
                continue;
            }
            sum += weight;
            let slot = layer.slot.min(MAX_TEXTURE_ID);
            if first.is_none_or(|(_, top)| weight > top) {
                second = first;
                first = Some((slot, weight));
            } else if second.is_none_or(|(_, next)| weight > next) {
                second = Some((slot, weight));
            }
        }
        let Some((base, base_weight)) = first else {
            continue;
        };
        let scale = if sum > 1.0 { 1.0 / sum } else { 1.0 };
        let base_weight = base_weight * scale;
        let (overlay, overlay_weight) = second
            .map(|(slot, weight)| (slot, weight * scale))
            .unwrap_or((base, 0.0));
        let share = overlay_weight / (base_weight + overlay_weight).max(f32::MIN_POSITIVE);
        *word = word
            .with_base_id(base)
            .with_overlay_id(overlay)
            .with_blend((share * MAX_BLEND as f32).round() as u8)
            .with_manual(true);
    }
}

/// One paint channel's image: the value it writes and its samples already
/// stretched across the grid.
#[derive(Clone, Debug, PartialEq)]
pub struct ChannelPaint {
    /// Value written wherever the image is not black.
    pub value: u16,
    /// Sample per grid point, row-major, `0..1`.
    pub samples: Vec<f32>,
}

/// Write a channel's value into every cell its image is not black on.
///
/// Black is unpainted and leaves the cell the value it already carried, so
/// one image lays a shape into a channel without erasing what was painted
/// around it, and two images can arrive one after the other.
pub fn paint_channel(values: &mut [u16], paint: &ChannelPaint) {
    for (cell, value) in values.iter_mut().enumerate() {
        let Some(sample) = paint.samples.get(cell).copied() else {
            continue;
        };
        if sample > 0.0 {
            *value = paint.value;
        }
    }
}

/// Lay a mask over a channel of continuous cover: every cell takes its
/// sample scaled to `ceiling`.
///
/// Black included, unlike [`paint_channel`]: an image of where something
/// grows is the whole mask, and the ground it leaves black is ground it
/// says nothing grows on.
pub fn paint_mask(values: &mut [u16], samples: &[f32], ceiling: u16) {
    for (cell, value) in values.iter_mut().enumerate() {
        let Some(sample) = samples.get(cell).copied() else {
            continue;
        };
        *value = (sample.clamp(0.0, 1.0) * ceiling as f32).round() as u16;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn image(width: u32, height: u32, samples: &[f32]) -> GreyImage {
        GreyImage::new(width, height, samples.to_vec()).expect("well formed")
    }

    #[test]
    fn an_image_at_the_grids_resolution_resamples_sample_for_sample() {
        let source = [0.0, 0.25, 0.5, 1.0];
        let resampled = image(2, 2, &source).resample(2);
        assert_eq!(resampled, source);
    }

    #[test]
    fn a_smaller_image_stretches_across_the_grid_with_its_corners_on_the_corners() {
        let resampled = image(2, 2, &[0.0, 1.0, 0.0, 1.0]).resample(3);
        assert_eq!(resampled, vec![0.0, 0.5, 1.0, 0.0, 0.5, 1.0, 0.0, 0.5, 1.0]);
    }

    #[test]
    fn a_larger_image_resamples_down_to_the_grids_resolution() {
        let source: Vec<f32> = (0..16).map(|i| (i % 4) as f32 / 3.0).collect();
        let resampled = image(4, 4, &source).resample(2);
        assert_eq!(resampled.len(), 4);
        assert_eq!(resampled[0], 0.0);
        assert_eq!(resampled[1], 1.0);
    }

    /// A blend between two drawn values is a third value nothing drew, and
    /// it walks the edge outward. Whole texels keep the shape.
    #[test]
    fn a_nearest_resample_keeps_an_edge_where_it_was_drawn() {
        let drawn = image(2, 1, &[0.0, 1.0]);
        let row = [0.0, 0.0, 1.0, 1.0];
        assert_eq!(
            drawn.resample_nearest(4),
            row.iter().cycle().take(16).copied().collect::<Vec<f32>>(),
        );
        assert!(
            drawn.resample(4).iter().any(|s| *s > 0.0 && *s < 1.0),
            "the blend is what this avoids",
        );
    }

    #[test]
    fn the_height_range_puts_black_at_its_floor_and_white_at_its_ceiling() {
        let heights =
            heights_from_image(&image(2, 1, &[0.0, 1.0]), 2, HeightRange::new(10.0, 60.0));
        assert_eq!(heights[0], 10.0);
        assert_eq!(heights[1], 60.0);
    }

    #[test]
    fn an_image_that_does_not_fill_its_rectangle_is_refused() {
        assert_eq!(
            GreyImage::new(2, 2, vec![0.0; 3]),
            Err(ImportError::SampleCount {
                expected: 4,
                found: 3
            })
        );
        assert_eq!(GreyImage::new(0, 4, Vec::new()), Err(ImportError::Empty));
    }

    #[test]
    fn a_single_weight_image_claims_the_cells_it_is_not_black_on() {
        let mut control = vec![Control::default(); 3];
        paint_weights(
            &mut control,
            &[SlotWeights {
                slot: 2,
                weights: vec![0.0, 1.0, 0.5],
            }],
        );
        assert!(!control[0].manual(), "black claims nothing");
        assert_eq!(control[1].base_id(), 2);
        assert_eq!(control[1].blend(), 0, "one weight is a pure base");
        assert!(control[1].manual());
        assert_eq!(control[2].base_id(), 2);
    }

    #[test]
    fn two_weights_blend_in_proportion_and_the_stronger_is_the_base() {
        let mut control = vec![Control::default(); 1];
        paint_weights(
            &mut control,
            &[
                SlotWeights {
                    slot: 0,
                    weights: vec![0.75],
                },
                SlotWeights {
                    slot: 1,
                    weights: vec![0.25],
                },
            ],
        );
        assert_eq!(control[0].base_id(), 0);
        assert_eq!(control[0].overlay_id(), 1);
        assert_eq!(control[0].blend(), (0.25 * MAX_BLEND as f32).round() as u8);
    }

    #[test]
    fn weights_summing_past_one_normalise_to_the_same_blend() {
        let mut over = vec![Control::default(); 1];
        paint_weights(
            &mut over,
            &[
                SlotWeights {
                    slot: 3,
                    weights: vec![1.0],
                },
                SlotWeights {
                    slot: 4,
                    weights: vec![1.0],
                },
            ],
        );
        assert_eq!(over[0].base_id(), 3);
        assert_eq!(over[0].overlay_id(), 4);
        assert_eq!(over[0].blend(), (0.5 * MAX_BLEND as f32).round() as u8);
    }

    #[test]
    fn a_channel_image_writes_its_value_where_it_is_not_black() {
        let mut values = vec![0, 0, 9, 0];
        paint_channel(
            &mut values,
            &ChannelPaint {
                value: 3,
                samples: vec![0.0, 1.0, 0.0, 0.4],
            },
        );
        assert_eq!(values, vec![0, 3, 9, 3], "black kept what the cell had");
    }

    #[test]
    fn a_mask_image_scales_its_samples_across_the_channels_ceiling() {
        let mut values = vec![255, 255, 255];
        paint_mask(&mut values, &[0.0, 0.5, 1.0], 255);
        assert_eq!(
            values,
            vec![0, 128, 255],
            "black is bare ground rather than ground left alone",
        );
    }
}
