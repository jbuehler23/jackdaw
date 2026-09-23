// Instanced ground detail with wind, bend and pressers.

#import bevy_pbr::{
    forward_io::FragmentOutput,
    mesh_types,
    mesh_view_bindings::{view, globals},
    pbr_functions,
    pbr_types,
}

const MAX_PRESSERS: u32 = 16u;
/// Darkest the tint byte takes an instance, as a fraction of its own colour.
const DIMMEST: f32 = 0.7;
/// How far a blade's shading normal is bent onto the ground's, so it takes the light the ground under it takes.
/// `DETAIL_NORMAL_BEND` on the Rust side is the same number.
const NORMAL_BEND: f32 = 0.8;
/// Fraction of the cull distance the instances spend shrinking into the ground.
const FADE_BAND: f32 = 0.25;
/// Alpha a textured instance has to clear to draw.
const ALPHA_CUTOFF: f32 = 0.5;
const TAU: f32 = 6.2831855;
/// How far a breeze of strength 1 leans a tip responding at 1, in world units.
/// `DetailLayer::BREEZE_LEAN` on the Rust side is the same number.
const BREEZE_LEAN: f32 = 0.12;
/// How far a tip bobs up and down against how far it leans sideways.
const VERTICAL_SHARE: f32 = 0.333;
/// How much wider than the sway the gust swell is read, and how much slower it
/// travels.
const GUST_SPAN: f32 = 0.25;
const GUST_DRIFT: f32 = 0.5;

struct DetailUniform {
    /// Linear colour at the foot and at the top of an instance. `w` is unused.
    color_base: vec4<f32>,
    color_tip: vec4<f32>,
    /// Which way the scene's wind blows, on the XZ plane.
    wind_direction: vec2<f32>,
    wind_strength: f32,
    wind_gust: f32,
    wind_gust_speed: f32,
    wind_turbulence_scale: f32,
    /// How far this layer goes with that wind, over a blade of grass.
    wind_response: f32,
    bend: f32,
    /// Shortest and tallest an instance stands, in world units.
    height_range: vec2<f32>,
    /// Narrowest and widest it is drawn, over its mesh's own width.
    width_range: vec2<f32>,
    push_strength: f32,
    cull_distance: f32,
    presser_count: u32,
    /// Whether the mesh is the built-in card, a straight strip whose taper the
    /// fragment stage carves. An asset mesh is cut out by its texture instead.
    is_card: u32,
    /// `xyz` is a presser's world position, `w` how far it flattens detail.
    pressers: array<vec4<f32>, 16>,
}

@group(3) @binding(0) var<uniform> detail: DetailUniform;
@group(3) @binding(1) var wind_noise: texture_2d<f32>;
@group(3) @binding(2) var wind_sampler: sampler;
@group(3) @binding(3) var color_texture: texture_2d<f32>;
@group(3) @binding(4) var color_sampler: sampler;

/// Vertex buffer 0 is the layer's mesh, buffer 1 one `DetailInstance` per draw
/// instance, already in world space.
struct Vertex {
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    /// 0 at the foot of the mesh and 1 at its top.
    @location(3) height_fraction: f32,
    @location(4) instance_position: vec3<f32>,
    @location(5) instance_packed: u32,
    /// The xz of the ground normal the placement recorded; zero stands up.
    @location(6) instance_tilt: vec2<f32>,
}

struct DetailOutput {
    @builtin(position) position: vec4<f32>,
    @location(0) world_position: vec4<f32>,
    @location(1) world_normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) color: vec3<f32>,
    @location(4) ground_normal: vec3<f32>,
}

fn rotate_y(v: vec3<f32>, yaw: f32) -> vec3<f32> {
    let s = sin(yaw);
    let c = cos(yaw);
    return vec3<f32>(v.x * c + v.z * s, v.y, v.z * c - v.x * s);
}

/// `v` turned by the rotation that takes straight up onto `up`.
fn tilt_toward(v: vec3<f32>, up: vec3<f32>) -> vec3<f32> {
    let axis = cross(vec3<f32>(0.0, 1.0, 0.0), up);
    let span = length(axis);
    if span < 1e-5 {
        return v;
    }
    let unit_axis = axis / span;
    let angle = atan2(span, clamp(up.y, -1.0, 1.0));
    let c = cos(angle);
    return v * c + cross(unit_axis, v) * sin(angle) + unit_axis * dot(unit_axis, v) * (1.0 - c);
}

