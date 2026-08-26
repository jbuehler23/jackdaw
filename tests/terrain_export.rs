//! Pure-core tests for `jd export-terrain`'s output builder. Everything
//! here exercises [`jackdaw::terrain::export::build_export`] directly on
//! plain in-memory data -- no `.bsn` scene, no headless `World` -- per the
//! module's pure-core/shell split.
//!
//! The output shape is a cross-repo contract (an importer in another repo
//! is being built against it), so these assertions check real values, not
//! just "it didn't panic."

use jackdaw::terrain::export::{
    ExportChannel, ExportChannelElement, ExportInput, ExportPaletteEntry, ExportPlacement,
    build_export,
};

/// A 3x3 height field whose values are NOT symmetric under transposition:
/// swapping x and z would change every off-diagonal value, so a writer that
/// transposed the grid would fail the pixel-fidelity assertions below.
///
/// It must be SQUARE. A `Terrain` is described by a single `resolution`
/// (vertices per edge), so a heightmap is always `resolution^2`; an earlier
/// 3x2 fixture here supplied 6 values for a resolution of 3 and every test
/// using it failed inside the PNG encoder with "pixel buffer size mismatch".
fn sample_heights() -> Vec<f32> {
    vec![
        0.0, 1.0, 2.0, // z = 0
        10.0, 11.0, 12.0, // z = 1
        20.0, 21.0, 22.0, // z = 2
    ]
}

fn sample_channel() -> ExportChannel {
    ExportChannel {
        name: "biome".to_string(),
        element: ExportChannelElement::U16,
        values: vec![5, 6, 7, 8, 9, 10, 11, 12, 13],
        palette: vec![
            ExportPaletteEntry {
                value: 2,
                label: "forest".to_string(),
                color_hex: "#224411".to_string(),
            },
            ExportPaletteEntry {
                value: 1,
                label: "meadow".to_string(),
                color_hex: "#66aa44".to_string(),
            },
        ],
    }
}

fn base_input<'a>(
    heights: &'a [f32],
    channels: &'a [ExportChannel],
    placements: &'a [ExportPlacement],
) -> ExportInput<'a> {
    ExportInput {
        resolution: 3,
        heights,
        channels,
        // 3 vertices -> 2 cells per edge. x and z must agree: the exporter
        // checks the declared cell size against size/(resolution-1) on BOTH
        // axes, so a terrain whose axes disagree has no single cell size.
        size: (2.0, 2.0),
        terrain_origin_m: [-1.0, -1.0],
        max_height: 22.0,
        quantization: None,
        source_scene: "scenes/sample.bsn",
        placements,
        raw_heights: false,
    }
}

fn find_file<'a>(
    files: &'a [jackdaw::terrain::export::ExportedFile],
    relative: &str,
) -> &'a jackdaw::terrain::export::ExportedFile {
    files
        .iter()
        .find(|f| f.relative_path.as_path() == std::path::Path::new(relative))
        .unwrap_or_else(|| panic!("missing output file {relative}"))
}

// 1. Pixel fidelity: heightmap and channel PNGs decode back to the exact
//    in-memory arrays, in z-major row order.
#[test]
fn pixel_fidelity_heightmap_and_channel_round_trip_exactly() {
    let heights = sample_heights();
    let channel = sample_channel();
    let channels = vec![channel];
    let input = base_input(&heights, &channels, &[]);
    let files = build_export(&input).expect("build_export succeeds");

    let heightmap = find_file(&files, "heightmap.png");
    let decoded = image::load_from_memory(&heightmap.bytes)
        .expect("decode heightmap.png")
        .into_luma16();
    assert_eq!(decoded.dimensions(), (3, 3));
    // Unquantized: step_m = (max - min) / 65535, base_m = floor(min/step)*step.
    // sample_heights() spans exactly 0..22, so this coincides with 22.0 / 65535.
    let step_m = 22.0_f32 / 65535.0;
    let base_m = (0.0_f32 / step_m).floor() * step_m;
    for z in 0..3u32 {
        for x in 0..3u32 {
            let expected_h = heights[(z * 3 + x) as usize];
            let pixel = decoded.get_pixel(x, z).0[0];
            let decoded_h = base_m + pixel as f32 * step_m;
            assert!(
                (decoded_h - expected_h).abs() < step_m,
                "x={x} z={z}: decoded {decoded_h} vs expected {expected_h}"
            );
        }
    }

    let channel_file = find_file(&files, "channels/biome.png");
    let decoded_channel = image::load_from_memory(&channel_file.bytes)
        .expect("decode channels/biome.png")
        .into_luma16();
    assert_eq!(decoded_channel.dimensions(), (3, 3));
    let expected_values = [5u16, 6, 7, 8, 9, 10, 11, 12, 13];
    for z in 0..3u32 {
        for x in 0..3u32 {
            let expected = expected_values[(z * 3 + x) as usize];
            let pixel = decoded_channel.get_pixel(x, z).0[0];
            assert_eq!(pixel, expected, "x={x} z={z}");
        }
    }
}

