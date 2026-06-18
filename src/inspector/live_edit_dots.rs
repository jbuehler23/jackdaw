use crate::default_style;
use crate::prelude::*;

use bevy::prelude::*;

use bevy::picking::hover::Hovered;

use super::InspectorFieldRow;

/// Marker on the live-edit dot rendered next to an inspector field row
/// whose value has been streamed to the running game and is tracked in
/// the [`LiveEditLog`](crate::live_edits::LiveEditLog). Carries the log
/// key so the click menu can save or revert the tracked entry.
#[derive(Component, Clone)]
pub(crate) struct LiveEditDot {
    pub(crate) key: crate::live_edits::LiveEditKey,
}

/// Bookkeeping on a field row currently showing a live-edit dot:
/// the absolutely-positioned wrapper (for teardown) and the dot itself
/// (for key comparison on refresh).
#[derive(Component)]
pub(crate) struct LiveEditDotWrapper {
    wrapper: Entity,
    dot: Entity,
}

pub(crate) const LIVE_EDIT_SAVE_ACTION: &str = "inspector.live_edit.save";
pub(crate) const LIVE_EDIT_REVERT_ACTION: &str = "inspector.live_edit.revert";

/// Log key captured when a live-edit dot opens its context menu. Only
/// one inspector context menu can be open at a time, so a single-slot
/// resource is enough (same shape as `PrefabMenuTarget`).
#[derive(Resource, Default)]
pub(crate) struct LiveEditMenuTarget {
    pub(crate) key: Option<crate::live_edits::LiveEditKey>,
}

/// Whether a tracked entry's field path belongs to a field row. Rows
/// for composite fields (`translation`) own the per-axis entries the
/// live dispatch records (`translation.x`).
fn live_entry_matches_row(entry_path: &str, row_path: &str) -> bool {
    entry_path == row_path
        || (entry_path.len() > row_path.len()
            && entry_path.starts_with(row_path)
            && entry_path.as_bytes()[row_path.len()] == b'.')
}

/// Keep every inspector field row's live-edit dot in sync with the
/// [`LiveEditLog`](crate::live_edits::LiveEditLog): a dot appears on
/// the first tracked edit of a field, follows the entry when it is
/// saved / reverted / discarded, and vanishes when the log clears.
/// Runs only when the log or projection changed (or rows respawned),
/// and spawns / despawns nothing when the row already matches.
pub(crate) fn refresh_live_edit_field_dots(
    log: Res<crate::live_edits::LiveEditLog>,
    projection: Res<crate::pie_projection::PieProjection>,
    rows: Query<(Entity, &InspectorFieldRow, Option<&LiveEditDotWrapper>)>,
    new_rows: Query<(), Added<InspectorFieldRow>>,
    dots: Query<&LiveEditDot>,
    mut commands: Commands,
) {
    if !log.is_changed() && !projection.is_changed() && new_rows.is_empty() {
        return;
    }
    for (row_entity, row, marker) in &rows {
        let bits = crate::live_edits::live_bits_for_preview(&projection, row.source_entity);
        let found = bits.and_then(|bits| {
            log.entries.iter().find(|(key, _)| {
                key.bits == bits
                    && key.type_path == row.type_path
                    && live_entry_matches_row(&key.field_path, &row.field_path)
            })
        });
        let Some((key, entry)) = found else {
            if let Some(marker) = marker {
                if let Ok(mut ec) = commands.get_entity(marker.wrapper) {
                    ec.despawn();
                }
                commands.entity(row_entity).remove::<LiveEditDotWrapper>();
            }
            continue;
        };
        if let Some(marker) = marker {
            if dots.get(marker.dot).is_ok_and(|dot| dot.key == *key) {
                continue;
            }
            if let Ok(mut ec) = commands.get_entity(marker.wrapper) {
                ec.despawn();
            }
        }
        spawn_live_edit_dot(&mut commands, row_entity, key, entry);
    }
}

