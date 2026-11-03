//! What a recipe computes before it touches a scene: its typed inputs, checked
//! against their kind and range, and the small expression language its call
//! parameters are written in.

pub mod expr;
pub mod input;

pub use expr::{Scope, Value, eval, is_expression};
pub use input::{InputKind, InputSpec, SEED_INPUT, value_text};
