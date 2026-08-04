//! Answering the editor's "what types do you have?" question from
//! inside the game.
//!
//! The request is an ordinary command line argument: the editor
//! runs it with [`SCHEMA_FLAG`], and [`JackdawPlugin`](crate::JackdawPlugin)
//! writes the schema to stdout and exits. Games do not need to handle
//! the flag themselves.

#![expect(
    clippy::print_stdout,
    reason = "the schema payload IS this mode's output; the editor reads it off stdout"
)]
#![expect(
    clippy::print_stderr,
    reason = "schema extraction failures must reach the editor via stderr"
)]

pub use jackdaw_schema::SCHEMA_FLAG;

/// Whether this process was asked to report its schema.
pub fn schema_extraction_requested() -> bool {
    std::env::args().any(|arg| arg == SCHEMA_FLAG)
}

/// This binary's reflected component and resource types, as the JSON
/// wire format the editor reads.
pub fn extract_schema_json() -> Result<String, serde_json::Error> {
    let schema = jackdaw_schema::extract_derived_schema();
    serde_json::to_string(&schema)
}

/// Print the schema and exit if the flag was passed; otherwise return so
/// startup continues.
pub fn extract_schema_and_exit_if_requested() {
    if !schema_extraction_requested() {
        return;
    }
    match extract_schema_json() {
        Ok(json) => {
            println!("{json}");
            std::process::exit(0);
        }
        Err(err) => {
            eprintln!("schema extraction failed: {err}");
            std::process::exit(1);
        }
    }
}
