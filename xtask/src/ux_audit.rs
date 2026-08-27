//! `cargo xtask ux-audit`: scripted golden-path walkthrough of the terrain
//! tools, one whole-window screenshot per step, driven through
//! `JACKDAW_RUN_OP`.
//!
//! The harness only captures the contact sheet; it does not grade it. A
//! step whose UI state cannot be reached through an existing operator is
//! skipped and recorded in the manifest rather than faked.
//!
//! The editor runs headed (a real window, not the headless test harness):
//! `window.screenshot` reads back the primary window's own surface, which
//! needs an actual GPU surface and a settled first frame, same as an
//! interactive launch.

use std::io::Write as _;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// One golden-path step. `ops` is the `JACKDAW_RUN_OP` clause(s) that
/// reach the step's state, `None` when no operator exists to drive it.
struct Step {
    name: &'static str,
    ops: Option<&'static str>,
    skip_reason: &'static str,
}

const STEPS: &[Step] = &[
    Step {
        // A directional light so terrain isn't unlit flat brown from
        // here on: the shots need a believable render, not a silhouette.
        // Deselect it right after: it's set dressing, not something the
        // golden path is about, and an unrelated entity highlighted in
        // the Outliner/Components panel would be a distraction in a
        // "fresh, nothing going on yet" screenshot.
        name: "fresh-scene",
        ops: Some("scene.new ; entity.add.directional_light ; selection.clear"),
        skip_reason: "",
    },
    Step {
        // view.frame_all: establishing shot showing the terrain in
        // context with the grid around it, in three-quarter view. It
        // keeps whatever angled orientation the fresh scene's default
        // camera has, moving the camera along that existing forward
        // vector without re-aiming it, so this is a deterministic
        // distance and position on top of that angle rather than a
        // literal isometric snap (view_ops.rs has no such op).
        name: "add-terrain",
        ops: Some("entity.add.terrain ; view.frame_all"),
        skip_reason: "",
    },
    Step {
        name: "default-generate",
        ops: Some("terrain.generate ; view.frame_all"),
        skip_reason: "",
    },
    Step {
        // view.frame_selected: pulls in close on the terrain (a
        // "working distance" for sculpting), instead of frame_all's
        // establishing-shot distance.
        name: "sculpt-mode",
        ops: Some("terrain.tool.raise ; view.frame_selected"),
        skip_reason: "",
    },
    Step {
        name: "paint-mode",
        ops: Some("terrain.tool.paint ; view.frame_selected"),
        skip_reason: "",
    },
    Step {
        // The Paint tool's target picker switched to Textures. The
        // options bar shows brush radius/falloff/opacity, the
        // modifier-key hint, and which material the brush is loaded with.
        // No `terrain.tool.paint` here: the previous step turned Paint
        // mode on, and that operator *toggles*, so calling it again would
        // turn it back off.
        name: "texture-paint-mode",
        ops: Some("terrain.paint.target target=textures ; view.frame_selected"),
        skip_reason: "",
    },
    Step {
        name: "quantize-mode",
        ops: Some("terrain.tool.quantize ; view.frame_selected"),
        skip_reason: "",
    },
    Step {
        name: "textures-tab",
        // Empty state: the terrain has no materials yet, so this is the
        // "None yet" hint plus the Add Material affordance.
        ops: Some("terrain.tool.exit_to_select ; terrain.panel.tab tab=textures"),
        skip_reason: "",
    },
    Step {
        // The picker: the fixture project's saved materials, in the same
        // tile grammar the Materials panel browses them in. This is the
        // whole add flow; there is no path to type anywhere.
        name: "material-picker",
        ops: Some("terrain.material.picker show=true"),
        skip_reason: "",
    },
    Step {
        // Two materials added and the second selected, so the shot shows
        // the thumbnail grid, the per-slot tiling row and the reorder and
        // remove affordances all at once.
        name: "terrain-materials",
        ops: Some(
            "terrain.material.add material=audit_ground ; \
             terrain.material.add material=audit_rock ; \
             terrain.texture.select index=1",
        ),
        skip_reason: "",
    },
    Step {
        // The same material the other two surfaces show, in slot 0: the
        // terrain leg of the side-by-side comparison.
        name: "material-terrain",
        ops: Some("terrain.texture.select index=0"),
        skip_reason: "",
    },
    Step {
        name: "scatter-tab",
        // Scatter is TerrainPanelTab's default, so leaving the active
        // tool is enough to reach it. `terrain.tool.exit_to_select` is
        // what leaves it, bound to Escape with no palette button of its
        // own (see src/terrain/ops.rs).
        ops: Some("terrain.tool.exit_to_select ; terrain.panel.tab tab=scatter"),
        skip_reason: "",
    },
    Step {
        name: "generation-tab",
        ops: Some("terrain.panel.tab tab=generation"),
        skip_reason: "",
    },
    Step {
        name: "deselected",
        ops: Some("selection.clear"),
        skip_reason: "",
    },
    Step {
        // The Materials window with the same material loaded. The
        // library leg of the comparison.
        name: "material-window",
        ops: Some(
            "window.open window_id=jackdaw.inspector.materials ; \
             material.select material=audit_ground",
        ),
        skip_reason: "",
    },
    Step {
        // The same material again, this time on something using it: a
        // cube, the material applied to its faces, the inspector on its
        // Material tab. The edit-in-context leg.
        name: "material-inspector",
        ops: Some(
            "entity.add.cube ; \
             material.apply material=audit_ground ; \
             window.open window_id=jackdaw.inspector.components ; \
             inspector.category category=material",
        ),
        skip_reason: "",
    },
];

