//! A field added to the same type.
use bevy::prelude::*;

#[derive(Component, Reflect, Default)]
#[reflect(Component, Default)]
pub struct ShapeShifter {
    pub strength: f32,
    pub label: String,
}