// 2. Byte-stability: running the pure core twice on the same input
//    produces byte-identical output for every file.
#[test]
fn byte_stability_across_repeated_runs() {
    let heights = sample_heights();
    let channels = vec![sample_channel()];
    let placements = vec![ExportPlacement {
        name: "Rock_02".to_string(),
        asset: Some("models/Rock.glb".to_string()),
        translation: [1.0, 0.0, 2.0],
        rotation: [0.0, 0.0, 0.0, 1.0],
        scale: [1.0, 1.0, 1.0],
        components: serde_json::Map::new(),
    }];
    let input = base_input(&heights, &channels, &placements);

    let first = build_export(&input).expect("first run");
    let second = build_export(&input).expect("second run");

    assert_eq!(first.len(), second.len());
    for a in &first {
        let b = find_file(&second, a.relative_path.to_str().unwrap());
        assert_eq!(
            a.bytes,
            b.bytes,
            "{} differs across runs",
            a.relative_path.display()
        );
    }
}

// 3. Manifest shape: parses, documented keys present, quantization null vs
//    populated, channels sorted, palette sorted.
#[test]
fn manifest_shape_and_sort_order() {
    let heights = sample_heights();
    let channel_z = ExportChannel {
        name: "zoning".to_string(),
        element: ExportChannelElement::U8,
        values: vec![1; 9],
        palette: vec![],
    };
    let channel_a = sample_channel(); // "biome"
    let channels = vec![channel_z, channel_a];
    let input = base_input(&heights, &channels, &[]);

    let files = build_export(&input).expect("build_export succeeds");
    let manifest_bytes = &find_file(&files, "manifest.json").bytes;
    let manifest: serde_json::Value =
        serde_json::from_slice(manifest_bytes).expect("manifest.json parses");

    assert_eq!(manifest["format"], "jackdaw-terrain-export");
    assert_eq!(manifest["format_version"], 2);
    assert_eq!(manifest["source_scene"], "scenes/sample.bsn");
    assert_eq!(manifest["terrain"]["cells_per_edge"], 3);
    assert_eq!(
        manifest["terrain"]["world_size_m"],
        serde_json::json!([2.0, 2.0])
    );
    assert_eq!(manifest["terrain"]["max_height_m"], 22.0);
    assert_eq!(
        manifest["terrain"]["terrain_origin_m"],
        serde_json::json!([-1.0, -1.0])
    );
    assert!(manifest["quantization"].is_null(), "unquantized export");
    assert_eq!(manifest["heightmap"]["file"], "heightmap.png");
    assert_eq!(manifest["heightmap"]["bit_depth"], 16);
    assert_eq!(manifest["placements_file"], "placements.json");

    let channels_json = manifest["channels"].as_array().expect("channels array");
    assert_eq!(channels_json.len(), 2);
    // Sorted by name: "biome" before "zoning".
    assert_eq!(channels_json[0]["name"], "biome");
    assert_eq!(channels_json[1]["name"], "zoning");

    // "biome" palette sorted by value: 1 (meadow) before 2 (forest).
    let palette = channels_json[0]["palette"].as_array().expect("palette");
    assert_eq!(palette[0]["value"], 1);
    assert_eq!(palette[0]["label"], "meadow");
    assert_eq!(palette[1]["value"], 2);
    assert_eq!(palette[1]["label"], "forest");

    // The trailing-newline / 2-space-indent contract.
    let text = String::from_utf8(manifest_bytes.clone()).unwrap();
    assert!(text.ends_with('\n'), "single trailing newline");
    assert!(!text.ends_with("\n\n"), "exactly one trailing newline");
    assert!(text.contains("\n  \""), "2-space indent");
}

