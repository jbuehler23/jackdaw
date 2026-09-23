#import bevy_pbr::mesh_view_bindings::{view, lights, globals}
#import bevy_pbr::utils::coords_to_viewport_uv
#ifdef TONEMAP_IN_SHADER
#import bevy_core_pipeline::tonemapping::tone_mapping
#endif

struct SkyUniform {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    ground: vec4<f32>,
    cloud_color: vec4<f32>,
    cloud_drift: vec2<f32>,
    horizon_softness: f32,
    brightness: f32,
    sun_cos: f32,
    sun_intensity: f32,
    cloud_coverage: f32,
    cloud_scale: f32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;

struct VertexInput {
    @location(0) position: vec3<f32>,
};

struct VertexOutput {
    @builtin(position) position: vec4<f32>,
};

@vertex
fn vertex(in: VertexInput) -> VertexOutput {
    return VertexOutput(vec4<f32>(in.position.xy, 0.0, 1.0));
}

fn ray_direction(position: vec2<f32>) -> vec3<f32> {
    let clip = coords_to_viewport_uv(position, view.viewport) * vec2(2.0, -2.0) + vec2(-1.0, 1.0);
    let homogeneous = view.view_from_clip * vec4(clip, 1.0, 1.0);
    let in_view = homogeneous.xyz / homogeneous.w;
    return normalize((view.world_from_view * vec4(in_view, 0.0)).xyz);
}

fn gradient(up: f32) -> vec3<f32> {
    let reach = 1.0 / max(sky.horizon_softness, 0.01);
    if up >= 0.0 {
        return mix(sky.zenith.rgb, sky.horizon.rgb, pow(1.0 - up, reach));
    }
    return mix(sky.ground.rgb, sky.horizon.rgb, pow(1.0 + up, reach));
}

fn hash(cell: vec2<f32>) -> f32 {
    var p = fract(vec3<f32>(cell.xyx) * 0.1031);
    p += dot(p, p.yzx + 33.33);
    return fract((p.x + p.y) * p.z);
}

fn value_noise(at: vec2<f32>) -> f32 {
    let cell = floor(at);
    let f = fract(at);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash(cell);
    let b = hash(cell + vec2(1.0, 0.0));
    let c = hash(cell + vec2(0.0, 1.0));
    let d = hash(cell + vec2(1.0, 1.0));
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

fn fbm(at: vec2<f32>) -> f32 {
    var sum = 0.0;
    var amplitude = 0.5;
    var p = at;
    for (var octave = 0; octave < 5; octave++) {
        sum += amplitude * value_noise(p);
        p = p * 2.03 + vec2(17.0, 9.0);
        amplitude *= 0.5;
    }
    return sum;
}

fn cloud_cover(direction: vec3<f32>) -> f32 {
    if sky.cloud_coverage <= 0.0 || direction.y <= 0.0 {
        return 0.0;
    }
    let plane = direction.xz / (direction.y + 0.1);
    let at = plane * sky.cloud_scale * 2.0 + sky.cloud_drift * globals.time;
    let edge = 1.0 - sky.cloud_coverage;
    let cover = smoothstep(edge, edge + 0.25, fbm(at));
    return cover * smoothstep(0.0, 0.2, direction.y) * sky.cloud_color.a;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let direction = ray_direction(in.position.xy);
    let scale = sky.brightness * view.exposure;
    var color = gradient(direction.y) * scale;

    if lights.n_directional_lights > 0u {
        let sun = lights.directional_lights[0];
        let facing = dot(direction, sun.direction_to_light);
        let edge = (1.0 - sky.sun_cos) * 0.25;
        let disc = smoothstep(sky.sun_cos - edge, sky.sun_cos + edge, facing);
        color += sun.color.rgb * view.exposure * sky.sun_intensity * disc;
    }

    let cover = cloud_cover(direction);
    color = mix(color, sky.cloud_color.rgb * scale, cover);

    var out = vec4<f32>(color, 1.0);
#ifdef TONEMAP_IN_SHADER
    out = tone_mapping(out, view.color_grading);
#endif
    return out;
}