/// Timeout waiting for all captured steps' screenshots to land. Covers
/// the whole run: every clause costs the boot-op driver its settle gap,
/// so this scales with the step table rather than with any one step.
const CAPTURE_TIMEOUT: Duration = Duration::from_secs(300);

fn workspace_root() -> PathBuf {
    // xtask's own manifest dir is `<repo>/xtask`.
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask lives one level under the workspace root")
        .to_path_buf()
}

fn timestamp() -> String {
    let out = Command::new("date")
        .args(["-u", "+%Y%m%dT%H%M%SZ"])
        .output()
        .expect("run `date` for a run-directory timestamp");
    String::from_utf8_lossy(&out.stdout).trim().to_string()
}

pub fn cmd_ux_audit() -> bool {
    let root = workspace_root();

    eprintln!("+ cargo build -p jackdaw");
    let build_ok = Command::new("cargo")
        .args(["build", "-p", "jackdaw"])
        .current_dir(&root)
        .status()
        .map(|s| s.success())
        .unwrap_or(false);
    if !build_ok {
        eprintln!("ux-audit: build of `jackdaw` failed");
        return false;
    }

    let binary = root
        .join("target/debug/jackdaw")
        .with_extension(std::env::consts::EXE_EXTENSION);
    if !binary.is_file() {
        eprintln!("ux-audit: expected editor binary at {}", binary.display());
        return false;
    }

    let run_dir = root.join("target/ux-audit").join(timestamp());
    let project_dir = run_dir.join("project");
    let config_dir = run_dir.join("config");
    for dir in [
        run_dir.clone(),
        project_dir.join("assets"),
        config_dir.clone(),
    ] {
        if let Err(err) = std::fs::create_dir_all(&dir) {
            eprintln!("ux-audit: cannot create {}: {err}", dir.display());
            return false;
        }
    }

    // A project directory with no Cargo.toml lands on the launcher's
    // "not a project" card, not the editor, so JACKDAW_RUN_OP's boot
    // queue (gated on AppState::Editor) never fires. A minimal
    // scaffold with a jackdaw.toml pinned to this build's own versions
    // skips straight past the setup/upgrade offers in
    // `project_select::enter_project_with` (src/project_select.rs), which
    // this only has to satisfy, not change.
    if let Err(err) = write_project_scaffold(&project_dir) {
        eprintln!("ux-audit: cannot write project scaffold: {err}");
        return false;
    }

    // Build the JACKDAW_RUN_OP clause list and the file list it should
    // produce. Steps with no operator are skipped up front, not queued.
    let mut clauses: Vec<String> = Vec::new();
    let mut expected: Vec<(usize, &Step, PathBuf)> = Vec::new();
    for (i, step) in STEPS.iter().enumerate() {
        let Some(ops) = step.ops else { continue };
        let file = run_dir.join(format!("step-{:02}-{}.png", i + 1, step.name));
        clauses.push(ops.to_string());
        clauses.push(format!("window.screenshot path={}", file.display()));
        expected.push((i + 1, step, file));
    }
    let run_op = clauses.join(" ; ");

    eprintln!("ux-audit: run directory {}", run_dir.display());
    eprintln!("+ JACKDAW_RUN_OP={run_op:?} {}", binary.display());

    let mut child = match Command::new(&binary)
        .env("JACKDAW_OPEN_PROJECT", &project_dir)
        .env("JACKDAW_RUN_OP", &run_op)
        .env("XDG_CONFIG_HOME", &config_dir)
        // Pin the window size so every shot is comparable, and so
        // measurements taken off them (chrome's share of the window) mean
        // the same thing run to run, whatever the window manager hands out.
        .env("JACKDAW_WINDOW_SIZE", "1920x1080")
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
    {
        Ok(child) => child,
        Err(err) => {
            eprintln!("ux-audit: cannot launch {}: {err}", binary.display());
            return false;
        }
    };

    let start = Instant::now();
    let mut all_present;
    let exit_status = loop {
        all_present = expected.iter().all(|(.., path)| path.is_file());
        if all_present {
            break None;
        }
        if let Ok(Some(status)) = child.try_wait() {
            break Some(status);
        }
        if start.elapsed() > CAPTURE_TIMEOUT {
            let _ = child.kill();
            let _ = child.wait();
            eprintln!(
                "ux-audit: timed out after {:?} waiting for screenshots",
                CAPTURE_TIMEOUT
            );
            break None;
        }
        std::thread::sleep(Duration::from_millis(200));
    };

    // A killed run must never look like a passing run: an abnormal exit
    // (the editor quit on its own before every expected file landed) or
    // a timeout are both hard failures, checked before the shutdown
    // below.
    if !all_present {
        if let Some(status) = exit_status {
            eprintln!("ux-audit: editor exited unexpectedly with {status}");
        }
        dump_child_output(&mut child);
        for (n, step, path) in &expected {
            if !path.is_file() {
                eprintln!("ux-audit: step {n:02}-{} never captured", step.name);
            }
        }
        return false;
    }

    // Every expected file landed; the editor never exits on its own
    // (JACKDAW_RUN_OP is a "run this, then keep going" mechanism, not a
    // one-shot), so shutting it down here is the intended end of the run,
    // not a crash.
    let _ = child.kill();
    let _ = child.wait();

    if let Err(err) = write_manifest(&run_dir, &expected) {
        eprintln!("ux-audit: cannot write manifest: {err}");
        return false;
    }

    eprintln!(
        "ux-audit: {} steps captured, {} skipped (missing op)",
        expected.len(),
        STEPS.len() - expected.len()
    );
    eprintln!("ux-audit: run directory {}", run_dir.display());
    true
}

