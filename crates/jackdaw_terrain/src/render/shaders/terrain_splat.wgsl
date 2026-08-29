// Terrain splat shading: two texture ids per control texel, blended by
// comparing their height maps.
//
// The control map is `texture_2d<u32>` read with `textureLoad` only: a
// packed control word must never be filtered into a different packed word.
// Four texels are read per fragment and their decoded contributions are
// mixed with bilinear weights, so a painted boundary follows the height
// fields rather than the cell grid.
//
// A terrain with autoterrain on replaces the word of every cell no hand
// has claimed with one its own slope stands for, before any of the
// sampling below. Painted cells keep the word they were painted with.
//
// That slope comes from `slope_map`, which holds the ground's own slope
// per grid point, and never from the surface being drawn. The surface is
// a clipmap: the same ground is meshed at several step sizes, each
// smoothing the relief by a different amount, so a slope taken from the
// mesh normal reads gentler with distance and the ground re-textures
// itself as a level hands over under a moving camera.
//
// Every texture read is `textureSampleGrad` with derivatives taken once in
// uniform control flow at the top of `fragment`. The per-layer branches
// below make implicit-derivative sampling illegal, and the derivative of a
// UV-scaled position is the scaled derivative, so one pair covers every
// layer.

#import bevy_pbr::{
    forward_io::{VertexOutput, FragmentOutput},
    mesh_bindings::mesh,
    mesh_view_bindings::view,
    pbr_functions,
    pbr_types,
}

// The remap of a 0..1 authoring dial onto a power exponent. 4 is a wide,
// soft cross-fade; 64 is a near-binary height cutout.
const SHARPNESS_MIN: f32 = 4.0;
const SHARPNESS_RANGE: f32 = 60.0;

const BASE_SHIFT: u32 = 0u;
const BASE_MASK: u32 = 0x1Fu;
const OVERLAY_SHIFT: u32 = 5u;
const OVERLAY_MASK: u32 = 0x1Fu;
const BLEND_SHIFT: u32 = 10u;
const BLEND_MASK: u32 = 0xFFu;
// A hand painted this cell, so its own word is what it draws.
const MANUAL_BIT: u32 = 0x40000u;

// Entries in `uv_scales`, which is four `vec4`s wide.
const MAX_LAYERS: u32 = 16u;

