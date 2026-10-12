// The foliage material's prepass and shadow stages: the stock vertex stage with
// the same lean the main pass gives, and the stock cutout with the same edge-on
// hiding, so depth, normals and shadows sit where the drawn leaves are. The
// prepass and shadow view layouts hold the frame's globals at binding 1.

import jackdaw_surface::shaders::foliage_wind::{side_hidden, wind_offset};
import bevy_pbr::render::{
    mesh_functions,
    mesh_view_bindings::view,
    pbr_functions,
    pbr_prepass_functions,
    view_transformations::position_world_to_clip,
};
import bevy_pbr::prepass::io::{FragmentOutput, Vertex, VertexOutput, decompress_vertex};
import bevy_render::globals::Globals;

@group(0) @binding(1) var<uniform> globals: Globals;

@vertex
fn vertex(packed: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let vertex = decompress_vertex(packed, packed.instance_index);
    let world_from_local = mesh_functions::get_world_from_local(packed.instance_index);

    let planted = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    let up_the_mesh = planted.y - world_from_local[3].y;
    let leaned = planted.xyz + wind_offset(up_the_mesh, planted.xyz, globals.time);
    out.world_position = vec4<f32>(leaned, 1.0);
    out.position = position_world_to_clip(leaned);
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION) {
        out.unclipped_depth = out.position.z;
        out.position.z = min(out.position.z, 1.0);
    }

    @if(VERTEX_UVS_A)
    out.uv = vertex.uv;
    @if(VERTEX_UVS_B)
    out.uv_b = vertex.uv_b;

    @if(NORMAL_PREPASS_OR_DEFERRED_PREPASS) {
        @if(VERTEX_NORMALS)
        out.world_normal = mesh_functions::mesh_normal_local_to_world(
            vertex.normal, packed.instance_index);
        @if(VERTEX_TANGENTS)
        out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
            world_from_local, vertex.tangent, packed.instance_index);
    }

    @if(VERTEX_COLORS)
    out.color = vertex.color;

    @if(MOTION_VECTOR_PREPASS) {
        let previous_from_local = mesh_functions::get_previous_world_from_local(packed.instance_index);
        let previous = mesh_functions::mesh_position_local_to_world(
            previous_from_local, vec4<f32>(vertex.position, 1.0));
        out.previous_world_position = vec4<f32>(
            previous.xyz + wind_offset(up_the_mesh, previous.xyz, globals.time - globals.delta_time),
            1.0,
        );
    }

    @if(VERTEX_OUTPUT_INSTANCE_INDEX)
    out.instance_index = packed.instance_index;
    @if(VISIBILITY_RANGE_DITHER)
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        packed.instance_index, world_from_local[3]);

    return out;
}

// Hide a card turned edge on to the view the pass is drawn from, then cut the
// mesh out as the stock prepass does.
fn cut_out(in: VertexOutput) {
    let orthographic = view.clip_from_view[3].w == 1.0;
    let view_direction = pbr_functions::calculate_view(in.world_position, orthographic);
    if side_hidden(in.world_position.xyz, view_direction, in.position.xy) {
        discard;
    }
    @if(VISIBILITY_RANGE_DITHER)
    pbr_functions::visibility_range_dither(in.position, in.visibility_range_dither);
    pbr_prepass_functions::prepass_sample_color_and_alpha_discard(in);
}

@if(PREPASS_FRAGMENT)
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    cut_out(in);
    var out: FragmentOutput;
    @if(UNCLIPPED_DEPTH_ORTHO_EMULATION)
    out.frag_depth = in.unclipped_depth;
    @if(NORMAL_PREPASS) {
        let normal = pbr_functions::prepare_world_normal(in.world_normal, true, is_front);
        out.normal = vec4(normal * 0.5 + vec3(0.5), 1.0);
    }
    @if(MOTION_VECTOR_PREPASS)
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(
        in.world_position, in.previous_world_position);
    return out;
}

@if(!PREPASS_FRAGMENT)
@fragment
fn fragment(in: VertexOutput) {
    cut_out(in);
}
