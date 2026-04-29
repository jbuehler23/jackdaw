//! Single shared dylib bundling every jackdaw crate whose types
//! cross the extension boundary.
//!
//! Following the pattern of `bevy_dylib`, this crate:
//!
//! * Declares `crate-type = ["dylib"]`, so building it produces a
//!   single `.so` / `.dylib` / `.dll`.
//! * Has no original code; every type lives in the inner crates
//!   listed in `[dependencies]`. Cargo bundles their compiled rlib
//!   output into this dylib when it links.
//! * Is linked into both the editor binary (via
//!   `jackdaw_api`'s `dynamic_linking` feature) and every
//!   extension dylib (through the same feature on the extension
//!   side). That gives one shared copy of jackdaw's types across
//!   the boundary, which is what keeps `TypeId`-keyed resource
//!   lookups working.
//!
//! Users never import this crate directly; it's activated via
//! `jackdaw_api/dynamic_linking`.

// The `use` statements below exist purely to make sure the listed
// crates' symbols end up inside the produced dylib. Rust's linker
// drops unused transitive rlibs otherwise.
use jackdaw_api_internal as _;
use jackdaw_commands as _;
use jackdaw_panels as _;
