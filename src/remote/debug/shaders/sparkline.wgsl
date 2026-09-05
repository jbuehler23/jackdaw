// Sparkline renderer for the debugger's diagnostics panels.
//
// Plots up to `count` samples as an anti-aliased polyline with a soft area
// fill beneath it, normalized by `min_v`/`max_v` over the node's height. The
// samples are packed four per vec4 (std140-friendly): sample `i` lives at
// `samples[i / 4][i % 4]`.

#import bevy_ui::ui_vertex_output::UiVertexOutput

struct SparklineUniforms {
    samples: array<vec4<f32>, 16>,
    color: vec4<f32>,
    count: u32,
    min_v: f32,
    max_v: f32,
};

@group(1) @binding(0)
var<uniform> u: SparklineUniforms;

fn sample_at(i: u32) -> f32 {
    return u.samples[i / 4u][i % 4u];
}

// Height in pixels for sample `i`: 0 at `min_v`, node height at `max_v`.
fn sample_height(i: u32, h: f32) -> f32 {
    let range = max(u.max_v - u.min_v, 1.0e-6);
    return clamp((sample_at(i) - u.min_v) / range, 0.0, 1.0) * h;
}

@fragment
fn fragment(in: UiVertexOutput) -> @location(0) vec4<f32> {
    let px = in.uv * in.size;
    // UV grows downward; flip so larger samples sit higher on screen.
    let y_up = in.size.y - px.y;
    let p = vec2<f32>(px.x, y_up);

    // Need at least two points to draw a segment.
    if (u.count < 2u) {
        return vec4<f32>(0.0);
    }

    let n = u.count;
    let step_x = in.size.x / f32(n - 1u);

    // Walk every filled segment: track the nearest distance for the stroke and
    // the interpolated line height at this column for the area fill. The loop
    // bound `n` is uniform across the draw, so derivatives stay well-defined.
    var d = 1.0e9;
    var line_y = 0.0;
    for (var i = 0u; i < n - 1u; i = i + 1u) {
        let x0 = f32(i) * step_x;
        let x1 = f32(i + 1u) * step_x;
        let y0 = sample_height(i, in.size.y);
        let y1 = sample_height(i + 1u, in.size.y);
        let a = vec2<f32>(x0, y0);
        let b = vec2<f32>(x1, y1);
        let ab = b - a;
        let ap = p - a;
        let seg_t = clamp(dot(ap, ab) / max(dot(ab, ab), 1.0e-6), 0.0, 1.0);
        d = min(d, length(ap - ab * seg_t));

        if (px.x >= x0 && px.x <= x1) {
            let col_t = clamp((px.x - x0) / max(x1 - x0, 1.0e-6), 0.0, 1.0);
            line_y = mix(y0, y1, col_t);
        }
    }

    // Anti-aliased stroke around the polyline.
    let half_width = 1.5;
    let edge = d - half_width;
    let aa = max(fwidth(edge), 1.0);
    let line_alpha = 1.0 - smoothstep(-aa, aa, edge);

    // Soft translucent fill from the baseline up to the line.
    let fill_aa = max(fwidth(y_up), 1.0);
    let fill_mask = 1.0 - smoothstep(line_y - fill_aa, line_y + fill_aa, y_up);
    let fill_alpha = fill_mask * 0.22;

    let alpha = max(line_alpha, fill_alpha) * u.color.a;
    return vec4<f32>(u.color.rgb, alpha);
}
