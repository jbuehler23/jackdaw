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
    hide_sides: u32,
    hide_power: f32,
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

// How much of a card facing the view by `facing`, the cosine between its face
// and the view, is drawn when sides hide: 0 edge on, rising to 1 as it turns.
fn side_visibility(facing: f32) -> f32 {
    return clamp((1.0 - (1.0 - abs(facing)) * 2.0) * foliage.hide_power, 0.0, 1.0);
}

// Whether a fragment of a card seen along `view_direction` dithers away
// because the card is turned edge on. The card is the flat face the fragment
// lies on, measured from its screen-space derivatives so bent vertex normals
// do not count.
fn side_hidden(world_position: vec3<f32>, view_direction: vec3<f32>, frag_coord: vec2<f32>) -> bool {
    let face = normalize(cross(dpdy(world_position), dpdx(world_position)));
    if foliage.hide_sides == 0u {
        return false;
    }
    let coords = vec2<u32>(floor(frag_coord)) % 4u;
    let bayer = array<f32, 16>(0.0, 8.0, 2.0, 10.0, 12.0, 4.0, 14.0, 6.0,
        3.0, 11.0, 1.0, 9.0, 15.0, 7.0, 13.0, 5.0);
    let threshold = (bayer[coords.y * 4u + coords.x] + 0.5) / 16.0;
    return side_visibility(dot(view_direction, face)) < threshold;
}
