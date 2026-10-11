//! `cargo xtask bsn-parser [--check]`: regenerate the BSN parser from its
//! lalrpop grammar. The generated parser is committed so builds need no
//! lalrpop; `--check` fails when it no longer matches the grammar.

use std::path::{Path, PathBuf};

/// The directory holding `grammar.lalrpop` and the committed `grammar.rs`.
fn parse_dir() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("../crates/jackdaw_bsn/src/parse")
}

/// Regenerate `grammar.rs`, or with `check` only compare it with a fresh build.
pub fn cmd(check: bool) -> bool {
    let parse_dir = parse_dir();
    let out_dir = std::env::temp_dir().join(format!("jackdaw-bsn-parser-{}", std::process::id()));
    let generated = lalrpop::Configuration::new()
        .set_in_dir(&parse_dir)
        .set_out_dir(&out_dir)
        .force_build(true)
        .log_quiet()
        .process();
    if let Err(error) = generated {
        eprintln!("generate the BSN parser: {error}");
        return false;
    }
    let fresh = out_dir.join("grammar.rs");
    let committed = parse_dir.join("grammar.rs");
    let result = if check {
        let same = std::fs::read(&fresh).ok() == std::fs::read(&committed).ok();
        if !same {
            eprintln!(
                "{} does not match grammar.lalrpop; run `cargo xtask bsn-parser`",
                committed.display()
            );
        }
        same
    } else {
        match std::fs::copy(&fresh, &committed) {
            Ok(_) => true,
            Err(error) => {
                eprintln!("write {}: {error}", committed.display());
                false
            }
        }
    };
    let _ = std::fs::remove_dir_all(&out_dir);
    result
}