/// Jackdaw's own version pins, read the same way `--version` does. Kept
/// local rather than depending on `jackdaw_project_build`, since xtask is
/// a detached workspace and a version string out of `Cargo.toml` is
/// enough to satisfy the launcher's pin check.
fn workspace_version() -> String {
    let text = std::fs::read_to_string(workspace_root().join("Cargo.toml")).unwrap_or_default();
    text.lines()
        .skip_while(|l| l.trim() != "[workspace.package]")
        .find_map(|l| {
            let l = l.trim();
            l.strip_prefix("version = \"")
                .and_then(|rest| rest.strip_suffix('"'))
        })
        .unwrap_or("0.0.0")
        .to_string()
}

fn bevy_minor() -> String {
    let version = workspace_version();
    version.splitn(3, '.').take(2).collect::<Vec<_>>().join(".")
}

fn write_project_scaffold(project_dir: &Path) -> std::io::Result<()> {
    std::fs::write(
        project_dir.join("Cargo.toml"),
        format!(
            "[package]\nname = \"ux-audit-fixture\"\nversion = \"0.1.0\"\nedition = \"2024\"\n\n\
             [workspace]\n\n\
             [dependencies]\nbevy = \"{}\"\n",
            bevy_minor()
        ),
    )?;
    std::fs::write(
        project_dir.join("jackdaw.toml"),
        format!(
            "[jackdaw]\nversion = \"{}\"\nbevy = \"{}\"\n",
            workspace_version(),
            bevy_minor()
        ),
    )?;
    // Without a target, `cargo metadata`, which the editor's PIE
    // pre-build runs at startup to resolve this project's package,
    // refuses the manifest outright ("no targets specified ... either
    // src/lib.rs, src/main.rs, a [lib] section, or [[bin]] section must
    // be present"), and the editor shows a permanent "Cannot build this
    // project" banner over every screenshot regardless of anything the
    // golden path does. An empty lib target is enough to satisfy it.
    std::fs::create_dir_all(project_dir.join("src"))?;
    std::fs::write(
        project_dir.join("src/lib.rs"),
        "//! Placeholder crate so `cargo metadata` resolves this scaffold; \
         the ux-audit fixture project has no game code of its own.\n",
    )?;
    write_fixture_materials(project_dir)
}

