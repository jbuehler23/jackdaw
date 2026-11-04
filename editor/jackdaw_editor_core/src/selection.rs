pub use jackdaw_api_internal::selection::{
    Selected, Selection, clear_selection_in_world, select_for_edit, select_many, select_only,
};

use bevy::prelude::*;

pub struct SelectionPlugin;

impl Plugin for SelectionPlugin {
    fn build(&self, app: &mut App) {
        jackdaw_api_internal::selection::plugin(app);
    }
}
