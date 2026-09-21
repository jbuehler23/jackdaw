// Water shading: a standard material whose vertices ride a wave, whose colour
// deepens with how far behind the surface the bed lies, and which gathers foam
// where the two meet.
//
// The depth behind the surface comes from the camera's depth prepass. Without
// one there is nothing to compare against, so every fragment is treated as
// fully deep: the deep colour, no shore fade and no foam.
//
// `Water` on the Rust side computes the same wave, depth blend and foam
// weight, and its tests hold the two together.

#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::{globals, view},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions,
    prepass_utils,
    view_transformations::{depth_ndc_to_view_z, position_world_to_clip},
}

const TAU: f32 = 6.2831855;
// How much of the wave the long octave carries, the short one taking the rest.
const LONG_WAVE_SHARE: f32 = 0.65;
// How many times shorter the second octave is than the first.
const SHORT_WAVE_RATE: f32 = 2.3;
// How far apart the two ripple samples are turned, in radians, so neither the
// pattern nor its drift lines up with the other.
const RIPPLE_CROSSING: f32 = 2.1;
// How much shorter the second ripple sample is than the first.
const RIPPLE_RATE: f32 = 1.7;
// How many pixels a refraction strength of 1 moves the depth read by.
const REFRACTION_PIXELS: f32 = 24.0;
// How many tiles of the ripple pattern away the surface settles flat again.
// Past that the pattern is finer than a pixel and would alias into bands
// marching across the distance.
const SETTLE_TILES: f32 = 10.0;
// The roughness a fully smooth surface reaches, below which Bevy's lighting
// breaks down.
const MIN_ROUGHNESS: f32 = 0.089;

struct WaterUniform {
    shallow_color: vec4<f32>,
    deep_color: vec4<f32>,
    caustics_color: vec4<f32>,
    normal_heading: vec2<f32>,
    depth_distance: f32,
    edge_fade: f32,
    normal_scale: f32,
    normal_strength: f32,
    normal_speed: f32,
    refraction_strength: f32,
    foam_scale: f32,
    foam_speed: f32,
    foam_distance: f32,
    foam_strength: f32,
    caustics_scale: f32,
    caustics_speed: f32,
    wave_height: f32,
    wave_scale: f32,
    wave_speed: f32,
    smoothness: f32,
    flags: u32,
}

// A map that is not bound falls back to the blank texture, which reads as a
// flat sheet with no foam, so each one says whether it is there and the shader
// draws its own pattern instead.
const NORMAL_MAP_BOUND: u32 = 1u;
const FOAM_MASK_BOUND: u32 = 2u;

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> water: WaterUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var normal_map_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var water_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var foam_mask: texture_2d<f32>;

// How far the wave lifts a point standing at `world`, in world units.
fn wave_offset(world: vec3<f32>, time: f32) -> f32 {
    if water.wave_height == 0.0 {
        return 0.0;
    }
    let travelling = time * water.wave_speed;
    let on_the_plane = vec2<f32>(world.x, world.z) / max(water.wave_scale, 1e-4);
    let long = sin(dot(on_the_plane, vec2<f32>(0.94, 0.34)) * TAU + travelling * TAU);
    let short = sin(
        dot(on_the_plane, vec2<f32>(-0.37, 0.93)) * TAU * SHORT_WAVE_RATE - travelling * TAU);
    return water.wave_height * (long * LONG_WAVE_SHARE + short * (1.0 - LONG_WAVE_SHARE));
}

// How steeply the wave rises along X and along Z where it lifts a point, read
// off the wave itself a short step to either side so the slope always matches
// the height the vertex stage used.
fn wave_slope(world: vec3<f32>, time: f32) -> vec2<f32> {
    let step = max(water.wave_scale, 1e-4) * 0.02;
    let along_x = wave_offset(world + vec3<f32>(step, 0.0, 0.0), time)
        - wave_offset(world - vec3<f32>(step, 0.0, 0.0), time);
    let along_z = wave_offset(world + vec3<f32>(0.0, 0.0, step), time)
        - wave_offset(world - vec3<f32>(0.0, 0.0, step), time);
    return vec2<f32>(along_x, along_z) / (2.0 * step);
}

// How far toward the deep colour a fragment is, with the bed `depth` behind
// the surface.
fn depth_blend(depth: f32) -> f32 {
    return clamp(depth / max(water.depth_distance, 1e-4), 0.0, 1.0);
}

// How much foam gathers where the bed is `depth` behind the surface.
fn foam_weight(depth: f32) -> f32 {
    let reached = clamp(depth / max(water.foam_distance, 1e-4), 0.0, 1.0);
    return water.foam_strength * (1.0 - reached);
}

// A ripple the shader draws itself, for a material with no normal map: two
// crossing sine trains, as a slope along the plane.
fn drawn_ripple(uv: vec2<f32>) -> vec2<f32> {
    let first = cos(uv.x * TAU + uv.y * TAU * 0.4);
    let second = cos(uv.y * TAU * 1.3 - uv.x * TAU * 0.7);
    return vec2<f32>(first * 0.5 + second * 0.2, second * 0.5 - first * 0.2);
}

