//! Decides what the editor binary does with its command line before any
//! Bevy app is built.

/// What `main` should do for a given argv.
#[derive(Debug, PartialEq, Eq)]
pub enum Launch<'a> {
    Editor,
    Help,
    Version,
    ExportTerrain(&'a [String]),
    /// An option the editor does not accept.
    Unknown(&'a str),
}

pub const USAGE: &str = "\
jackdaw: the Jackdaw scene editor

Usage:
  jackdaw                       start the editor
  jackdaw export-terrain ...    export a scene's terrain (jackdaw export-terrain <scene.bsn> --out <dir>)
  jackdaw --version, -V         print the version and exit
  jackdaw --help, -h            print this help and exit

Environment:
  JACKDAW_OPEN_PROJECT=<dir>    open this project on startup

See `jd --help` for project and extension commands.";

/// `args` is the full argv including the program name.
pub fn decide(args: &[String]) -> Launch<'_> {
    match args.get(1).map(String::as_str) {
        Some("--help" | "-h") => Launch::Help,
        Some("--version" | "-V") => Launch::Version,
        Some("export-terrain") => Launch::ExportTerrain(&args[2..]),
        // macOS passes `-psn_<n>` when launching an app bundle.
        Some(flag) if flag.starts_with('-') && !flag.starts_with("-psn_") => Launch::Unknown(flag),
        _ => Launch::Editor,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn argv(args: &[&str]) -> Vec<String> {
        std::iter::once("jackdaw")
            .chain(args.iter().copied())
            .map(String::from)
            .collect()
    }

    #[test]
    fn help_flags_print_usage() {
        assert_eq!(decide(&argv(&["--help"])), Launch::Help);
        assert_eq!(decide(&argv(&["-h"])), Launch::Help);
    }

    #[test]
    fn version_flags_print_version() {
        assert_eq!(decide(&argv(&["--version"])), Launch::Version);
        assert_eq!(decide(&argv(&["-V"])), Launch::Version);
    }

    #[test]
    fn export_terrain_forwards_its_arguments() {
        let args = argv(&["export-terrain", "a.bsn", "--out", "d"]);
        assert_eq!(decide(&args), Launch::ExportTerrain(&args[2..]));
    }

    #[test]
    fn unknown_flags_are_rejected() {
        assert_eq!(decide(&argv(&["--bogus"])), Launch::Unknown("--bogus"));
    }

    #[test]
    fn no_arguments_and_bundle_launch_start_the_editor() {
        assert_eq!(decide(&argv(&[])), Launch::Editor);
        assert_eq!(decide(&argv(&["-psn_0_12345"])), Launch::Editor);
    }
}
