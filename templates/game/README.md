# {{project-name}}

A Bevy game scaffolded by jackdaw.

## Running

`cargo run` runs the standalone game.

`jackdaw open .` opens this project in the jackdaw editor (assumes `jackdaw` is installed globally).

## Hot reload

Inside the editor, run `cargo build` (in your terminal or IDE) while playing to reload your changes. The editor watches `target/debug/` for build artifacts and reloads automatically.

## Project layout

Single crate, vanilla Bevy. Components, resources, and systems can live anywhere; the only jackdaw-specific line is the `export_game_plugin!` at the bottom of `src/lib.rs`.
