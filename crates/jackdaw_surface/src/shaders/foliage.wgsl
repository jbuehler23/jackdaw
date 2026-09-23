// Foliage shading: a standard material whose vertices lean in the scene's
// wind and whose leaves pass light through from behind.
//
// The vertex stage is the stock mesh one with the lean added, gathered toward
// the top of the mesh so a trunk bends at its crown while a leaf card flutters
// whole. The fragment stage cuts the mesh out, tints it up its own height,
// varies it by where it stands and adds the light that comes through it.
//
// `Foliage` on the Rust side computes the same lean, gradient and variation,
// and its tests hold the two together.

#import jackdaw_surface::foliage_wind::{
    foliage, gradient_weight, variation_weight, wind_offset, MIN_EXPONENT, PI,
}
#import bevy_pbr::{
    forward_io::{Vertex, VertexOutput, FragmentOutput},
    mesh_functions,
    mesh_view_bindings::{globals, lights, view},
    mesh_view_types,
    pbr_fragment::pbr_input_from_standard_material,
    pbr_functions,
    pbr_types,
    shadows,
    view_transformations::position_world_to_clip,
}

// Where a mesh's own origin stands, which is the root the height above is
// measured from.
fn root_of(instance_index: u32) -> vec3<f32> {
    return mesh_functions::get_world_from_local(instance_index)[3].xyz;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal, vertex.instance_index);
#endif

#ifdef VERTEX_POSITIONS
    let planted = mesh_functions::mesh_position_local_to_world(
        world_from_local, vec4<f32>(vertex.position, 1.0));
    let up_the_mesh = planted.y - world_from_local[3].y;
    let leaned = planted.xyz + wind_offset(up_the_mesh, planted.xyz, globals.time);
    out.world_position = vec4<f32>(leaned, 1.0);
    out.position = position_world_to_clip(leaned);
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
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    if pbr_input.material.base_color.a < foliage.alpha_cutoff {
        discard;
    }

    let world_position = in.world_position.xyz;
    var up_the_mesh = world_position.y;
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    up_the_mesh = world_position.y - root_of(in.instance_index).y;
#endif
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