// Quantized manifest: "quantization" is populated, not null.
#[test]
fn manifest_quantization_populated_when_quantized() {
    let heights = sample_heights();
    let channels: Vec<ExportChannel> = Vec::new();
    let mut input = base_input(&heights, &channels, &[]);
    input.quantization = Some((1.0, 0.25));

    let files = build_export(&input).expect("build_export succeeds");
    let manifest: serde_json::Value =
        serde_json::from_slice(&find_file(&files, "manifest.json").bytes).unwrap();
    assert_eq!(manifest["quantization"]["cell_size_m"], 1.0);
    assert_eq!(manifest["quantization"]["elevation_step_m"], 0.25);
}

// I1 pinning test, the exact scenario from the review finding: authored
// heights whose span exceeds the terrain's configured `max_height` must
// still export. The old step derivation (`max_height / 65535`) sized the
// pixel range off the ceiling rather than the actual data, so a span
// wider than that ceiling overflowed HeightOutOfRange on data that is
// perfectly representable once the step is sized off the real span.
#[test]
fn heights_spanning_more_than_max_height_still_export() {
    let heights: Vec<f32> = (0..9).map(|i| i as f32 * 7.5).collect(); // 0..60
    let channels: Vec<ExportChannel> = Vec::new();
    let mut input = base_input(&heights, &channels, &[]);
    input.max_height = 50.0;

    let files =
        build_export(&input).expect("a span wider than max_height must still be representable");

    let manifest: serde_json::Value =
        serde_json::from_slice(&find_file(&files, "manifest.json").bytes).unwrap();
    let step_m = manifest["heightmap"]["step_m"].as_f64().unwrap() as f32;
    let expected_step = 60.0_f32 / 65535.0;
    assert!(
        (step_m - expected_step).abs() < 1e-6,
        "step_m {step_m} must derive from the actual 0..60 span, not max_height 50",
    );

    let decoded = image::load_from_memory(&find_file(&files, "heightmap.png").bytes)
        .unwrap()
        .into_luma16();
    let base_m = manifest["heightmap"]["base_m"].as_f64().unwrap() as f32;
    for (i, &expected_h) in heights.iter().enumerate() {
        let (x, z) = (i as u32 % 3, i as u32 / 3);
        let pixel = decoded.get_pixel(x, z).0[0];
        let decoded_h = base_m + pixel as f32 * step_m;
        assert!(
            (decoded_h - expected_h).abs() < step_m,
            "x={x} z={z}: decoded {decoded_h} vs expected {expected_h}"
        );
    }
}

// M1: quantized heights decode back to the actual snapped value, not
// merely to *some* multiple of the step. The previous assertion checked
// only "decoded_h is a multiple of step_m" -- true of every pixel this
// encoding can produce by construction (decoded_h = base_m + pixel *
// step_m, and base_m is itself floor(min/step)*step), so it would still
// pass even if quantize_heights were never actually applied to the
// input. Comparing against jackdaw_terrain::quantize_height(original,
// step) pins the real behavior: the encoded value must match what
// quantization actually does to each input height.
#[test]
fn quantized_heights_decode_to_the_actual_snapped_value() {
    let heights = vec![0.1, 0.4, 0.6, -0.3, 1.55, 2.0, 0.0, -1.2, 3.05];
    let channels: Vec<ExportChannel> = Vec::new();
    let mut input = base_input(&heights, &channels, &[]);
    input.quantization = Some((1.0, 0.25));

    let files = build_export(&input).expect("build_export succeeds");
    let manifest: serde_json::Value =
        serde_json::from_slice(&find_file(&files, "manifest.json").bytes).unwrap();
    let base_m = manifest["heightmap"]["base_m"].as_f64().unwrap() as f32;
    let step_m = manifest["heightmap"]["step_m"].as_f64().unwrap() as f32;
    assert_eq!(step_m, 0.25);

    let decoded = image::load_from_memory(&find_file(&files, "heightmap.png").bytes)
        .unwrap()
        .into_luma16();
    for z in 0..3u32 {
        for x in 0..3u32 {
            let i = (z * 3 + x) as usize;
            let expected = jackdaw_terrain::quantize_height(heights[i], 0.25);
            let pixel = decoded.get_pixel(x, z).0[0];
            let decoded_h = base_m + pixel as f32 * step_m;
            assert!(
                (decoded_h - expected).abs() < 1e-3,
                "x={x} z={z}: decoded {decoded_h} does not match the actual quantized \
                 value {expected} of input {}",
                heights[i]
            );
        }
    }
}