/// Two saved materials, so the material flow has something real to show.
///
/// Both bind the same generated base-colour image, so a slot, a swatch
/// and a preview all have actual content to render: the three material
/// surfaces are compared against each other, and blank squares would
/// compare equally well whatever the surfaces did.
///
/// Written in the exact shape `material_to_bsn` emits, fully-qualified type
/// paths and all, because that is what the loader reads back.
fn write_fixture_materials(project_dir: &Path) -> std::io::Result<()> {
    let dir = project_dir.join("assets/materials");
    std::fs::create_dir_all(&dir)?;
    let textures = project_dir.join("assets/textures");
    std::fs::create_dir_all(&textures)?;
    for (name, red, green, blue) in [
        ("audit_ground", 0.36, 0.42, 0.24),
        ("audit_rock", 0.48, 0.44, 0.41),
    ] {
        let texture = format!("textures/{name}_albedo.png");
        std::fs::write(
            project_dir.join("assets").join(&texture),
            checker_png(red, green, blue),
        )?;
        std::fs::write(
            dir.join(format!("{name}.material.bsn")),
            format!(
                "#{name}\nbevy_pbr::pbr_material::StandardMaterial {{\n    \
                 base_color: bevy_color::color::Color::Srgba(bevy_color::srgba::Srgba \
                 {{ red: {red}, green: {green}, blue: {blue}, alpha: 1.0 }}),\n    \
                 base_color_texture: \"{texture}\",\n    \
                 perceptual_roughness: 0.9,\n}}\n"
            ),
        )?;
    }
    Ok(())
}

/// Edge of the generated fixture texture, and of one checker square.
const CHECKER_PX: usize = 64;
const CHECKER_CELL: usize = 8;

/// A checkerboard PNG in the given tint, encoded by hand.
///
/// Hand-rolled because xtask is a build tool with no image dependency,
/// and one uncompressed RGB image is a few lines of framing rather than a
/// reason to grow its graph.
fn checker_png(red: f32, green: f32, blue: f32) -> Vec<u8> {
    let dark = |c: f32| (c * 255.0) as u8;
    let light = |c: f32| ((c * 0.45 + 0.55) * 255.0) as u8;

    let mut raw = Vec::with_capacity(CHECKER_PX * (1 + CHECKER_PX * 3));
    for y in 0..CHECKER_PX {
        raw.push(0); // filter: none
        for x in 0..CHECKER_PX {
            let lit = ((x / CHECKER_CELL) + (y / CHECKER_CELL)).is_multiple_of(2);
            let (r, g, b) = if lit {
                (light(red), light(green), light(blue))
            } else {
                (dark(red), dark(green), dark(blue))
            };
            raw.extend_from_slice(&[r, g, b]);
        }
    }

    let mut png = vec![0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a];
    let mut ihdr = Vec::new();
    ihdr.extend_from_slice(&(CHECKER_PX as u32).to_be_bytes());
    ihdr.extend_from_slice(&(CHECKER_PX as u32).to_be_bytes());
    // 8-bit RGB, no interlace.
    ihdr.extend_from_slice(&[8, 2, 0, 0, 0]);
    push_chunk(&mut png, b"IHDR", &ihdr);
    push_chunk(&mut png, b"IDAT", &zlib_stored(&raw));
    push_chunk(&mut png, b"IEND", &[]);
    png
}

