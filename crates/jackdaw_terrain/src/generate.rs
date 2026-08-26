use noise::{Fbm, MultiFractal, NoiseFn, OpenSimplex, Perlin, RidgedMulti, Simplex};

/// Noise algorithm type.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum NoiseType {
    Perlin,
    Simplex,
    OpenSimplex,
    RidgedMulti,
}

impl NoiseType {
    pub const ALL: &[NoiseType] = &[
        NoiseType::Perlin,
        NoiseType::Simplex,
        NoiseType::OpenSimplex,
        NoiseType::RidgedMulti,
    ];

    pub fn label(self) -> &'static str {
        match self {
            NoiseType::Perlin => "Perlin",
            NoiseType::Simplex => "Simplex",
            NoiseType::OpenSimplex => "OpenSimplex",
            NoiseType::RidgedMulti => "Ridged Multi",
        }
    }

    pub fn index(self) -> usize {
        match self {
            NoiseType::Perlin => 0,
            NoiseType::Simplex => 1,
            NoiseType::OpenSimplex => 2,
            NoiseType::RidgedMulti => 3,
        }
    }

    pub fn from_index(i: usize) -> Self {
        #[expect(clippy::match_same_arms, reason = "one arm per index")]
        match i {
            0 => NoiseType::Perlin,
            1 => NoiseType::Simplex,
            2 => NoiseType::OpenSimplex,
            3 => NoiseType::RidgedMulti,
            _ => NoiseType::Perlin,
        }
    }
}

/// Settings for procedural terrain generation.
#[derive(Clone, Debug)]
pub struct GenerateSettings {
    pub noise_type: NoiseType,
    pub seed: u32,
    pub frequency: f64,
    pub octaves: usize,
    pub lacunarity: f64,
    pub persistence: f64,
    pub amplitude: f32,
    pub offset: f32,
}

impl Default for GenerateSettings {
    fn default() -> Self {
        Self {
            noise_type: NoiseType::Perlin,
            seed: 42,
            frequency: 0.01,
            octaves: 4,
            lacunarity: 2.0,
            persistence: 0.45,
            amplitude: 8.0,
            offset: 0.0,
        }
    }
}

/// Generate a heightmap from noise settings.
///
/// Returns a `Vec<f32>` of length `resolution * resolution`, row-major.
pub fn generate_heightmap(resolution: u32, settings: &GenerateSettings) -> Vec<f32> {
    let len = (resolution * resolution) as usize;
    let mut heights = vec![0.0_f32; len];

    match settings.noise_type {
        NoiseType::Perlin => {
            let noise = Fbm::<Perlin>::new(settings.seed)
                .set_frequency(settings.frequency)
                .set_octaves(settings.octaves)
                .set_lacunarity(settings.lacunarity)
                .set_persistence(settings.persistence);
            fill_heights(&noise, resolution, settings, &mut heights);
        }
        NoiseType::Simplex => {
            let noise = Fbm::<Simplex>::new(settings.seed)
                .set_frequency(settings.frequency)
                .set_octaves(settings.octaves)
                .set_lacunarity(settings.lacunarity)
                .set_persistence(settings.persistence);
            fill_heights(&noise, resolution, settings, &mut heights);
        }
        NoiseType::OpenSimplex => {
            let noise = Fbm::<OpenSimplex>::new(settings.seed)
                .set_frequency(settings.frequency)
                .set_octaves(settings.octaves)
                .set_lacunarity(settings.lacunarity)
                .set_persistence(settings.persistence);
            fill_heights(&noise, resolution, settings, &mut heights);
        }
        NoiseType::RidgedMulti => {
            let noise = RidgedMulti::<Perlin>::new(settings.seed)
                .set_frequency(settings.frequency)
                .set_octaves(settings.octaves)
                .set_lacunarity(settings.lacunarity);
            fill_heights(&noise, resolution, settings, &mut heights);
        }
    }

    heights
}

fn fill_heights(
    noise: &dyn NoiseFn<f64, 2>,
    resolution: u32,
    settings: &GenerateSettings,
    heights: &mut [f32],
) {
    for gz in 0..resolution {
        for gx in 0..resolution {
            let val = noise.get([gx as f64, gz as f64]);
            heights[(gz * resolution + gx) as usize] =
                val as f32 * settings.amplitude + settings.offset;
        }
    }
}
