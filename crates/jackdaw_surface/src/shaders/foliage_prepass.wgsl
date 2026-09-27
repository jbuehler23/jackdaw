// The foliage material's prepass and shadow stages: the stock vertex stage with
// the same lean the main pass gives, and the stock cutout with the same edge-on
// hiding, so depth, normals and shadows sit where the drawn leaves are. The
// prepass and shadow view layouts hold the frame's globals at binding 1.

#import jackdaw_surface::foliage_wind::{side_hidden, wind_offset}
#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::view,
    pbr_functions,
    pbr_prepass_functions,
    prepass_io::{FragmentOutput, Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}
#import bevy_render::globals::Globals

@group(0) @binding(1) var<uniform> globals: Globals;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    let planted = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    let up_the_mesh = planted.y - world_from_local[3].y;
    let leaned = planted.xyz + wind_offset(up_the_mesh, planted.xyz, globals.time);
    out.world_position = vec4<f32>(leaned, 1.0);
    out.position = position_world_to_clip(leaned);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal, vertex.instance_index);
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local, vertex.tangent, vertex.instance_index);
#endif
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef MOTION_VECTOR_PREPASS
    let previous_from_local = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let previous = mesh_functions::mesh_position_local_to_world(
        previous_from_local, vec4<f32>(vertex.position, 1.0));
    out.previous_world_position = vec4<f32>(
        previous.xyz + wind_offset(up_the_mesh, previous.xyz, globals.time - globals.delta_time),
        1.0,
    );
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

// Hide a card turned edge on to the view the pass is drawn from, then cut the
// mesh out as the stock prepass does.
fn cut_out(in: VertexOutput) {
    let orthographic = view.clip_from_view[3].w == 1.0;
    let view_direction = pbr_functions::calculate_view(in.world_position, orthographic);
    if side_hidden(in.world_position.xyz, view_direction, in.position.xy) {
        discard;
    }
#ifdef VISIBILITY_RANGE_DITHER
    pbr_functions::visibility_range_dither(in.position, in.visibility_range_dither);
#endif
    pbr_prepass_functions::prepass_alpha_discard(in);
}

#ifdef PREPASS_FRAGMENT
@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    cut_out(in);
    var out: FragmentOutput;
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.frag_depth = in.unclipped_depth;
#endif
#ifdef NORMAL_PREPASS
    let normal = pbr_functions::prepare_world_normal(in.world_normal, true, is_front);
    out.normal = vec4(normal * 0.5 + vec3(0.5), 1.0);
#endif
#ifdef MOTION_VECTOR_PREPASS
    out.motion_vector = pbr_prepass_functions::calculate_motion_vector(
        in.world_position, in.previous_world_position);
#endif
    return out;
}
#else
@fragment
fn fragment(in: VertexOutput) {
    cut_out(in);
}
#endif
