// Cubic Bezier wire renderer for the node graph.
//
// The cubic (p0, p1, p2, p3) is split at t = 0.5 into two quadratic Beziers
// and the signed distance field for each quadratic is computed using Inigo
// Quilez's closed-form solution from distfunctions2d. The fragment color is
// the minimum of the two SDFs, anti-aliased with smoothstep(fwidth(d)).
//
// Reference: https://iquilezles.org/articles/distfunctions2d/

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct ConnectionUniforms {
    p0: vec2<f32>,
    p1: vec2<f32>,
    p2: vec2<f32>,
    p3: vec2<f32>,
    color: vec4<f32>,
    width: f32,
    feather: f32,
};

@group(1) @binding(0)
var<uniform> u: ConnectionUniforms;

// Inigo Quilez: signed distance from point `pos` to quadratic Bezier (A, B, C).
// Solves the cubic `dot(pos - bezier(t), bezier'(t)) = 0` for the closest t.
fn sd_bezier_quadratic(pos: vec2<f32>, A: vec2<f32>, B: vec2<f32>, C: vec2<f32>) -> f32 {
    let a = B - A;
    let b = A - 2.0 * B + C;
    let c = a * 2.0;
    let d = A - pos;

    let bb = dot(b, b);
    if (bb < 1.0e-6) {
        // Degenerate: control points collinear; treat as a line segment.
        let ba = C - A;
        let t = clamp(dot(pos - A, ba) / max(dot(ba, ba), 1.0e-6), 0.0, 1.0);
        return length((A - pos) + ba * t);
    }

    let kk = 1.0 / bb;
    let kx = kk * dot(a, b);
    let ky = kk * (2.0 * dot(a, a) + dot(d, b)) / 3.0;
    let kz = kk * dot(d, a);

    let p = ky - kx * kx;
    let q = kx * (2.0 * kx * kx - 3.0 * ky) + kz;
    let p3 = p * p * p;
    let q2 = q * q;
    let h = q2 + 4.0 * p3;

    var res: f32;
    if (h >= 0.0) {
        // One real root.
        let h_sqrt = sqrt(h);
        let x = (vec2<f32>(h_sqrt, -h_sqrt) - q) * 0.5;
        let uv = sign(x) * pow(abs(x), vec2<f32>(1.0 / 3.0));
        let t = clamp(uv.x + uv.y - kx, 0.0, 1.0);
        let qpt = d + (c + b * t) * t;
        res = dot(qpt, qpt);
    } else {
        // Three real roots; pick the closest.
        let z = sqrt(-p);
        let v = acos(q / (p * z * 2.0)) / 3.0;
        let m = cos(v);
        let n = sin(v) * 1.732050808;
        let t3 = clamp(vec3<f32>(m + m, -n - m, n - m) * z - kx, vec3<f32>(0.0), vec3<f32>(1.0));
        let q0 = d + (c + b * t3.x) * t3.x;
        let q1 = d + (c + b * t3.y) * t3.y;
        let q2v = d + (c + b * t3.z) * t3.z;
        res = min(min(dot(q0, q0), dot(q1, q1)), dot(q2v, q2v));
    }

    return sqrt(res);
}

// De Casteljau split of a cubic Bezier at t = 0.5.
// Returns two quadratic approximations that together cover the cubic.
// We pick the midpoint of each half as the quadratic control point by
// choosing the average of its two cubic control points; a common cheap
// approximation that's accurate enough for stroked wires.
fn split_cubic(
    p0: vec2<f32>, p1: vec2<f32>, p2: vec2<f32>, p3: vec2<f32>,
) -> array<vec2<f32>, 6> {
    let q0 = mix(p0, p1, 0.5);
    let q1 = mix(p1, p2, 0.5);
    let q2 = mix(p2, p3, 0.5);
    let r0 = mix(q0, q1, 0.5);
    let r1 = mix(q1, q2, 0.5);
    let mid = mix(r0, r1, 0.5);
    return array<vec2<f32>, 6>(
        // First quadratic: start, control, end
        p0, q0, mid,
        // Second quadratic: start, control, end
        mid, q2, p3,
    );
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    // Convert UV into pixel-space within the material's local rect.
    let pos = in.uv * in.size;

    // Split the cubic into 2 quadratics and take min-SDF.
    let split = split_cubic(u.p0, u.p1, u.p2, u.p3);
    let d0 = sd_bezier_quadratic(pos, split[0], split[1], split[2]);
    let d1 = sd_bezier_quadratic(pos, split[3], split[4], split[5]);
    let d = min(d0, d1);

    // Anti-aliased stroke: the curve SDF is unsigned (distance from a thin
    // line), so we subtract half the stroke width and smoothstep-feather.
    let half_width = u.width * 0.5;
    let edge = d - half_width;
    let aa = max(fwidth(edge), u.feather);
    let alpha = 1.0 - smoothstep(-aa, aa, edge);

    return vec4<f32>(u.color.rgb, u.color.a * alpha);
}