/// Spawn the live-edit dot on one field row: same absolutely-positioned
/// wrapper approach as the prefab override dot, anchored one slot to
/// its left so both can show on the same row. Hovering surfaces the
/// authored value; a primary click opens the save / revert menu.
fn spawn_live_edit_dot(
    commands: &mut Commands,
    row_entity: Entity,
    key: &crate::live_edits::LiveEditKey,
    entry: &crate::live_edits::LiveEditEntry,
) {
    let wrapper = commands
        .spawn(Node {
            position_type: PositionType::Absolute,
            top: Val::Px(2.0),
            right: Val::Px(34.0),
            ..default()
        })
        .id();

    let baseline = crate::live_edits::truncate_json_for_display(entry.baseline.as_ref());
    let dot = commands
        .spawn((
            LiveEditDot { key: key.clone() },
            Node {
                width: Val::Px(8.0),
                height: Val::Px(8.0),
                border_radius: BorderRadius::all(Val::Px(4.0)),
                ..default()
            },
            BackgroundColor(default_style::LIVE_EDIT_ACCENT),
            Hovered::default(),
            jackdaw_feathers::tooltip::Tooltip::title(format!("scene: {baseline}")),
        ))
        .id();

    commands.entity(dot).observe(
        |click: On<Pointer<Click>>,
         dots: Query<&LiveEditDot>,
         mut commands: Commands,
         windows: Query<&Window>,
         mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
         mut target: ResMut<LiveEditMenuTarget>| {
            if click.event().button != PointerButton::Primary {
                return;
            }
            let Ok(dot_data) = dots.get(click.event_target()) else {
                return;
            };
            let cursor_pos = windows
                .single()
                .ok()
                .and_then(bevy::prelude::Window::cursor_position)
                .unwrap_or_default();
            if let Some(existing) = state.menu_entity.take()
                && let Ok(mut ec) = commands.get_entity(existing)
            {
                ec.despawn();
            }
            target.key = Some(dot_data.key.clone());
            let items: [(&str, &str); 2] = [
                (LIVE_EDIT_SAVE_ACTION, "Save to Scene"),
                (LIVE_EDIT_REVERT_ACTION, "Revert"),
            ];
            let menu = jackdaw_feathers::context_menu::spawn_context_menu(
                &mut commands,
                cursor_pos,
                None,
                &items,
            );
            state.menu_entity = Some(menu);
        },
    );

    jackdaw_feathers::utils::attach_or_despawn(commands, wrapper, dot);
    jackdaw_feathers::utils::attach_or_despawn(commands, row_entity, wrapper);
    commands
        .entity(row_entity)
        .insert(LiveEditDotWrapper { wrapper, dot });
}

/// Observer: dispatch the live-edit save / revert operators when the
/// dot's context menu fires. The struct key travels through
/// `LiveEditLog.pending_action`, the same channel the Live Changes
/// tray uses, so the operators stay parameter-free.
pub(crate) fn on_live_edit_menu_action(
    event: On<jackdaw_widgets::context_menu::ContextMenuAction>,
    mut commands: Commands,
    mut state: ResMut<jackdaw_widgets::context_menu::ContextMenuState>,
    mut target: ResMut<LiveEditMenuTarget>,
    mut log: ResMut<crate::live_edits::LiveEditLog>,
) {
    let action = match event.action.as_str() {
        LIVE_EDIT_SAVE_ACTION => crate::live_edits::LiveEditAction::Save,
        LIVE_EDIT_REVERT_ACTION => crate::live_edits::LiveEditAction::Revert,
        _ => return,
    };
    let Some(key) = target.key.take() else {
        return;
    };
    log.pending_action = Some((action, key));
    let operator_id = match action {
        crate::live_edits::LiveEditAction::Save => crate::live_edits::PieLiveEditSaveOp::ID,
        crate::live_edits::LiveEditAction::Revert => crate::live_edits::PieLiveEditRevertOp::ID,
    };
    commands
        .operator(operator_id)
        .settings(CallOperatorSettings {
            execution_context: ExecutionContext::Invoke,
            creates_history_entry: false,
        })
        .call();

    // Close the menu after dispatching.
    if let Some(menu) = state.menu_entity.take()
        && let Ok(mut ec) = commands.get_entity(menu)
    {
        ec.despawn();
    }
    state.target_entity = None;
}
