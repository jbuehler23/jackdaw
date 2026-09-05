//! Sample buffer and GPU material for the debugger's live sparklines. The
//! `RingBuffer` is a pure FIFO of recent values; `SparklineMaterial` is the
//! `UiMaterial` that plots them. The debug plugin (a later task) embeds the
//! shader and registers `UiMaterialPlugin::<SparklineMaterial>`.

use bevy::prelude::*;
use bevy::reflect::TypePath;
use bevy::render::render_resource::*;
use bevy::shader::ShaderRef;

/// A fixed-capacity FIFO of `f32` samples for a sparkline. Pure, no Bevy.
#[derive(Debug, Clone)]
pub struct RingBuffer {
    data: std::collections::VecDeque<f32>,
    cap: usize,
}

impl RingBuffer {
    pub fn new(cap: usize) -> Self {
        Self {
            data: std::collections::VecDeque::with_capacity(cap),
            cap,
        }
    }

    /// Append a sample, evicting the oldest once at capacity.
    pub fn push(&mut self, v: f32) {
        if self.data.len() == self.cap {
            self.data.pop_front();
        }
        self.data.push_back(v);
    }

    /// The current window, oldest first.
    pub fn samples(&self) -> Vec<f32> {
        self.data.iter().copied().collect()
    }

    /// Smallest sample in the window, or `+inf` when empty.
    pub fn min(&self) -> f32 {
        self.data.iter().copied().fold(f32::INFINITY, f32::min)
    }

    /// Largest sample in the window, or `-inf` when empty.
    pub fn max(&self) -> f32 {
        self.data.iter().copied().fold(f32::NEG_INFINITY, f32::max)
    }

    /// The most recent sample, or `0.0` when empty.
    pub fn last(&self) -> f32 {
        self.data.back().copied().unwrap_or(0.0)
    }
}

/// Number of samples the material can plot at once.
pub const SPARKLINE_WINDOW: usize = 64;

/// Samples packed four per `vec4` for std140 (see the shader).
const SAMPLE_VECS: usize = SPARKLINE_WINDOW / 4;

const SHADER_SPARKLINE_PATH: &str = "embedded://jackdaw/remote/debug/shaders/sparkline.wgsl";

/// GPU material that draws a `RingBuffer`'s window as a filled polyline.
///
/// `samples` holds `SPARKLINE_WINDOW` values packed four per `Vec4`; the shader
/// reads sample `i` at `samples[i / 4][i % 4]` and plots the first `count` of
/// them normalized between `min` and `max` over the node's height.
#[derive(AsBindGroup, Asset, TypePath, Debug, Clone)]
pub struct SparklineMaterial {
    #[uniform(0)]
    pub samples: [Vec4; SAMPLE_VECS],
    #[uniform(0)]
    pub color: Vec4,
    #[uniform(0)]
    pub count: u32,
    #[uniform(0)]
    pub min: f32,
    #[uniform(0)]
    pub max: f32,
}

impl Default for SparklineMaterial {
    fn default() -> Self {
        Self {
            samples: [Vec4::ZERO; SAMPLE_VECS],
            color: Vec4::new(0.4, 0.7, 1.0, 1.0),
            count: 0,
            min: 0.0,
            max: 1.0,
        }
    }
}

impl SparklineMaterial {
    /// A material tinted with `color` and no samples yet.
    pub fn new(color: Color) -> Self {
        Self {
            color: color_to_vec4(color),
            ..default()
        }
    }

    /// Load the most recent `SPARKLINE_WINDOW` samples from `buf` into the
    /// uniforms, updating `count`, `min`, and `max` over the plotted window.
    pub fn set_samples(&mut self, buf: &RingBuffer) {
        self.samples = [Vec4::ZERO; SAMPLE_VECS];
        let all = buf.samples();
        let start = all.len().saturating_sub(SPARKLINE_WINDOW);
        let window = &all[start..];
        for (i, &v) in window.iter().enumerate() {
            self.samples[i / 4][i % 4] = v;
        }
        self.count = window.len() as u32;
        if window.len() >= 2 {
            self.min = window.iter().copied().fold(f32::INFINITY, f32::min);
            self.max = window.iter().copied().fold(f32::NEG_INFINITY, f32::max);
        } else {
            self.min = 0.0;
            self.max = 1.0;
        }
    }
}

impl UiMaterial for SparklineMaterial {
    fn fragment_shader() -> ShaderRef {
        SHADER_SPARKLINE_PATH.into()
    }
}

fn color_to_vec4(color: Color) -> Vec4 {
    let c = color.to_linear();
    Vec4::new(c.red, c.green, c.blue, c.alpha)
}

/// A parent-filling sparkline node tinted with `color`, ready to spawn.
///
/// A `MaterialNode` needs a minted handle, so the caller passes in the material
/// store; the panel still adds a sparkline in a single spawn call.
pub fn sparkline_node(materials: &mut Assets<SparklineMaterial>, color: Color) -> impl Bundle {
    let material = materials.add(SparklineMaterial::new(color));
    (
        Node {
            width: percent(100),
            height: percent(100),
            ..default()
        },
        MaterialNode(material),
    )
}

#[cfg(test)]
mod tests {
    use super::{RingBuffer, SPARKLINE_WINDOW, SparklineMaterial};

    #[test]
    fn push_evicts_oldest_at_capacity() {
        let mut b = RingBuffer::new(3);
        b.push(1.0);
        b.push(2.0);
        b.push(3.0);
        b.push(4.0);
        assert_eq!(b.samples(), vec![2.0, 3.0, 4.0]);
        assert_eq!(b.last(), 4.0);
    }

    #[test]
    fn min_max_over_current_window() {
        let mut b = RingBuffer::new(4);
        for v in [5.0, 1.0, 9.0, 3.0] {
            b.push(v);
        }
        assert_eq!(b.min(), 1.0);
        assert_eq!(b.max(), 9.0);
    }

    #[test]
    fn empty_buffer_is_safe() {
        let b = RingBuffer::new(4);
        assert_eq!(b.samples(), Vec::<f32>::new());
        assert_eq!(b.last(), 0.0);
    }

    #[test]
    fn set_samples_packs_window_and_bounds() {
        let mut b = RingBuffer::new(4);
        for v in [5.0, 1.0, 9.0, 3.0] {
            b.push(v);
        }
        let mut mat = SparklineMaterial::default();
        mat.set_samples(&b);
        assert_eq!(mat.count, 4);
        assert_eq!(mat.min, 1.0);
        assert_eq!(mat.max, 9.0);
        // First four samples packed into the first vec4.
        assert_eq!(mat.samples[0].to_array(), [5.0, 1.0, 9.0, 3.0]);
    }

    #[test]
    fn set_samples_keeps_most_recent_window() {
        let mut b = RingBuffer::new(SPARKLINE_WINDOW * 2);
        for i in 0..(SPARKLINE_WINDOW * 2) {
            b.push(i as f32);
        }
        let mut mat = SparklineMaterial::default();
        mat.set_samples(&b);
        assert_eq!(mat.count as usize, SPARKLINE_WINDOW);
        // Oldest plotted sample is the start of the most recent window.
        assert_eq!(mat.samples[0].x, SPARKLINE_WINDOW as f32);
    }
}
