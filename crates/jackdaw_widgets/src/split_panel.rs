use bevy::{
    feathers::cursor::{CursorIconPlugin, EntityCursor, OverrideCursor},
    prelude::*,
    window::SystemCursorIcon,
};
use bevy_monitors::prelude::{Mutation, NotifyChanged};

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
            .add_systems(Startup, setup_panel_watcher);
    }
}

fn setup_panel_watcher(mut commands: Commands) {
    commands
        .spawn(NotifyChanged::<Panel>::default())
        .observe(on_panel_mutated);
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

fn on_panel_mutated(
    trigger: On<Mutation<Panel>>,
    child_of: Query<&ChildOf>,
    mut queries: ParamSet<(
        Query<(&Node, &Children), With<PanelGroup>>,
        Query<(&mut Node, &Panel)>,
    )>,
) {
    let entity = trigger.mutated;
    let Ok(&ChildOf(parent)) = child_of.get(entity) else {
        return;
    };
    recalculate_group(parent, &mut queries);
}

fn recalculate_group(
    group_entity: Entity,
    queries: &mut ParamSet<(
        Query<(&Node, &Children), With<PanelGroup>>,
        Query<(&mut Node, &Panel)>,
    )>,
) {
    // Pass 1: read group info and panel ratios
    let groups = queries.p0();
    let Ok((group_node, children)) = groups.get(group_entity) else {
        return;
    };
    let flex_direction = group_node.flex_direction;
    let child_entities: Vec<Entity> = children.iter().collect();

    // Sum only visible panels; hidden ones (`Display::None`) are out
    // of flex layout, so giving them a percentage steals space from
    // siblings.
    let panels_ro = queries.p1();
    let total: f32 = panels_ro
        .iter_many(&child_entities)
        .filter(|(node, _)| node.display != Display::None)
        .map(|(_, panel)| panel.ratio)
        .sum();

    if total <= 0.0 {
        return;
    }

    // Pass 2: mutate panel nodes; skip hidden so we don't overwrite
    // the zero-size set by a collapse toggle.
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
    nodes: Query<(&Children, &Node)>,
    mut commands: Commands,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.entity) else {
        return;
    };

    let Ok((children, node)) = nodes.get(parent) else {
        return;
    };

    let index = children
        .iter()
        .position(|e| e == trigger.entity)
        .unwrap_or(0);

    let cursor_icon = get_drag_icon(node.flex_direction, index, children.len());

    commands
        .entity(trigger.entity)
        .insert(EntityCursor::System(cursor_icon));
}

fn on_handle_drag_start(
    trigger: On<Pointer<DragStart>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    nodes: Query<(&Children, &Node)>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.event_target()) else {
        return;
    };

    let Ok((children, node)) = nodes.get(parent) else {
        return;
    };

    let index = children
        .iter()
        .position(|e| e == trigger.entity)
        .unwrap_or(0);

    let cursor_icon = get_drag_icon(node.flex_direction, index, children.len());

    // This is a low priority override, so if anything else is overriding the cursor, we don't need to
    if override_cursor.is_none() {
        override_cursor.0 = Some(EntityCursor::System(cursor_icon));
    }
}

fn on_handle_drag_end(
    trigger: On<Pointer<DragEnd>>,
    handles: Query<&ChildOf, With<PanelHandle>>,
    nodes: Query<(&Children, &Node)>,
    mut override_cursor: ResMut<OverrideCursor>,
) {
    let Ok(&ChildOf(parent)) = handles.get(trigger.event_target()) else {
        return;
    };

    let Ok((children, node)) = nodes.get(parent) else {
        return;
    };

    let index = children
        .iter()
        .position(|e| e == trigger.entity)
        .unwrap_or(0);

    let cursor_icon = get_drag_icon(node.flex_direction, index, children.len());

    if override_cursor.0 == Some(EntityCursor::System(cursor_icon)) {
        override_cursor.0 = None;
    }
}

fn get_drag_icon(direction: FlexDirection, index: usize, count: usize) -> SystemCursorIcon {
    let is_right_half = index > count / 2;
    match (direction, is_right_half) {
        (FlexDirection::Row, false) | (FlexDirection::RowReverse, true) => {
            SystemCursorIcon::EResize
        }
        (FlexDirection::Row, true) | (FlexDirection::RowReverse, false) => {
            SystemCursorIcon::WResize
        }
        (FlexDirection::Column, false) | (FlexDirection::ColumnReverse, true) => {
            SystemCursorIcon::NResize
        }
        (FlexDirection::Column, true) | (FlexDirection::ColumnReverse, false) => {
            SystemCursorIcon::SResize
        }
    }
}
