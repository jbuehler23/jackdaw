use bevy_app::prelude::*;
use bevy_color::prelude::*;
use bevy_ecs::prelude::*;
use bevy_ecs::{query::QueryFilter, spawn::SpawnableList};
use bevy_feathers::cursor::{CursorIconPlugin, EntityCursor, OverrideCursor};
use bevy_picking::prelude::*;
use bevy_ui::prelude::*;
use bevy_utils::prelude::*;
use bevy_window::SystemCursorIcon;

const HANDLE_SIZE: f32 = 3.0;
const HANDLE_HOVER_COLOR: Color = Color::srgba(1.0, 1.0, 1.0, 0.12);

#[derive(Component)]
pub struct PanelGroup {
    pub min_ratio: f32,
}

#[derive(Component)]
pub struct Panel {
    pub ratio: f32,
}

#[derive(Component)]
pub struct PanelHandle;

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
        BackgroundColor::from(Color::NONE),
    )
}

pub struct SplitPanelPlugin;

impl Plugin for SplitPanelPlugin {
    fn build(&self, app: &mut App) {
        if !app.is_plugin_added::<CursorIconPlugin>() {
            app.add_plugins(CursorIconPlugin);
        }

        app.add_observer(on_panel_added)
            .add_observer(on_handle_added)
            .add_observer(on_handle_drag_start)
            .add_observer(on_handle_drag_end)
            .add_observer(set_background_on_with::<Pointer<Over>, With<PanelHandle>>(
                HANDLE_HOVER_COLOR,
            ))
            .add_observer(set_background_on_with::<Pointer<Out>, With<PanelHandle>>(
                Color::NONE,
            ))
            .add_observer(handle_panel_drag)
            .add_systems(Update, recalculate_changed_panels);
    }
}

fn on_panel_added(
    trigger: On<Add, Panel>,
    child_of: Query<&ChildOf>,
    mut queries: ParamSet<(
        Query<(&Node, &Children), With<PanelGroup>>,
        Query<(&mut Node, &Panel)>,
    )>,
) {
    let entity = trigger.event_target();
    let Ok(&ChildOf(parent)) = child_of.get(entity) else {
        return;
    };
    recalculate_group(parent, &mut queries);
}

fn recalculate_changed_panels(
    changed: Query<&ChildOf, Changed<Panel>>,
    mut queries: ParamSet<(
        Query<(&Node, &Children), With<PanelGroup>>,
        Query<(&mut Node, &Panel)>,
    )>,
) {
    let mut seen = std::collections::HashSet::new();
    for parent_ref in &changed {
        let parent = parent_ref.parent();
        if seen.insert(parent) {
            recalculate_group(parent, &mut queries);
        }
    }
}

fn recalculate_group(
    group_entity: Entity,
    queries: &mut ParamSet<(
        Query<(&Node, &Children), With<PanelGroup>>,
        Query<(&mut Node, &Panel)>,
    )>,
) {
    let groups = queries.p0();
    let Ok((group_node, children)) = groups.get(group_entity) else {
        return;
    };
    let flex_direction = group_node.flex_direction;
    let child_entities: Vec<Entity> = children.iter().collect();

    // Sum only visible panels. Hidden (Display::None) panels are out
    // of layout, so giving them a percentage steals space from siblings.
    let panels_ro = queries.p1();
    let total: f32 = panels_ro
        .iter_many(&child_entities)
        .filter(|(node, _)| node.display != Display::None)
        .map(|(_, panel)| panel.ratio)
        .sum();

    if total <= 0.0 {
        return;
    }

    let mut panels = queries.p1();
    let mut iterator = panels.iter_many_mut(&child_entities);
    while let Some((mut node, panel)) = iterator.fetch_next() {
        if node.display == Display::None {
            continue;
        }
        let pct = (panel.ratio / total) * 100.;
        match flex_direction {
            FlexDirection::Row | FlexDirection::RowReverse => {
                node.width = percent(pct);
                node.min_width = px(0.0);
            }
            FlexDirection::Column | FlexDirection::ColumnReverse => {
                node.height = percent(pct);
                node.min_height = px(0.0);
            }
        }
    }
}

