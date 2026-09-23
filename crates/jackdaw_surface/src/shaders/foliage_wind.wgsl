// The foliage material's uniform and the lean, gradient and variation its
// vertex, prepass and fragment stages share.

#define_import_path jackdaw_surface::foliage_wind

const TAU: f32 = 6.2831855;
const PI: f32 = 3.1415927;
// How far a wind of strength 1 leans a part responding at 1, in world units.
const FOLIAGE_LEAN: f32 = 0.35;
// How far the high-frequency flutter reaches beside the main lean.
const FLUTTER_LEAN: f32 = 0.06;
// How many times faster than the wind itself the flutter travels.
const FLUTTER_RATE: f32 = 7.0;
// The smallest exponent a dial reaches, so a value raised to zero is never
// flattened to one.
const MIN_EXPONENT: f32 = 0.001;

struct FoliageUniform {
    gradient_color: vec4<f32>,
    variation_color: vec4<f32>,
    wind_direction: vec2<f32>,
    wind_strength: f32,
    wind_gust: f32,
    wind_gust_speed: f32,
    wind_turbulence_scale: f32,
    alpha_cutoff: f32,
    gradient_position: f32,
    gradient_falloff: f32,
    gradient_invert: u32,
    variation_strength: f32,
    variation_scale: f32,
    translucency_strength: f32,
    translucency_normal_distortion: f32,
    translucency_scattering: f32,
    translucency_direct: f32,
    translucency_ambient: f32,
    translucency_shadow: f32,
    shading_normal_up: f32,
    wind_response: f32,
    micro_wind_response: f32,
    bend_position: f32,
    bend_contrast: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> foliage: FoliageUniform;

// How far the wind carries a vertex, in world units. `up_the_mesh` is how far
// above its own root the vertex sits and `world` where it stands, which is
// what the pattern is read at so two plants side by side move apart.
fn wind_offset(up_the_mesh: f32, world: vec3<f32>, time: f32) -> vec3<f32> {
    if foliage.wind_strength == 0.0 {
        return vec3<f32>(0.0);
    }
    let heading = foliage.wind_direction;
    let along = vec3<f32>(heading.x, 0.0, heading.y);
    let risen = clamp(up_the_mesh / max(foliage.bend_position, 1e-4), 0.0, 1.0);
    let leaned = pow(risen, max(foliage.bend_contrast, MIN_EXPONENT));

    let travelled = dot(vec2<f32>(world.x, world.z), heading)
        / max(foliage.wind_turbulence_scale, 1e-4)
        - time * foliage.wind_gust_speed;
    let sway = sin(travelled * TAU);
    let swell = 0.5 + 0.5 * sin(travelled * TAU * 0.25 + 1.3);
    let gusted = sway * (1.0 - foliage.wind_gust + foliage.wind_gust * swell);

    let flutter = sin((travelled * FLUTTER_RATE + world.y) * TAU)
        * foliage.micro_wind_response
        * FLUTTER_LEAN;
    return along
        * (gusted * foliage.wind_response * FOLIAGE_LEAN * leaned + flutter * leaned)
        * foliage.wind_strength;
}

// How much of the gradient colour a point sitting `up_the_mesh` above its own
// root takes.
fn gradient_weight(up_the_mesh: f32) -> f32 {
    let risen = clamp(
        (up_the_mesh - foliage.gradient_position) / max(foliage.gradient_falloff, 1e-4),
        0.0,
        1.0,
    );
    if foliage.gradient_invert != 0u {
        return 1.0 - risen;
    }
    return risen;
}

// How much of the variation colour a plant standing at `at` takes.
fn variation_weight(at: vec3<f32>) -> f32 {
    let scaled = at / max(foliage.variation_scale, 1e-4);
    let field = sin(scaled.x * TAU * 0.13 + 1.7) * cos(scaled.z * TAU * 0.11 - 0.4)
        + sin(scaled.z * TAU * 0.07 + 2.3) * 0.5;
    return clamp(0.5 + 0.25 * field, 0.0, 1.0) * clamp(foliage.variation_strength, 0.0, 1.0);
}
