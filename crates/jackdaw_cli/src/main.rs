//! The standalone `jackdaw-cli` binary. All logic lives in the library so
//! the editor `jackdaw` package can expose the same command.

fn main() -> std::process::ExitCode {
    jackdaw_cli::run()
}