/// The wind pattern where an instance stands, in `-1..1`. Sampled at the
/// instance's foot, not per vertex.
///
/// The gust dial mixes in a second, wider reading of the same field travelling
/// the other way, which swells and drops the sway rather than speeding it up.
fn wind_at(world_xz: vec2<f32>) -> f32 {
    let travel = detail.wind_direction * globals.time * detail.wind_gust_speed;
    let uv = world_xz / max(detail.wind_turbulence_scale, 0.001) + travel;
    let sway = textureSampleLevel(wind_noise, wind_sampler, uv, 0.0).r * 2.0 - 1.0;
    let swell = textureSampleLevel(
        wind_noise, wind_sampler, uv * GUST_SPAN - travel * GUST_DRIFT, 0.0).r;
    return sway * mix(1.0, swell, detail.wind_gust);
}

@vertex
fn vertex(in: Vertex) -> DetailOutput {
    let packed = vec4<u32>(
        in.instance_packed & 0xffu,
        (in.instance_packed >> 8u) & 0xffu,
        (in.instance_packed >> 16u) & 0xffu,
        (in.instance_packed >> 24u) & 0xffu,
    );
    let variation = vec4<f32>(packed) / 255.0;

    let up_the_mesh = in.height_fraction;
    let base = in.instance_position;

    let to_viewer = distance(view.world_position, base);
    let fade_start = detail.cull_distance * (1.0 - FADE_BAND);
    let fade = 1.0 - smoothstep(fade_start, detail.cull_distance, to_viewer);

    let height = mix(detail.height_range.x, detail.height_range.y, variation.x) * fade;
    let width = mix(detail.width_range.x, detail.width_range.y, variation.y);
    let yaw = variation.z * TAU;

    let bend_gathered_toward_the_top = up_the_mesh * up_the_mesh * detail.bend;
    let local = vec3<f32>(
        in.position.x * width,
        up_the_mesh * height,
        in.position.z * width + bend_gathered_toward_the_top,
    );
    let normal = normalize(mix(in.normal, vec3<f32>(0.0, 1.0, 0.0), up_the_mesh));

    let tilt = in.instance_tilt;
    let ground_normal = vec3<f32>(tilt.x, sqrt(max(1.0 - dot(tilt, tilt), 0.0)), tilt.y);
    var offset = tilt_toward(rotate_y(local, yaw), ground_normal);
    let world_normal = tilt_toward(rotate_y(normal, yaw), ground_normal);

    let wind = wind_at(base.xz);
    let along = normalize(detail.wind_direction + vec2<f32>(1e-6, 0.0));
    let leaned = wind * detail.wind_strength * detail.wind_response * BREEZE_LEAN * up_the_mesh;
    offset += vec3<f32>(
        along.x * leaned,
        abs(leaned) * VERTICAL_SHARE,
        along.y * leaned,
    );

    for (var i = 0u; i < min(detail.presser_count, MAX_PRESSERS); i += 1u) {
        let presser = detail.pressers[i];
        let away = base - presser.xyz;
        let flat_away = vec2<f32>(away.x, away.z);
        let reach = max(presser.w, 0.001);
        let strength = 1.0 - smoothstep(0.0, reach, length(flat_away));
        if strength > 0.0 {
            let direction = normalize(vec3<f32>(flat_away.x, 0.0, flat_away.y) + vec3<f32>(1e-6, 0.0, 0.0));
            let pushed = strength * detail.push_strength * up_the_mesh;
            offset += direction * pushed;
            let pressed_down = pushed * 0.5;
            offset.y -= pressed_down;
        }
    }

    let world = base + offset;
    let tint = mix(DIMMEST, 1.0, variation.w);

    var out: DetailOutput;
    out.world_position = vec4<f32>(world, 1.0);
    out.position = view.clip_from_world * out.world_position;
    out.world_normal = normalize(world_normal);
    out.uv = in.uv;
    out.color = mix(detail.color_base.rgb, detail.color_tip.rgb, up_the_mesh) * tint;
    out.ground_normal = ground_normal;
    return out;
}

@fragment
fn fragment(in: DetailOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    if detail.is_card != 0u {
        let half_span = 1.0 - in.uv.y * in.uv.y;
        if abs(in.uv.x - 0.5) * 2.0 > half_span {
            discard;
        }
    }
    let sampled = textureSample(color_texture, color_sampler, in.uv);
    if sampled.a < ALPHA_CUTOFF {
        discard;
    }

    let double_sided = true;
    let facing = pbr_functions::prepare_world_normal(in.world_normal, double_sided, is_front);
    let n = normalize(mix(normalize(facing), normalize(in.ground_normal), NORMAL_BEND));

    var pbr_input = pbr_types::pbr_input_new();
    pbr_input.flags = mesh_types::MESH_FLAGS_SHADOW_RECEIVER_BIT;
    pbr_input.material.base_color = vec4<f32>(in.color * sampled.rgb, 1.0);
    pbr_input.material.perceptual_roughness = 0.9;
    pbr_input.material.metallic = 0.0;
    pbr_input.material.flags |= pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = n;
    pbr_input.N = n;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);

    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr_input);
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