fn push_chunk(out: &mut Vec<u8>, kind: &[u8; 4], data: &[u8]) {
    out.extend_from_slice(&(data.len() as u32).to_be_bytes());
    out.extend_from_slice(kind);
    out.extend_from_slice(data);
    let mut crc_input = kind.to_vec();
    crc_input.extend_from_slice(data);
    out.extend_from_slice(&crc32(&crc_input).to_be_bytes());
}

/// A zlib stream of stored (uncompressed) deflate blocks. Size is not
/// worth a compressor here: the fixture image is a few kilobytes.
fn zlib_stored(data: &[u8]) -> Vec<u8> {
    let mut out = vec![0x78, 0x01];
    let mut rest = data;
    while !rest.is_empty() {
        let take = rest.len().min(0xffff);
        let (block, tail) = rest.split_at(take);
        rest = tail;
        out.push(u8::from(rest.is_empty()));
        out.extend_from_slice(&(take as u16).to_le_bytes());
        out.extend_from_slice(&(!(take as u16)).to_le_bytes());
        out.extend_from_slice(block);
    }
    out.extend_from_slice(&adler32(data).to_be_bytes());
    out
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = 0xffff_ffffu32;
    for &byte in data {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            let mask = (crc & 1).wrapping_neg();
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn adler32(data: &[u8]) -> u32 {
    let (mut a, mut b) = (1u32, 0u32);
    for &byte in data {
        a = (a + u32::from(byte)) % 65521;
        b = (b + a) % 65521;
    }
    (b << 16) | a
}

fn dump_child_output(child: &mut std::process::Child) {
    use std::io::Read as _;
    if let Some(mut out) = child.stdout.take() {
        let mut buf = String::new();
        let _ = out.read_to_string(&mut buf);
        if !buf.is_empty() {
            eprintln!("ux-audit: editor stdout:\n{buf}");
        }
    }
    if let Some(mut err) = child.stderr.take() {
        let mut buf = String::new();
        let _ = err.read_to_string(&mut buf);
        if !buf.is_empty() {
            eprintln!("ux-audit: editor stderr:\n{buf}");
        }
    }
}

fn write_manifest(run_dir: &Path, expected: &[(usize, &Step, PathBuf)]) -> std::io::Result<()> {
    let path = run_dir.join("manifest.md");
    let mut f = std::fs::File::create(&path)?;

    writeln!(f, "# UX audit run")?;
    writeln!(f)?;
    writeln!(f, "- timestamp: {}", timestamp())?;
    writeln!(f, "- host: {}", host_triple())?;
    writeln!(f, "- jackdaw commit: {}", git_head())?;
    writeln!(
        f,
        "- display: {}",
        std::env::var("DISPLAY").unwrap_or_default()
    )?;
    writeln!(f)?;
    writeln!(
        f,
        "Every screenshot below is `window.screenshot` (src/screenshot.rs): \
         the whole editor window, palette/options-bar/Terrain-panel chrome \
         included, not just the 3D viewport."
    )?;
    writeln!(f)?;
    writeln!(f, "| step | name | status | screenshot / reason |")?;
    writeln!(f, "|---|---|---|---|")?;
    for (i, step) in STEPS.iter().enumerate() {
        let n = i + 1;
        match expected.iter().find(|(en, ..)| *en == n) {
            Some((_, _, file)) => {
                writeln!(
                    f,
                    "| {n:02} | {} | captured | {} |",
                    step.name,
                    file.file_name().unwrap().to_string_lossy()
                )?;
            }
            None => {
                writeln!(
                    f,
                    "| {n:02} | {} | skipped | {} |",
                    step.name, step.skip_reason
                )?;
            }
        }
    }
    Ok(())
}

fn host_triple() -> String {
    let out = Command::new("rustc").arg("-vV").output();
    out.ok()
        .and_then(|o| {
            String::from_utf8_lossy(&o.stdout)
                .lines()
                .find_map(|line| line.strip_prefix("host: ").map(str::to_string))
        })
        .unwrap_or_else(|| "unknown".to_string())
}

fn git_head() -> String {
    let out = Command::new("git")
        .args(["rev-parse", "HEAD"])
        .current_dir(workspace_root())
        .output();
    out.ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_else(|| "unknown".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Walk the zlib stream back out without using any of the writer's
    /// own code: check the header, follow the stored-block framing, and
    /// verify each block's LEN against its complement. Independent of
    /// [`zlib_stored`], so a framing bug there shows up here rather than
    /// cancelling out.
    fn inflate_stored(stream: &[u8]) -> Vec<u8> {
        assert_eq!(stream[0] & 0x0f, 8, "deflate compression method");
        assert_eq!(
            u16::from_be_bytes([stream[0], stream[1]]) % 31,
            0,
            "zlib header check bits"
        );

        let mut out = Vec::new();
        let mut at = 2;
        loop {
            let header = stream[at];
            assert_eq!(header >> 1 & 0b11, 0, "stored block");
            let final_block = header & 1 == 1;
            let len = u16::from_le_bytes([stream[at + 1], stream[at + 2]]);
            let nlen = u16::from_le_bytes([stream[at + 3], stream[at + 4]]);
            assert_eq!(nlen, !len, "LEN and NLEN must be complements");
            let start = at + 5;
            out.extend_from_slice(&stream[start..start + usize::from(len)]);
            at = start + usize::from(len);
            if final_block {
                break;
            }
        }
        assert_eq!(at + 4, stream.len(), "adler32 trailer follows the blocks");
        out
    }

    /// The fixture texture has to be a real PNG: the editor loads it
    /// through the asset server like any other, and a malformed one
    /// would leave every material swatch in the audit blank, which is what
    /// the texture exists to rule out.
    ///
    /// The chunk CRC is checked with the same routine that wrote it, so
    /// that assertion only catches framing slips, not a wrong CRC
    /// polynomial. The image data is checked by inflating it
    /// independently, which is where a real encoding bug would live.
    #[test]
    fn the_generated_texture_is_a_well_formed_png() {
        let png = checker_png(0.36, 0.42, 0.24);
        assert_eq!(&png[..8], &[0x89, b'P', b'N', b'G', 0x0d, 0x0a, 0x1a, 0x0a]);

        let mut at = 8;
        let mut kinds: Vec<String> = Vec::new();
        let mut idat: Vec<u8> = Vec::new();
        while at < png.len() {
            let len = u32::from_be_bytes(png[at..at + 4].try_into().unwrap()) as usize;
            let kind = &png[at + 4..at + 8];
            let body = &png[at + 4..at + 8 + len];
            let stated = u32::from_be_bytes(png[at + 8 + len..at + 12 + len].try_into().unwrap());
            assert_eq!(crc32(body), stated, "chunk CRC");
            if kind == b"IDAT" {
                idat.extend_from_slice(&png[at + 8..at + 8 + len]);
            }
            kinds.push(String::from_utf8_lossy(kind).into_owned());
            at += 12 + len;
        }
        assert_eq!(at, png.len(), "chunks must exactly cover the file");
        assert_eq!(kinds, vec!["IHDR", "IDAT", "IEND"]);

        // One filter byte plus three bytes per pixel, per row.
        let raw = inflate_stored(&idat);
        let stride = 1 + CHECKER_PX * 3;
        assert_eq!(raw.len(), CHECKER_PX * stride);
        for row in 0..CHECKER_PX {
            assert_eq!(raw[row * stride], 0, "row {row} declares the None filter");
        }
        // The checker has to alternate, or the texture is a flat square
        // and shows nothing about what the surfaces render.
        let pixel = |x: usize, y: usize| {
            let at = y * stride + 1 + x * 3;
            [raw[at], raw[at + 1], raw[at + 2]]
        };
        assert_ne!(pixel(0, 0), pixel(CHECKER_CELL, 0));
        assert_eq!(pixel(0, 0), pixel(CHECKER_CELL * 2, 0));
    }
}
