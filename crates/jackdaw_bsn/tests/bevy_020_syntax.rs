//! Scenes in Bevy 0.20 syntax, and the 0.19 scenes that migrate to it.

use jackdaw_bsn::syntax::{parse_stream::ParseStream, span::Span, types::BsnListRoot};
use jackdaw_bsn::{emit_scene, parse_bsn, parse_bsn_text};

const SCENE_019: &str = include_str!("fixtures/scene_bevy_019.bsn");
const SCENE_020: &str = include_str!("fixtures/scene_bevy_020.bsn");

/// Whether `text` follows the file syntax exactly, with no migration first.
fn is_strict_syntax(text: &str) -> bool {
    ParseStream::from(Span::from(text))
        .parse::<BsnListRoot>()
        .is_ok()
}

#[test]
fn a_019_scene_saves_as_020_syntax() {
    let ast = parse_bsn_text(SCENE_019).expect("the 0.19 scene migrates");
    assert_eq!(emit_scene(&ast), SCENE_020);
}

#[test]
fn a_020_scene_parses_without_migration() {
    assert!(is_strict_syntax(SCENE_020));
    parse_bsn(SCENE_020).expect("the 0.20 scene parses");
}

#[test]
fn a_019_scene_is_not_020_syntax() {
    assert!(!is_strict_syntax(SCENE_019));
}

#[test]
fn an_unstamped_019_scene_also_migrates() {
    let unstamped = SCENE_019
        .split_once('\n')
        .map(|(_, body)| body)
        .unwrap()
        .replace(
            "bevy_core_pipeline::tonemapping::Tonemapping",
            "bevy_render::view::Tonemapping",
        );
    let ast = parse_bsn_text(&unstamped).expect("the unstamped scene migrates");
    assert_eq!(emit_scene(&ast), SCENE_020);
}

#[test]
fn the_writer_separates_siblings_with_a_double_minus_line() {
    let ast = parse_bsn_text(SCENE_020).unwrap();
    let text = emit_scene(&ast);
    assert_eq!(text.lines().filter(|line| line.trim() == "--").count(), 2);
    assert!(!text.lines().any(|line| line.trim() == ","));
    assert!(is_strict_syntax(&text));
}

#[test]
fn a_migrated_scene_round_trips() {
    let first = emit_scene(&parse_bsn_text(SCENE_019).unwrap());
    let second = emit_scene(&parse_bsn_text(&first).unwrap());
    assert_eq!(first, second);
}

#[test]
fn names_that_are_not_identifiers_round_trip_quoted() {
    let names = ["Main Camera", "1st", "true", "inf", "map", "_hidden"];
    let roots: Vec<String> = names
        .iter()
        .map(|name| format!("#\"{name}\"\nmy_game::Marker\n"))
        .collect();
    let text = roots.join("--\n");
    let emitted = emit_scene(&parse_bsn_text(&text).unwrap());
    let again = emit_scene(&parse_bsn_text(&emitted).unwrap());
    assert_eq!(emitted, again);
    for bare in ["#map\n", "#_hidden\n"] {
        assert!(emitted.contains(bare), "{emitted}");
    }
    for quoted in ["#\"Main Camera\"", "#\"1st\"", "#\"true\"", "#\"inf\""] {
        assert!(emitted.contains(quoted), "{emitted}");
    }
}