// The slope the ripples put on the surface, as a displacement along X and Z.
//
// Two samples of the same pattern crossing each other at different scales and
// speeds, averaged, so the surface never reads as one sheet sliding.
fn ripple_slope(world: vec3<f32>, time: f32) -> vec2<f32> {
    let heading = water.normal_heading;
    let across = vec2<f32>(
        heading.x * cos(RIPPLE_CROSSING) - heading.y * sin(RIPPLE_CROSSING),
        heading.x * sin(RIPPLE_CROSSING) + heading.y * cos(RIPPLE_CROSSING),
    );
    let on_the_plane = vec2<f32>(world.x, world.z) / max(water.normal_scale, 1e-4);
    let first_uv = on_the_plane - heading * time * water.normal_speed;
    let second_uv = on_the_plane * RIPPLE_RATE - across * time * water.normal_speed * 0.7;

    let sampled = (textureSample(normal_map_texture, water_sampler, first_uv).xy
        + textureSample(normal_map_texture, water_sampler, second_uv).xy) - 1.0;
    let drawn = (drawn_ripple(first_uv) + drawn_ripple(second_uv)) * 0.5;
    if (water.flags & NORMAL_MAP_BOUND) == 0u {
        return drawn;
    }
    return sampled;
}

// How much of the foam pattern reaches a fragment, `0..1`.
fn foam_pattern(world: vec3<f32>, time: f32) -> f32 {
    let on_the_plane = vec2<f32>(world.x, world.z) / max(water.foam_scale, 1e-4);
    let drift = on_the_plane - water.normal_heading * time * water.foam_speed;
    let sampled = textureSample(foam_mask, water_sampler, drift).r;
    let drawn = clamp(
        0.55 + 0.45 * sin(drift.x * TAU + sin(drift.y * TAU * 1.6) * 1.2),
        0.0,
        1.0,
    );
    if (water.flags & FOAM_MASK_BOUND) == 0u {
        return drawn;
    }
    return sampled;
}

// The light the surface focuses onto the bed, as a bright web travelling over
// it.
fn caustics_pattern(world: vec3<f32>, time: f32) -> f32 {
    let on_the_plane = vec2<f32>(world.x, world.z) / max(water.caustics_scale, 1e-4);
    let travelled = time * water.caustics_speed;
    let first = sin(on_the_plane.x * TAU + travelled * TAU)
        + sin(on_the_plane.y * TAU * 1.1 - travelled * TAU * 0.8);
    let second = sin((on_the_plane.x + on_the_plane.y) * TAU * 0.7 + travelled * TAU * 1.3);
    let web = abs(first * 0.5 + second * 0.5);
    return pow(clamp(1.0 - web, 0.0, 1.0), 3.0);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var slope = vec2<f32>(0.0);

#ifdef VERTEX_POSITIONS
    let flat_position = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    let lifted = flat_position.xyz
        + vec3<f32>(0.0, wave_offset(flat_position.xyz, globals.time), 0.0);
    slope = wave_slope(flat_position.xyz, globals.time);
    out.world_position = vec4<f32>(lifted, 1.0);
    out.position = position_world_to_clip(lifted);
#endif

#ifdef VERTEX_NORMALS
    let resting = mesh_functions::mesh_normal_local_to_world(
        vertex.normal, vertex.instance_index);
    out.world_normal = normalize(resting + vec3<f32>(-slope.x, 0.0, -slope.y));
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local, vertex.tangent, vertex.instance_index);
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif

    return out;
}

// How far behind the surface the bed lies, in world units. `bent` moves the
// pixel the depth is read at, which is what makes the shallows and the foam
// wobble with the ripples.
fn depth_behind(frag_coord: vec4<f32>, bent: vec2<f32>) -> f32 {
#ifdef DEPTH_PREPASS
    let at = vec4<f32>(frag_coord.xy + bent, frag_coord.z, frag_coord.w);
    let behind = depth_ndc_to_view_z(prepass_utils::prepass_depth(at, 0u));
    let here = depth_ndc_to_view_z(frag_coord.z);
    return max(here - behind, 0.0);
#else
    return water.depth_distance;
#endif
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let world_position = in.world_position.xyz;
    let settled = clamp(
        1.0 - distance(view.world_position, world_position)
            / (max(water.normal_scale, 1e-4) * SETTLE_TILES),
        0.0,
        1.0,
    );
    let rippled = ripple_slope(world_position, globals.time) * settled;
    let facing = normalize(pbr_input.world_normal);
    pbr_input.N = normalize(
        facing + vec3<f32>(-rippled.x, 0.0, -rippled.y) * water.normal_strength);

    let bent = rippled * water.refraction_strength * REFRACTION_PIXELS;
    let depth = depth_behind(in.position, bent);
    let deepened = depth_blend(depth);

    let tinted = mix(water.shallow_color, water.deep_color, deepened);
    let foam = clamp(foam_weight(depth) * foam_pattern(world_position, globals.time), 0.0, 1.0);
    let faded = clamp(depth / max(water.edge_fade, 1e-4), 0.0, 1.0);

    let base = pbr_input.material.base_color;
    pbr_input.material.base_color = vec4<f32>(
        mix(tinted.rgb * base.rgb, vec3<f32>(1.0), foam),
        mix(tinted.a * base.a * faded, 1.0, foam),
    );
    pbr_input.material.metallic = 0.0;
    pbr_input.material.perceptual_roughness = clamp(
        1.0 - water.smoothness, MIN_ROUGHNESS, 1.0);

    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr_input);
    let lit = caustics_pattern(world_position, globals.time)
        * settled * (1.0 - deepened) * (1.0 - foam);
    out.color = vec4<f32>(
        out.color.rgb + water.caustics_color.rgb * lit * view.exposure,
        out.color.a,
    );
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
