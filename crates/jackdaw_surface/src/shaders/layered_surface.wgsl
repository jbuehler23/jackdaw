// Layered surface shading: a standard material with a second surface blended
// over its upward faces and a detail surface multiplied over the result.
//
// The layer and the detail set are sampled triplanar in world space. A mesh
// exported with UVs laid out for its base texture keeps those UVs for the base
// and takes the two world-space sets over them, so a cliff and the boulder
// beside it wear the same grass at the same scale.
//
// Every texture read sits at the top of `fragment` in uniform control flow: the
// blend below decides how much of each read reaches the surface, never whether
// the read happens.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions,
}

// The narrowest exponent the threshold dial reaches, so a mask raised to zero
// is never flattened to one.
const MIN_THRESHOLD_EXPONENT: f32 = 0.001;
// How sharply a triplanar projection favours the axis a surface faces along.
const PROJECTION_SHARPNESS: f32 = 4.0;

struct LayeredSurfaceUniform {
    layer_color: vec4<f32>,
    detail_color: vec4<f32>,
    layer_uv_scale: f32,
    layer_normal_strength: f32,
    layer_metallic: f32,
    layer_perceptual_roughness: f32,
    detail_uv_scale: f32,
    detail_normal_strength: f32,
    blend_amount: f32,
    blend_power: f32,
    blend_threshold: f32,
    blend_position: f32,
    blend_contrast: f32,
    vertex_color_channel: u32,
    use_vertex_color: u32,
    flags: u32,
}

// A normal map that is not bound falls back to white, which reads as a slant
// rather than as a flat surface, so each one says whether it is there.
const LAYER_NORMAL_MAP_BOUND: u32 = 1u;
const DETAIL_NORMAL_MAP_BOUND: u32 = 2u;

@group(#{MATERIAL_BIND_GROUP}) @binding(100) var<uniform> layered: LayeredSurfaceUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(101) var layer_base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102) var layer_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(103) var layer_normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104) var layer_orm_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(105) var detail_base_color_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(106) var detail_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(107) var detail_normal_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(108) var detail_orm_texture: texture_2d<f32>;

// How much each of the three world planes contributes at a surface normal.
fn projection_weights(normal: vec3<f32>) -> vec3<f32> {
    let raised = pow(abs(normal), vec3<f32>(PROJECTION_SHARPNESS));
    return raised / max(raised.x + raised.y + raised.z, 0.00001);
}

fn triplanar_color(
    texture: texture_2d<f32>,
    texture_sampler: sampler,
    position: vec3<f32>,
    weights: vec3<f32>,
) -> vec4<f32> {
    return textureSample(texture, texture_sampler, position.zy) * weights.x
        + textureSample(texture, texture_sampler, position.xz) * weights.y
        + textureSample(texture, texture_sampler, position.xy) * weights.z;
}

// A tangent-space normal map read on all three planes and folded back into one
// world-space normal, each plane's map turning the surface normal rather than
// replacing it.
fn triplanar_normal(
    texture: texture_2d<f32>,
    texture_sampler: sampler,
    position: vec3<f32>,
    normal: vec3<f32>,
    weights: vec3<f32>,
) -> vec3<f32> {
    let x = textureSample(texture, texture_sampler, position.zy).xyz * 2.0 - 1.0;
    let y = textureSample(texture, texture_sampler, position.xz).xyz * 2.0 - 1.0;
    let z = textureSample(texture, texture_sampler, position.xy).xyz * 2.0 - 1.0;
    let turned_x = vec3<f32>(x.z * sign(normal.x), x.y, x.x);
    let turned_y = vec3<f32>(y.x, y.z * sign(normal.y), y.y);
    let turned_z = vec3<f32>(z.x, z.y, z.z * sign(normal.z));
    let summed = normal
        + turned_x.zyx * weights.x
        + turned_y.xzy * weights.y
        + turned_z.xyz * weights.z;
    return normalize(summed);
}

fn vertex_channel(color: vec4<f32>) -> f32 {
    switch layered.vertex_color_channel {
        case 0u: { return color.r; }
        case 1u: { return color.g; }
        case 3u: { return color.a; }
        default: { return color.b; }
    }
}

// How much of the layer a fragment takes. `LayerBlend::weight` on the Rust side
// computes the same expression.
fn layer_weight(vertex_color: vec4<f32>, up: f32) -> f32 {
    var selector = up;
    if layered.use_vertex_color != 0u {
        let mask = pow(abs(vertex_channel(vertex_color)), layered.blend_position);
        let contrasted = clamp(
            layered.blend_contrast * mask + 0.5 * (1.0 - layered.blend_contrast),
            0.0,
            1.0,
        );
        selector = pow(contrasted, 1.0 - layered.blend_power) * contrasted;
    }
    let lifted = clamp(selector + layered.blend_power, 0.0, 1.0);
    let exponent = MIN_THRESHOLD_EXPONENT
        + layered.blend_threshold * (1.0 - MIN_THRESHOLD_EXPONENT);
    return layered.blend_amount * pow(abs(lifted), exponent);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    let world_position = in.world_position.xyz;
    let facing = normalize(pbr_input.world_normal);
    let weights = projection_weights(facing);
    let layer_position = world_position * layered.layer_uv_scale;
    let detail_position = world_position * layered.detail_uv_scale;

    let layer_albedo = triplanar_color(
        layer_base_color_texture, layer_sampler, layer_position, weights);
    let layer_orm = triplanar_color(
        layer_orm_texture, layer_sampler, layer_position, weights);
    var layer_normal = triplanar_normal(
        layer_normal_texture, layer_sampler, layer_position, facing, weights);
    let detail_albedo = triplanar_color(
        detail_base_color_texture, detail_sampler, detail_position, weights);
    let detail_orm = triplanar_color(
        detail_orm_texture, detail_sampler, detail_position, weights);
    var detail_normal = triplanar_normal(
        detail_normal_texture, detail_sampler, detail_position, facing, weights);
    if (layered.flags & LAYER_NORMAL_MAP_BOUND) == 0u {
        layer_normal = facing;
    }
    if (layered.flags & DETAIL_NORMAL_MAP_BOUND) == 0u {
        detail_normal = pbr_input.N;
    }

    var vertex_color = vec4<f32>(1.0);
#ifdef VERTEX_COLORS
    vertex_color = in.color;
#endif
    let blend = layer_weight(vertex_color, facing.y);

    let under = pbr_input.material.base_color.rgb
        * detail_albedo.rgb * layered.detail_color.rgb;
    let over = layer_albedo.rgb * layered.layer_color.rgb;
    pbr_input.material.base_color = vec4<f32>(
        mix(under, over, blend),
        pbr_input.material.base_color.a,
    );

    pbr_input.material.perceptual_roughness = clamp(
        mix(
            pbr_input.material.perceptual_roughness * detail_orm.g,
            layer_orm.g * layered.layer_perceptual_roughness,
            blend,
        ),
        0.089,
        1.0,
    );
    pbr_input.material.metallic = mix(
        pbr_input.material.metallic * detail_orm.b,
        layer_orm.b * layered.layer_metallic,
        blend,
    );
    pbr_input.diffuse_occlusion *= mix(
        vec3<f32>(detail_orm.r),
        vec3<f32>(layer_orm.r),
        blend,
    );

    let detailed = normalize(mix(
        pbr_input.N, detail_normal, layered.detail_normal_strength));
    pbr_input.N = normalize(mix(
        detailed, layer_normal, blend * layered.layer_normal_strength));

    pbr_input.material.base_color = pbr_functions::alpha_discard(
        pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr_input);
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