struct SplatUniform {
    // 16 per-id UV scales, packed four to a vec4 for uniform alignment.
    uv_scales: array<vec4<f32>, 4>,
    // 16 per-id detiling strengths, packed the same way. 0 is off.
    detile_strengths: array<vec4<f32>, 4>,
    // Terrain XZ extent in world units, for turning UV0 back into a
    // terrain-local position. Local, not world, so a terrain that moves
    // carries its texturing with it.
    terrain_size: vec2<f32>,
    blend_sharpness: f32,
    perceptual_roughness: f32,
    control_resolution: u32,
    layer_count: u32,
    // Nonzero where this terrain textures the cells no hand has claimed
    // from their slope. 0 leaves every cell to its own control word.
    autoterrain_enabled: u32,
    autoterrain_base_slot: u32,
    autoterrain_slope_slot: u32,
    // The slope range in radians, converted from the authored degrees on
    // the way in so this side carries no conversion of its own.
    autoterrain_slope_start: f32,
    autoterrain_slope_end: f32,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> splat: SplatUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var albedo_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var layer_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var normal_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(4) var height_array: texture_2d_array<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(5) var control_map: texture_2d<u32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(6) var slope_map: texture_2d<f32>;

// The last id this material can address: one short of the bound set's
// layer count, and never past the UV-scale array. Every id is clamped
// through here exactly once, so the layer sampled and the UV scale
// applied can never disagree.
fn last_layer() -> u32 {
    return min(max(splat.layer_count, 1u), MAX_LAYERS) - 1u;
}

// `id` must already be clamped by `last_layer`.
fn uv_scale_for(id: u32) -> f32 {
    return splat.uv_scales[id / 4u][id % 4u];
}

// `id` must already be clamped by `last_layer`.
fn detile_for(id: u32) -> f32 {
    return splat.detile_strengths[id / 4u][id % 4u];
}

// Two uncorrelated values in 0..1 from a tile coordinate. Integer-valued
// input, so the hash only has to separate whole numbers.
fn tile_hash(tile: vec2<f32>) -> vec2<f32> {
    var h = fract(vec3<f32>(tile.x, tile.y, tile.x + tile.y) * vec3<f32>(0.1031, 0.1030, 0.0973));
    h += dot(h, h.yzx + 33.33);
    return fract((h.xx + h.yz) * h.zy);
}

// A UV and the derivatives that go with it.
struct Sampling {
    uv: vec2<f32>,
    ddx: vec2<f32>,
    ddy: vec2<f32>,
}

// Break a tiled texture out of its grid.
//
// Every tile of the repeat is turned and shifted by its own amount, drawn
// from a hash of its coordinate, breaking up the repeat. Applying one
// tile's transform on its own would seam at every tile border, so the four
// nearest tiles are blended with the bilinear weights of the position
// between their centres: the weights of a tile being left reach zero where
// it is left, so the blended transform is continuous across the plane.
//
// The rotation pivots on the tile centres rather than the texture origin,
// so how far a fragment is displaced does not grow with how far the
// terrain runs.
//
// `strength` 0 returns exactly what came in: a slot with detiling off
// samples the UV its scale alone put it at.
fn detile(uv: vec2<f32>, ddx: vec2<f32>, ddy: vec2<f32>, strength: f32) -> Sampling {
    var out: Sampling;
    out.uv = uv;
    out.ddx = ddx;
    out.ddy = ddy;
    if strength <= 0.0 {
        return out;
    }

    // Half a tile back, so `base` names the four tile centres around the
    // sample rather than the corners of the tile it is inside.
    let p = uv - vec2<f32>(0.5);
    let base = floor(p);
    let f = smoothstep(vec2<f32>(0.0), vec2<f32>(1.0), fract(p));

    var blended = vec2<f32>(0.0);
    var turn = vec2<f32>(0.0);
    for (var j = 0u; j < 2u; j++) {
        for (var i = 0u; i < 2u; i++) {
            let corner = base + vec2<f32>(f32(i), f32(j));
            let weight = mix(1.0 - f.x, f.x, f32(i)) * mix(1.0 - f.y, f.y, f32(j));
            let hash = tile_hash(corner);
            let angle = (hash.x * 2.0 - 1.0) * strength * 3.14159265;
            let shift = (hash - vec2<f32>(0.5)) * strength;
            let pivot = corner + vec2<f32>(0.5);
            let local = uv - pivot;
            let c = cos(angle);
            let s = sin(angle);
            let rotated = vec2<f32>(c * local.x - s * local.y, s * local.x + c * local.y);
            blended += (pivot + rotated + shift) * weight;
            turn += vec2<f32>(c, s) * weight;
        }
    }

    // The blended rotation stands in for the whole Jacobian. The term the
    // varying weights contribute is dropped for cost, though it is the
    // same order as the rotation: tile-scale displacements times O(1)
    // smoothstep gradients. These derivatives pick a mip level, so the
    // error is a fraction of a level.
    //
    // Where the four rotations cancel, at strength near 1 with tiles
    // turned against each other, `turn` collapses toward zero, the
    // derivatives go with it and the sharpest mip is chosen at those fold
    // points. The dropped weight-gradient term is what would keep the true
    // Jacobian non-degenerate there.
    out.uv = blended;
    out.ddx = vec2<f32>(turn.x * ddx.x - turn.y * ddx.y, turn.y * ddx.x + turn.x * ddx.y);
    out.ddy = vec2<f32>(turn.x * ddy.x - turn.y * ddy.y, turn.y * ddy.x + turn.x * ddy.y);
    return out;
}

// The texel a grid coordinate's lower corner sits in. `control_resolution`
// texels cover grid points 0..resolution-1, and the upper corner of the
// bilinear tap is this + 1, so the base clamps one short of the end.
fn control_corner(g: vec2<f32>) -> vec2<i32> {
    let last = i32(max(splat.control_resolution, 2u) - 2u);
    return clamp(vec2<i32>(floor(g)), vec2<i32>(0), vec2<i32>(last));
}

fn load_control(coord: vec2<i32>) -> u32 {
    let last = i32(max(splat.control_resolution, 1u) - 1u);
    let clamped = clamp(coord, vec2<i32>(0), vec2<i32>(last));
    return textureLoad(control_map, clamped, 0).r;
}

fn load_slope(coord: vec2<i32>) -> f32 {
    let last = vec2<i32>(textureDimensions(slope_map)) - vec2<i32>(1);
    let clamped = clamp(coord, vec2<i32>(0), max(last, vec2<i32>(0)));
    return textureLoad(slope_map, clamped, 0).r;
}

// The stored slope under a fragment, bilinear across the four grid points
// around it.
//
// Filtered rather than nearest so the band between the two textures moves
// smoothly across a cell instead of stepping at every grid line, and taken
// on the slope rather than on the two control words it produces so the
// four corners agree wherever the ground is uniformly flat or uniformly
// steep, which keeps the single-word fast path below covering most of a
// terrain.
fn slope_at(corner: vec2<i32>, f: vec2<f32>) -> f32 {
    let s00 = load_slope(corner);
    let s10 = load_slope(corner + vec2<i32>(1, 0));
    let s01 = load_slope(corner + vec2<i32>(0, 1));
    let s11 = load_slope(corner + vec2<i32>(1, 1));
    return mix(mix(s00, s10, f.x), mix(s01, s11, f.x), f.y);
}

struct Accum {
    albedo: vec3<f32>,
    normal: vec3<f32>,
    total: f32,
}

// One texture id's contribution, summed over the corners that named it.
//
// `corner_weights` holds the four bilinear corner shares, with zero in the
// slots that did not name this id at this layer weight. The weight is
//
//     sum over corners of pow(corner_w + layer_w + height, sharpness) * corner_w
//
// which is the height-blend curve: a taller layer wins the contested
// band, and a heavier painted share biases it. `corner_w` rides inside the
// power so a near corner gets a height advantage, and multiplies outside
// it so a far corner cannot win on height alone.
//
// Taking the four corners as a vector rather than one call per corner is
// what makes the uniform-word fast path in `fragment` exact: a zeroed slot
// contributes `pow(k, s) * 0`, so summing the slots here is identical to
// summing four separate calls, and a single call with all four slots
// filled is identical again whenever the four corners name the same id
// with the same layer weight.
fn accumulate(
    accum: Accum,
    id: u32,
    corner_weights: vec4<f32>,
    layer_weight: f32,
    local_xz: vec2<f32>,
    ddx_local: vec2<f32>,
    ddy_local: vec2<f32>,
) -> Accum {
    let layer_id = min(id, last_layer());
    let layer = i32(layer_id);
    let scale = uv_scale_for(layer_id);
    let tiled = detile(
        local_xz * scale, ddx_local * scale, ddy_local * scale, detile_for(layer_id));
    let uv = tiled.uv;
    let ddx = tiled.ddx;
    let ddy = tiled.ddy;

    let albedo = textureSampleGrad(albedo_array, layer_sampler, uv, layer, ddx, ddy).rgb;
    let height = textureSampleGrad(height_array, layer_sampler, uv, layer, ddx, ddy).r;
    let packed = textureSampleGrad(normal_array, layer_sampler, uv, layer, ddx, ddy).rgb;
    let normal = packed * 2.0 - 1.0;

    let sharpness = SHARPNESS_MIN + SHARPNESS_RANGE * splat.blend_sharpness;
    let contested = vec4<f32>(layer_weight + height);
    let weight = dot(pow(corner_weights + contested, vec4<f32>(sharpness)), corner_weights);

    var out = accum;
    out.albedo += albedo * weight;
    out.normal += normal * weight;
    out.total += weight;
    return out;
}

// Both of a control word's layers.
//
// The two guards must between them cover every word, or a fragment
// accumulates nothing and normalizes zero by zero. `blend < 1.0` covers
// the base for every blend but full overlay; the overlay covers the rest.
// The `overlay_id != base_id` skip is an optimization, blending a texture
// with itself being that texture, so it must not apply at full overlay,
// where the base has already been skipped. A word naming the same id twice
// at blend 255 is the case that would otherwise fall through both.
fn accumulate_control(
    accum: Accum,
    control: u32,
    corner_weights: vec4<f32>,
    local_xz: vec2<f32>,
    ddx_local: vec2<f32>,
    ddy_local: vec2<f32>,
) -> Accum {
    let base_id = (control >> BASE_SHIFT) & BASE_MASK;
    let overlay_id = (control >> OVERLAY_SHIFT) & OVERLAY_MASK;
    let blend = f32((control >> BLEND_SHIFT) & BLEND_MASK) / 255.0;

    var out = accum;
    if blend < 1.0 {
        out = accumulate(
            out, base_id, corner_weights, 1.0 - blend, local_xz, ddx_local, ddy_local);
    }
    if blend >= 1.0 || (blend > 0.0 && overlay_id != base_id) {
        out = accumulate(
            out, overlay_id, corner_weights, blend, local_xz, ddx_local, ddy_local);
    }
    return out;
}

// The control word a cell's own slope stands for.
//
// A word, not a colour: the two slots and the blend between them are what
// a hand would have painted for this slope, so the fragment carries on
// into `accumulate_control` and picks up the height blend, the detiling
// and the mip derivatives that painted ground gets. There is no second
// sampling path to drift from the first.
//
// `smoothstep` across the authored range means flat ground is purely the
// base slot, anything past the far end is purely the slope slot, and the
// band between them interlocks by height like any other transition.
fn autoterrain_control(slope: f32) -> u32 {
    let t = smoothstep(splat.autoterrain_slope_start, splat.autoterrain_slope_end, slope);
    let blend = u32(clamp(t, 0.0, 1.0) * 255.0 + 0.5);
    return ((splat.autoterrain_base_slot & BASE_MASK) << BASE_SHIFT)
        | ((splat.autoterrain_slope_slot & OVERLAY_MASK) << OVERLAY_SHIFT)
        | ((blend & BLEND_MASK) << BLEND_SHIFT);
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // UV0 is 0..1 across the whole terrain, shared bit for bit by chunks
    // that meet on a grid line, which is what keeps the control lookup
    // seamless across a chunk join.
    let local_xz = (in.uv - vec2<f32>(0.5)) * splat.terrain_size;
    let ddx_local = dpdx(local_xz);
    let ddy_local = dpdy(local_xz);

    let g = in.uv * f32(max(splat.control_resolution, 2u) - 1u);
    let corner = control_corner(g);
    let f = clamp(g - vec2<f32>(corner), vec2<f32>(0.0), vec2<f32>(1.0));

    var c00 = load_control(corner);
    var c10 = load_control(corner + vec2<i32>(1, 0));
    var c01 = load_control(corner + vec2<i32>(0, 1));
    var c11 = load_control(corner + vec2<i32>(1, 1));

    // A terrain with autoterrain off never reaches this, and a cell a
    // hand claimed keeps the word it was painted with. Everything
    // downstream reads these four words and cannot tell which of them
    // came from the control map.
    if splat.autoterrain_enabled != 0u {
        let unclaimed = autoterrain_control(slope_at(corner, f));
        c00 = select(unclaimed, c00, (c00 & MANUAL_BIT) != 0u);
        c10 = select(unclaimed, c10, (c10 & MANUAL_BIT) != 0u);
        c01 = select(unclaimed, c01, (c01 & MANUAL_BIT) != 0u);
        c11 = select(unclaimed, c11, (c11 & MANUAL_BIT) != 0u);
    }

    let inv = vec2<f32>(1.0) - f;
    let w = vec4<f32>(inv.x * inv.y, f.x * inv.y, inv.x * f.y, f.x * f.y);

    var accum: Accum;
    accum.albedo = vec3<f32>(0.0);
    accum.normal = vec3<f32>(0.0);
    accum.total = 0.0;

    if c00 == c10 && c00 == c01 && c00 == c11 {
        // Every unpainted fragment, and the interior of every uniformly
        // painted area: one control word, so four taps would sample the
        // same textures four times over. Passing all four corner weights
        // to one call sums the same four terms the branch below sums
        // separately, so this saves the samples without shading
        // differently, and draws no seam one cell outside a plateau.
        accum = accumulate_control(accum, c00, w, local_xz, ddx_local, ddy_local);
    } else {
        accum = accumulate_control(
            accum, c00, vec4<f32>(w.x, 0.0, 0.0, 0.0), local_xz, ddx_local, ddy_local);
        accum = accumulate_control(
            accum, c10, vec4<f32>(0.0, w.y, 0.0, 0.0), local_xz, ddx_local, ddy_local);
        accum = accumulate_control(
            accum, c01, vec4<f32>(0.0, 0.0, w.z, 0.0), local_xz, ddx_local, ddy_local);
        accum = accumulate_control(
            accum, c11, vec4<f32>(0.0, 0.0, 0.0, w.w), local_xz, ddx_local, ddy_local);
    }

    let inv_total = 1.0 / max(accum.total, 1e-8);
    let albedo = accum.albedo * inv_total;
    let tangent_normal = accum.normal * inv_total;

    // The mesher emits no tangents: UV0 runs along world X and Z, so the
    // tangent is world +X projected onto the surface normal. That
    // projection collapses where the surface stands vertical and faces X,
    // which is where a planar XZ UV is itself degenerate, so +Z stands in
    // there rather than normalizing a zero vector. Picking the axis the
    // normal leans on least, the usual choice for arbitrary geometry,
    // would swap the frame by 90 degrees wherever |n.x| and |n.z| cross,
    // seaming flat ground.
    let world_normal = pbr_functions::prepare_world_normal(in.world_normal, false, is_front);
    let n = normalize(world_normal);
    var reference = vec3<f32>(1.0, 0.0, 0.0);
    if abs(n.x) > 0.999 {
        reference = vec3<f32>(0.0, 0.0, 1.0);
    }
    let t = normalize(reference - n * dot(reference, n));
    let tbn = mat3x3<f32>(t, cross(t, n), n);
    let mapped_normal = normalize(tbn * normalize(tangent_normal));

    var pbr_input = pbr_types::pbr_input_new();
    // Mesh flags carry the shadow-receiver and transmitted-shadow-receiver
    // bits. Leaving them zero renders terrain that no shadow ever falls on.
    pbr_input.flags = mesh[in.instance_index].flags;
    pbr_input.material.base_color = vec4<f32>(albedo, 1.0);
    pbr_input.material.perceptual_roughness = splat.perceptual_roughness;
    pbr_input.material.metallic = 0.0;
    // `standard_material_new` leaves fog off; `StandardMaterial`'s Rust
    // default has it on, and terrain that ignores the scene's fog while
    // everything beside it obeys reads as a hole in the distance.
    pbr_input.material.flags |= pbr_types::STANDARD_MATERIAL_FLAGS_FOG_ENABLED_BIT;
    pbr_input.frag_coord = in.position;
    pbr_input.world_position = in.world_position;
    pbr_input.world_normal = n;
    pbr_input.N = mapped_normal;
    pbr_input.is_orthographic = view.clip_from_view[3].w == 1.0;
    pbr_input.V = pbr_functions::calculate_view(in.world_position, pbr_input.is_orthographic);

    var out: FragmentOutput;
    out.color = pbr_functions::apply_pbr_lighting(pbr_input);
    out.color = pbr_functions::main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
