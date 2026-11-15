//! Timings for parsing a scene of 761 brushes and about 21,000 faces, the
//! size of a large hand-built level, about 18 MB of text. Set
//! `JACKDAW_BENCH_SCENE` to a `.bsn` file to time that file instead. Run with
//! `cargo test -p jackdaw_bsn --release --test parse_bench -- --ignored --nocapture`.

use std::fmt::Write;
use std::time::{Duration, Instant};

use jackdaw_bsn::syntax::{parse_stream::ParseStream, span::Span, types::BsnListRoot};

const RUNS: usize = 5;
const GROUPS: usize = 40;
const BRUSHES: usize = 761;
const FACES: usize = 28;
const MATERIALS: usize = 43;

fn hash(a: usize, b: usize) -> u32 {
    let mut h = (a as u32).wrapping_mul(0x9e37_79b9) ^ (b as u32).wrapping_mul(0x85eb_ca6b);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7feb_352d);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846c_a68b);
    h ^ (h >> 16)
}

/// A float in -1..1 that prints with as many digits as a saved scene has.
fn unit(a: usize, b: usize) -> f32 {
    hash(a, b) as f32 / u32::MAX as f32 * 2.0 - 1.0
}

fn vec3(out: &mut String, a: usize, b: usize) {
    let _ = write!(
        out,
        "glam::Vec3 {{ x: {:?}, y: {:?}, z: {:?} }}",
        unit(a, b),
        unit(a, b + 1),
        unit(a, b + 2)
    );
}

fn transform(out: &mut String, indent: &str, a: usize) {
    let _ = write!(
        out,
        "{indent}bevy_transform::components::transform::Transform {{\n{indent}    translation: "
    );
    vec3(out, a, 900);
    let _ = write!(
        out,
        ",\n{indent}    rotation: glam::Quat {{ x: 0.0, y: {:?}, z: 0.0, w: {:?} }},\n{indent}}}\n",
        unit(a, 910),
        unit(a, 911)
    );
}

fn brush(out: &mut String, indent: &str, b: usize) {
    let _ = writeln!(
        out,
        "{indent}#wall\n{indent}jackdaw_scene_types::types::Brush {{\nfaces: ["
    );
    for f in 0..FACES {
        let k = b * FACES + f;
        out.push_str(
            "jackdaw_geometry::BrushFaceData { plane: jackdaw_geometry::BrushPlane { normal: ",
        );
        vec3(out, k, 0);
        let _ = write!(
            out,
            ", distance: {:?} }}, material: \"materials/m{}.bsn\", uv_scale: glam::Vec2 {{ x: 1.0, y: 1.0 }}, uv_u_axis: ",
            unit(k, 3) * 8.0,
            hash(k, 4) as usize % MATERIALS
        );
        vec3(out, k, 5);
        out.push_str(", uv_v_axis: ");
        vec3(out, k, 8);
        out.push_str(" },\n");
    }
    out.push_str("],\ntopology: jackdaw_geometry::topology::BrushTopology {\nvertices: [\n");
    let verts = FACES - 2;
    for v in 0..verts {
        out.push_str("jackdaw_geometry::topology::MeshVert { position: ");
        vec3(out, b * 100 + v, 20);
        out.push_str(" },\n");
    }
    out.push_str("],\nedges: [\n");
    for e in 0..verts * 2 {
        let _ = writeln!(
            out,
            "jackdaw_geometry::topology::MeshEdge {{ v: [{}, {}] }},",
            e % verts,
            (e * 7 + 1) % verts
        );
    }
    out.push_str("],\npolygons: [\n");
    for p in 0..FACES {
        let _ = writeln!(
            out,
            "jackdaw_geometry::topology::MeshPoly {{ loop_start: {}, loop_total: 4 }},",
            p * 4
        );
    }
    out.push_str("],\nloops: [\n");
    for l in 0..FACES * 4 {
        let _ = writeln!(
            out,
            "jackdaw_geometry::topology::MeshLoop {{ vert: {}, edge: {} }},",
            l % verts,
            (l * 3) % (verts * 2)
        );
    }
    let _ = writeln!(
        out,
        "],\nattributes: jackdaw_geometry::topology::AttributeStack {{ vert: map[], edge: map[], loop_: map[], face: map[] }},\n}},\n{indent}}}"
    );
    let _ = writeln!(
        out,
        "{indent}jackdaw_scene_types::node_id::SceneNodeId({})",
        866_029_641_478_460_591_u64 + b as u64
    );
    transform(out, indent, b);
}

