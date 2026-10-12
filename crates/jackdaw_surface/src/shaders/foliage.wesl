// Foliage shading: a standard material whose vertices lean in the scene's
// wind and whose leaves pass light through from behind.
//
// The vertex stage is the stock mesh one with the lean added, gathered toward
// the top of the mesh so a trunk bends at its crown while a leaf card flutters
// whole. The fragment stage hides a card turned edge on, cuts the mesh out,
// tints it up its own height, varies it by where it stands and adds the light
// that comes through it.
//
// `Foliage` on the Rust side computes the same lean, gradient and variation,
// and its tests hold the two together.

import jackdaw_surface::shaders::foliage_wind::{
    foliage, gradient_weight, side_hidden, variation_weight, wind_offset, MIN_EXPONENT, PI,
};
import bevy_pbr::render::{
    forward_io::{Vertex, VertexOutput, FragmentOutput, decompress_vertex},
    mesh_functions,
    mesh_view_bindings::{globals, lights, view},
    mesh_view_types,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions,
    pbr_types,
    shadows,
    view_transformations::position_world_to_clip,
};

// Where a mesh's own origin stands, which is the root the height above is
// measured from.
fn root_of(instance_index: u32) -> vec3<f32> {
    return mesh_functions::get_world_from_local(instance_index)[3].xyz;
}

@vertex
fn vertex(packed: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(packed, packed.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(packed.instance_index);

    @if(VERTEX_NORMALS)
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal, packed.instance_index);

    @if(VERTEX_POSITIONS) {
        let planted = mesh_functions::mesh_position_local_to_world(
            world_from_local, vec4<f32>(vertex.position, 1.0));
        let up_the_mesh = planted.y - world_from_local[3].y;
        let leaned = planted.xyz + wind_offset(up_the_mesh, planted.xyz, globals.time);
        out.world_position = vec4<f32>(leaned, 1.0);
        out.position = position_world_to_clip(leaned);
    }

    @if(VERTEX_UVS_A)
    out.uv = vertex.uv;
    @if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;
    @if(VERTEX_TANGENTS)
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local, vertex.tangent, packed.instance_index);
    @if(VERTEX_COLORS)
    out.color = vertex.color;
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = packed.instance_index;
    @if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        packed.instance_index, world_from_local[3]);

    return out;
}

// The light that comes through a leaf lit from behind, as a term gathered
// around the direction the light travels, bent by the surface normal so a
// leaf's edges glow rather than its whole face.
//
// Divided by pi and scaled by the view's exposure as the lit result already
// has been, so a strength of 1 is as bright as the lit side of the same leaf
// rather than a blown-out white.
fn translucency(pbr_input: pbr_types::PbrInput, base_color: vec3<f32>, shadow: f32) -> vec3<f32> {
    if foliage.translucency_strength <= 0.0 || lights.n_directional_lights == 0u {
        return vec3<f32>(0.0);
    }
    let light = lights.directional_lights[0];
    let through = normalize(
        -light.direction_to_light + pbr_input.N * foliage.translucency_normal_distortion);
    let gathered = pow(
        clamp(dot(pbr_input.V, through), 0.0, 1.0),
        max(foliage.translucency_scattering, MIN_EXPONENT),
    );
    let reaching = gathered * foliage.translucency_direct + foliage.translucency_ambient;
    let shadowed = mix(foliage.translucency_shadow, 1.0, shadow);
    return base_color * light.color.rgb * view.exposure * reaching * shadowed
        * foliage.translucency_strength / PI;
}

// How much of the first directional light reaches this fragment.
fn light_reaching(in: VertexOutput, world_normal: vec3<f32>) -> f32 {
    if lights.n_directional_lights == 0u {
        return 1.0;
    }
    let light = lights.directional_lights[0];
    if (light.flags & mesh_view_types::DIRECTIONAL_LIGHT_FLAGS_SHADOWS_ENABLED_BIT) == 0u {
        return 1.0;
    }
    let view_z = dot(vec4<f32>(
        view.view_from_world[0].z,
        view.view_from_world[1].z,
        view.view_from_world[2].z,
        view.view_from_world[3].z,
    ), in.world_position);
    return shadows::fetch_directional_shadow(
        0u, in.world_position, world_normal, view_z, in.position.xy);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    let orthographic = view.clip_from_view[3].w == 1.0;
    let view_direction = pbr_functions::calculate_view(in.world_position, orthographic);
    if side_hidden(in.world_position.xyz, view_direction, in.position.xy) {
        discard;
    }
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    @if(FOLIAGE_ALPHA_TO_COVERAGE) {
        let alpha = pbr_input.material.base_color.a;
        pbr_input.material.base_color.a =
            (alpha - foliage.alpha_cutoff) / max(fwidth(alpha), 0.0001) + 0.5;
    } @elif(!ALPHA_TO_COVERAGE) {
        if pbr_input.material.base_color.a < foliage.alpha_cutoff {
            discard;
        }
    }

    let world_position = in.world_position.xyz;
    var up_the_mesh = world_position.y;
    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    up_the_mesh = world_position.y - root_of(in.instance_index).y;
    let risen = gradient_weight(up_the_mesh);
    let varied = variation_weight(world_position);
    let tinted = pbr_input.material.base_color.rgb
        * mix(vec3<f32>(1.0), foliage.gradient_color.rgb, risen)
        * mix(vec3<f32>(1.0), foliage.variation_color.rgb, varied);
    pbr_input.material.base_color = vec4<f32>(tinted, pbr_input.material.base_color.a);
    pbr_input.N = normalize(mix(pbr_input.N, vec3<f32>(0.0, 1.0, 0.0), foliage.shading_normal_up));

    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr_input);
    let reaching = light_reaching(in, pbr_input.world_normal);
    out.color = vec4<f32>(
        out.color.rgb + translucency(pbr_input, tinted, reaching),
        out.color.a,
    );
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
