use bevy_app::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::{prelude::*, query::QueryFilter, spawn::SpawnableList};
use bevy_picking::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use jackdaw_widgets::split_panel::{Panel, PanelGroup, PanelHandle};

const HANDLE_SIZE: f32 = 3.0;

pub fn panel_group<C: SpawnableList<ChildOf> + Send + Sync + 'static>(
    min_ratio: f32,
    panels: C,
) -> impl Bundle {
    (PanelGroup { min_ratio }, Children::spawn(panels))
}

pub fn panel(ratio: impl ValNum) -> impl Bundle {
    Panel {
        ratio: ratio.val_num_f32(),
    }
}

pub fn panel_handle() -> impl Bundle {
    (
        PanelHandle,
        Node {
            min_width: px(HANDLE_SIZE),
            min_height: px(HANDLE_SIZE),
            ..default()
        },
        // Transparent so the dark window background shows through as the gap
        BackgroundColor::from(Color::NONE),
    )
}

pub struct SplitPanelPlugin;

impl Plugin for SplitPanelPlugin {
    fn build(&self, app: &mut App) {
        app.add_observer(set_background_on_with::<Pointer<Over>, With<PanelHandle>>(
            crate::tokens::PANEL_BORDER,
        ))
        .add_observer(set_background_on_with::<Pointer<Out>, With<PanelHandle>>(
            Color::NONE,
        ))
        .add_observer(handle_panel_drag);
    }
}

fn set_background_on_with<E: EntityEvent, F: QueryFilter>(
    color: Color,
) -> impl Fn(On<E>, Commands, Query<(), F>) {
    move |event, mut commands, filter| {
        if filter.contains(event.event_target()) {
            commands
                .entity(event.event_target())
                .insert(BackgroundColor(color));
        }
    }
}

fn handle_panel_drag(
    mut drag: On<Pointer<Drag>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    groups: Query<(&PanelGroup, &Node, &ComputedNode, &Children)>,
    mut panels: Query<&mut Panel>,
) {
    let handle_entity = drag.event_target();
    let Ok(&ChildOf(parent)) = handles.get(handle_entity) else {
        return;
    };
    let Ok((group, node, computed, children)) = groups.get(parent) else {
        return;
    };

    // Find handle index in children list
    let Some(handle_index) = children.iter().position(|e| e == handle_entity) else {
        return;
    };

    // Adjacent panels: one before the handle, one after
    if handle_index == 0 || handle_index + 1 >= children.len() {
        return;
    }
    let before_entity = children[handle_index - 1];
    let after_entity = children[handle_index + 1];

    // Compute delta in ratio space
    let logical_size = computed.size() * computed.inverse_scale_factor();
    let (total_px, delta_px) = match node.flex_direction {
        FlexDirection::Row | FlexDirection::RowReverse => (logical_size.x, drag.delta.x),
        FlexDirection::Column | FlexDirection::ColumnReverse => (logical_size.y, drag.delta.y),
    };

    if total_px <= 0.0 {
        return;
    }

    // Sum ratios of all panels in this group
    let total_ratio: f32 = panels.iter_many(children.iter()).map(|p| p.ratio).sum();

    let delta_ratio = (delta_px / total_px) * total_ratio;

    let Ok([mut before, mut after]) = panels.get_many_mut([before_entity, after_entity]) else {
        return;
    };

    let new_before = before.ratio + delta_ratio;
    let new_after = after.ratio - delta_ratio;

    // Clamp: reject if either panel would go below min_ratio
    if new_before < group.min_ratio || new_after < group.min_ratio {
        drag.propagate(false);
        return;
    }

    before.ratio = new_before;
    after.ratio = new_after;

    drag.propagate(false);
}