// 5. Refusal: a declared cell_size_m inconsistent with size/(resolution-1)
//    is rejected, naming both values.
#[test]
fn cell_size_mismatch_is_refused_naming_both_values() {
    let heights = sample_heights();
    let channels: Vec<ExportChannel> = Vec::new();
    let mut input = base_input(&heights, &channels, &[]);
    // size.x = 2.0, resolution = 3 -> derived cell size 1.0; declare 5.0.
    input.quantization = Some((5.0, 0.25));

    let error = build_export(&input).expect_err("mismatched cell size must be refused");
    let message = error.to_string();
    assert!(message.contains("5"), "names the declared value: {message}");
    assert!(message.contains("1"), "names the derived value: {message}");
}

// 6. Placements determinism: an unsorted input set of placements
//    serialises in sorted order, identically across runs.
#[test]
fn placements_serialize_in_sorted_order_deterministically() {
    let heights = sample_heights();
    let channels: Vec<ExportChannel> = Vec::new();
    let placements = vec![
        ExportPlacement {
            name: "Zebra".to_string(),
            asset: Some("models/Zebra.glb".to_string()),
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            components: serde_json::Map::new(),
        },
        ExportPlacement {
            name: "Anchor".to_string(),
            asset: None,
            translation: [0.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            components: serde_json::Map::new(),
        },
        ExportPlacement {
            name: "Anchor".to_string(),
            asset: Some("models/AnchorB.glb".to_string()),
            translation: [5.0, 0.0, 0.0],
            rotation: [0.0, 0.0, 0.0, 1.0],
            scale: [1.0, 1.0, 1.0],
            components: {
                let mut m = serde_json::Map::new();
                m.insert(
                    "SpawnPoint".to_string(),
                    serde_json::json!({"species": "meadow_hopper", "count": 3, "respawn_seconds": 120}),
                );
                m
            },
        },
    ];
    let input = base_input(&heights, &channels, &placements);

    let first = build_export(&input).expect("first run");
    let second = build_export(&input).expect("second run");

    let doc: serde_json::Value =
        serde_json::from_slice(&find_file(&first, "placements.json").bytes).unwrap();
    assert_eq!(doc["format_version"], 1);
    let list = doc["placements"].as_array().expect("placements array");
    assert_eq!(list.len(), 3);
    // Sorted by name, then asset (None < Some): "Anchor"/None, "Anchor"/Some, "Zebra".
    assert_eq!(list[0]["name"], "Anchor");
    assert!(list[0]["asset"].is_null());
    assert_eq!(list[1]["name"], "Anchor");
    assert_eq!(list[1]["asset"], "models/AnchorB.glb");
    assert_eq!(
        list[1]["components"]["SpawnPoint"]["species"],
        "meadow_hopper"
    );
    assert_eq!(list[2]["name"], "Zebra");

    let first_placements_bytes = &find_file(&first, "placements.json").bytes;
    let second_placements_bytes = &find_file(&second, "placements.json").bytes;
    assert_eq!(first_placements_bytes, second_placements_bytes);
}

// --raw-heights writes heights.f32 as little-endian f32, row-major, and
// carries the ORIGINAL (unsnapped) heights even when quantized.
#[test]
fn raw_heights_writes_original_little_endian_f32() {
    let heights = vec![0.1_f32, 0.4, 0.6, -0.3, 1.55, 2.0, 0.0, -1.2, 3.05];
    let channels: Vec<ExportChannel> = Vec::new();
    let mut input = base_input(&heights, &channels, &[]);
    input.quantization = Some((1.0, 0.25));
    input.raw_heights = true;

    let files = build_export(&input).expect("build_export succeeds");
    let raw = &find_file(&files, "heights.f32").bytes;
    assert_eq!(raw.len(), heights.len() * 4);
    for (i, h) in heights.iter().enumerate() {
        let bytes: [u8; 4] = raw[i * 4..i * 4 + 4].try_into().unwrap();
        assert_eq!(f32::from_le_bytes(bytes), *h);
    }
}

// No --raw-heights: heights.f32 is not written at all.
#[test]
fn heights_f32_absent_without_raw_heights_flag() {
    let heights = sample_heights();
    let channels: Vec<ExportChannel> = Vec::new();
    let input = base_input(&heights, &channels, &[]);

    let files = build_export(&input).expect("build_export succeeds");
    assert!(
        !files
            .iter()
            .any(|f| f.relative_path.as_path() == std::path::Path::new("heights.f32")),
        "heights.f32 must be absent without --raw-heights"
    );
}

// Channel filename collision: two channel names that sanitize to the same
// filename are refused, naming both.
#[test]
fn channel_filename_collision_is_refused_naming_both() {
    let heights = sample_heights();
    let channels = vec![
        ExportChannel {
            name: "biome/a".to_string(),
            element: ExportChannelElement::U8,
            values: vec![0; 9],
            palette: vec![],
        },
        ExportChannel {
            name: "biome_a".to_string(),
            element: ExportChannelElement::U8,
            values: vec![0; 9],
            palette: vec![],
        },
    ];
    let input = base_input(&heights, &channels, &[]);

    let error = build_export(&input).expect_err("colliding channel names must be refused");
    let message = error.to_string();
    assert!(message.contains("biome/a"));
    assert!(message.contains("biome_a"));
}

/// C3: two channels with the exact same name must be refused, not
/// silently collapsed to one exported file by the last-wins writer.
/// Reachable through the UI: add, add, remove index 0, add mints
/// `channel-1` twice (see `channel_ops.rs`'s `mint_channel_name`, which
/// this pins from the export side).
#[test]
fn exact_duplicate_channel_names_are_refused() {
    let heights = sample_heights();
    let channels = vec![
        ExportChannel {
            name: "channel-1".to_string(),
            element: ExportChannelElement::U8,
            values: vec![1; 9],
            palette: vec![],
        },
        ExportChannel {
            name: "channel-1".to_string(),
            element: ExportChannelElement::U8,
            values: vec![2; 9],
            palette: vec![],
        },
    ];
    let input = base_input(&heights, &channels, &[]);

    let error = build_export(&input).expect_err("exact-duplicate channel names must be refused");
    assert!(error.to_string().contains("channel-1"));
}

/// M3: a U8 channel value over 255 must saturate on export, matching
/// sidecar.rs's own U8 encode. Wrapping (`as u8`) would turn 256 into 0
/// -- silently the wrong palette index, not just a clipped one.
#[test]
fn a_u8_channel_value_over_255_saturates_rather_than_wraps() {
    let heights = sample_heights();
    let channels = vec![ExportChannel {
        name: "biome".to_string(),
        element: ExportChannelElement::U8,
        values: vec![256, 300, 0, 1, 2, 3, 4, 5, 6],
        palette: vec![],
    }];
    let input = base_input(&heights, &channels, &[]);

    let files = build_export(&input).expect("build_export succeeds");
    let decoded = image::load_from_memory(&find_file(&files, "channels/biome.png").bytes)
        .unwrap()
        .into_luma8();
    assert_eq!(
        decoded.get_pixel(0, 0).0[0],
        255,
        "256 must saturate to 255, not wrap to 0"
    );
    assert_eq!(
        decoded.get_pixel(1, 0).0[0],
        255,
        "300 must saturate to 255"
    );
    assert_eq!(decoded.get_pixel(2, 0).0[0], 0);
}
