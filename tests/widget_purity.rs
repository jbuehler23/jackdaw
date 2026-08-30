//! Every control in the editor is a `bevy_feathers` / `bevy_ui_widgets`
//! widget. The legacy `bevy_ui::widget::Button` and the `Interaction`
//! state it requires are the shape a hand-rolled control takes, so a
//! source scan for them is the cheapest way to keep new ones out.
//!
//! The scan is textual and deliberately narrow: it looks for the spawn
//! shape (a bare tuple element), not for every mention of the type, so
//! reading `Interaction` in a system the editor does not own still
//! compiles and still reads.

use std::path::{Path, PathBuf};

/// Files that still spawn a legacy control, with the reason each one is
/// still here. Paths are relative to the repository root.
const ALLOWED: &[(&str, &str)] = &[
    (
        "src/game_panel.rs",
        "segmented view switch; the radio-group widget it belongs on is a later pass",
    ),
    (
        "src/viewport_host.rs",
        "segmented view switch; the radio-group widget it belongs on is a later pass",
    ),
    (
        "src/viewport_2d.rs",
        "segmented view switch; the radio-group widget it belongs on is a later pass",
    ),
    (
        "src/layout.rs",
        "the Scene/Live segmented toggle; the radio-group widget it belongs on is a later pass",
    ),
    (
        "src/inspector/node_card.rs",
        "segmented anchor switch; the radio-group widget it belongs on is a later pass",
    ),
    (
        "crates/jackdaw_feathers/src/panel_header.rs",
        "the panel tab bar; the tab widget it belongs on is a later pass",
    ),
    (
        "crates/jackdaw_feathers/src/text_edit.rs",
        "the text field's own click and drag hitboxes; the feathers text input is a later pass",
    ),
    (
        "crates/jackdaw_feathers/src/dialog.rs",
        "the backdrop and panel read presses to tell a click outside the dialog from one inside; neither is a control",
    ),
    (
        "crates/jackdaw_feathers/src/toast.rs",
        "the toast body blocks presses from reaching what is behind it; it is not a control",
    ),
    (
        "crates/jackdaw_feathers/src/popover.rs",
        "the popover body blocks presses from reaching what is behind it; it is not a control",
    ),
];

/// Directories the scan covers.
const ROOTS: &[&str] = &["src", "crates/jackdaw_feathers/src"];

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn rust_files(dir: &Path, out: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            rust_files(&path, out);
        } else if path.extension().is_some_and(|ext| ext == "rs") {
            out.push(path);
        }
    }
}

/// Is `line` a spawn of a legacy control? A bare `Button` names the
/// headless widget in a file that imports it, so `headless` says which
/// of the two a bare name resolves to.
fn legacy_control(line: &str, headless: bool) -> Option<&'static str> {
    let trimmed = line.trim();
    match trimmed {
        "Interaction::default()," | "Interaction::None," => Some("an `Interaction` state"),
        "Button," if !headless => Some("a legacy `bevy_ui::widget::Button`"),
        _ => {
            if line.contains("bevy::ui::widget::Button") || line.contains("bevy::prelude::Button") {
                Some("a legacy `bevy_ui::widget::Button`")
            } else {
                None
            }
        }
    }
}

/// Does this source pull in the headless `bevy_ui_widgets::Button`? The
/// import may be wrapped across lines, so the whitespace goes first.
fn imports_headless_button(source: &str) -> bool {
    let dense: String = source.chars().filter(|c| !c.is_whitespace()).collect();
    dense.contains("ui_widgets::Button") || dense.contains("ui_widgets::{Button")
}

#[test]
fn no_control_is_spawned_on_the_legacy_button() {
    let root = repo_root();
    let mut files = Vec::new();
    for dir in ROOTS {
        rust_files(&root.join(dir), &mut files);
    }
    assert!(!files.is_empty(), "the scan found sources to read");

    let mut offenders = Vec::new();
    for file in files {
        let relative = file
            .strip_prefix(&root)
            .unwrap_or(&file)
            .to_string_lossy()
            .replace('\\', "/");
        if ALLOWED.iter().any(|(path, _)| *path == relative) {
            continue;
        }
        let Ok(source) = std::fs::read_to_string(&file) else {
            continue;
        };
        let headless = imports_headless_button(&source);
        for (index, line) in source.lines().enumerate() {
            if let Some(what) = legacy_control(line, headless) {
                offenders.push(format!("{relative}:{}: {what}", index + 1));
            }
        }
    }

    assert!(
        offenders.is_empty(),
        "these spawn a control the editor has a feathers widget for:\n{}",
        offenders.join("\n"),
    );
}

/// Every allowlist entry names a file that exists and still needs to be
/// there, so the list shrinks as the migration lands rather than
/// collecting dead rows.
#[test]
fn every_allowlist_entry_is_still_earning_its_place() {
    let root = repo_root();
    for (path, reason) in ALLOWED {
        let file = root.join(path);
        assert!(file.exists(), "`{path}` is on the allowlist but is gone");
        assert!(!reason.is_empty(), "`{path}` has no reason");
        let source = std::fs::read_to_string(&file).expect("the allowed file reads");
        let headless = imports_headless_button(&source);
        assert!(
            source
                .lines()
                .any(|line| legacy_control(line, headless).is_some()),
            "`{path}` no longer spawns a legacy control; drop its allowlist row",
        );
    }
}