/// A scene shaped like a saved level: a light and a camera, then groups of
/// brushes two levels deep.
fn large_scene() -> String {
    let mut out =
        String::from("// jackdaw 0.21.0-dev | bevy main\nbevy_ecs::hierarchy::Children [\n");
    out.push_str(
        "    #\"Directional Light\"\n    bevy_light::directional_light::DirectionalLight {\n        shadow_maps_enabled: true,\n    }\n    --\n",
    );
    out.push_str(
        "    #\"Main Camera\"\n    bevy_camera::camera::Camera\n    bevy_camera::projection::Projection::Perspective(bevy_camera::projection::PerspectiveProjection { fov: 0.7853982, aspect_ratio: 1.0, near: 0.1, far: 1000.0 })\n",
    );
    let per_group = BRUSHES.div_ceil(GROUPS);
    for g in 0..GROUPS {
        let _ = writeln!(out, "    --\n    #district_{g}");
        transform(&mut out, "    ", 10_000 + g);
        out.push_str("    bevy_ecs::hierarchy::Children [\n");
        let brushes = (g * per_group)..((g + 1) * per_group).min(BRUSHES);
        for (i, b) in brushes.enumerate() {
            if i > 0 {
                out.push_str("        --\n");
            }
            brush(&mut out, "        ", b);
        }
        out.push_str("    ]\n");
    }
    out.push_str("]\n");
    out
}

fn scene_text() -> String {
    match std::env::var_os("JACKDAW_BENCH_SCENE") {
        Some(path) => std::fs::read_to_string(path).expect("the scene file reads"),
        None => large_scene(),
    }
}

fn median(mut times: Vec<Duration>) -> f64 {
    times.sort();
    times[times.len() / 2].as_secs_f64() * 1000.0
}

#[expect(clippy::print_stderr, reason = "reports the timings")]
fn report(name: &str, bytes: usize, times: Vec<Duration>) {
    let runs = times.len();
    let ms = median(times);
    let rate = bytes as f64 / 1_000_000.0 / (ms / 1000.0);
    eprintln!("bench {name}: {ms:.2} ms, {rate:.1} MB/s (n={runs})");
}

fn time(runs: usize, mut f: impl FnMut()) -> Vec<Duration> {
    (0..runs)
        .map(|_| {
            let started = Instant::now();
            f();
            started.elapsed()
        })
        .collect()
}

#[test]
#[ignore = "benchmark"]
#[expect(clippy::print_stderr, reason = "reports the scene's size")]
fn large_scene_parse() {
    let text = scene_text();
    eprintln!("scene: {} bytes", text.len());
    report(
        "syntax pass",
        text.len(),
        time(RUNS, || {
            let root = ParseStream::from(Span::from(std::hint::black_box(text.as_str())))
                .parse::<BsnListRoot>();
            std::hint::black_box(root.expect("the scene parses"));
        }),
    );
    report(
        "migrate",
        text.len(),
        time(RUNS, || {
            let migrated = jackdaw_bsn::migrate::migrate_stamped(std::hint::black_box(&text));
            std::hint::black_box(migrated);
        }),
    );
    report(
        "parse_bsn_text",
        text.len(),
        time(RUNS, || {
            let ast = jackdaw_bsn::parse_bsn_text(std::hint::black_box(&text));
            std::hint::black_box(ast.expect("the scene parses"));
        }),
    );
}