fn on_handle_added(
    trigger: On<Add, PanelHandle>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    nodes: Query<&Node>,
    mut commands: Commands,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.entity) else {
        return;
    };

    let Ok(node) = nodes.get(parent) else {
        return;
    };

    let cursor_icon = get_drag_icon(node.flex_direction);

    commands
        .entity(trigger.entity)
        .insert(EntityCursor::System(cursor_icon));
}

fn on_handle_drag_start(
    trigger: On<Pointer<DragStart>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    nodes: Query<&Node>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.event_target()) else {
        return;
    };

    let Ok(node) = nodes.get(parent) else {
        return;
    };

    let cursor_icon = get_drag_icon(node.flex_direction);

    if override_cursor.is_none() {
        override_cursor.0 = Some(EntityCursor::System(cursor_icon));
    }
}

fn on_handle_drag_end(
    trigger: On<Pointer<DragEnd>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    nodes: Query<&Node>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.event_target()) else {
        return;
    };

    let Ok(node) = nodes.get(parent) else {
        return;
    };

    let cursor_icon = get_drag_icon(node.flex_direction);

    if override_cursor.0 == Some(EntityCursor::System(cursor_icon)) {
        override_cursor.0 = None;
    }
}

fn handle_panel_drag(
    mut drag: On<Pointer<Drag>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    groups: Query<(&PanelGroup, &Node, &ComputedNode, &Children)>,
    bindings: Query<&crate::reconcile::NodeBinding>,
    mut tree: ResMut<crate::tree::DockTree>,
    mut panels: Query<&mut Panel>,
) {
    let handle_entity = drag.event_target();
    let Ok(&ChildOf(parent)) = handles.get(handle_entity) else {
        return;
    };
    let Ok((group, node, computed, children)) = groups.get(parent) else {
        return;
    };

    let Some(handle_index) = children.iter().position(|e| e == handle_entity) else {
        return;
    };

    if handle_index == 0 || handle_index + 1 >= children.len() {
        return;
    }
    let before_entity = children[handle_index - 1];
    let after_entity = children[handle_index + 1];

    let logical_size = computed.size() * computed.inverse_scale_factor();
    let (total_px, delta_px) = match node.flex_direction {
        FlexDirection::Row | FlexDirection::RowReverse => (logical_size.x, drag.delta.x),
        FlexDirection::Column | FlexDirection::ColumnReverse => (logical_size.y, drag.delta.y),
    };

    if total_px <= 0.0 {
        return;
    }

    let total_ratio: f32 = panels.iter_many(children.iter()).map(|p| p.ratio).sum();

    let delta_ratio = (delta_px / total_px) * total_ratio;

    let Ok([mut before, mut after]) = panels.get_many_mut([before_entity, after_entity]) else {
        return;
    };

    let new_before = before.ratio + delta_ratio;
    let new_after = after.ratio - delta_ratio;

    if new_before < group.min_ratio || new_after < group.min_ratio {
        drag.propagate(false);
        return;
    }

    before.ratio = new_before;
    after.ratio = new_after;

    // If this PanelGroup is bound to a tree split, mirror the new fraction
    // into the tree so saved layouts and the reconciler stay in sync.
    if let Ok(binding) = bindings.get(parent) {
        let total = new_before + new_after;
        if total > 0.0 {
            tree.set_fraction(binding.0, new_before / total);
        }
    }

    drag.propagate(false);
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

fn get_drag_icon(direction: FlexDirection) -> SystemCursorIcon {
    match direction {
        FlexDirection::Row | FlexDirection::RowReverse => SystemCursorIcon::ColResize,
        FlexDirection::Column | FlexDirection::ColumnReverse => SystemCursorIcon::RowResize,
    }
}
