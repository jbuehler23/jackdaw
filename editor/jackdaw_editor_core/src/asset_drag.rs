//! Dragging a file out of the Project window: the path in flight and the
//! ghost card that follows the cursor until something consumes the drop.

use std::path::PathBuf;

use bevy::{
    picking::cursor::{EntityCursor, OverrideCursor},
    prelude::*,
    window::{PrimaryWindow, SystemCursorIcon},
};
use jackdaw_feathers::{
    file_browser,
    icons::{self, EditorFont, IconFont},
    tokens,
};

/// Path of the asset currently being dragged out of the Project window. Set by
/// a `PointerDragStart` observer on each entry, read by the viewport's drop
/// handler, and cleared after the drop (or by `DragEnd` if no drop happened).
#[derive(Resource, Default)]
pub struct ActiveAssetDrag {
    pub path: Option<PathBuf>,
    /// Set when the drag started on a 2D image tile. Dropping it on the
    /// viewport spawns a reference image plane at the drop point.
    pub image: Option<PathBuf>,
    /// The floating ghost entity that follows the cursor while a drag is in
    /// flight, keyed off `path` / `image`.
    pub ghost: Option<Entity>,
}

/// Marks the floating ghost node: a dimmed icon and label under the cursor.
#[derive(Component)]
struct AssetDragGhost;

pub struct AssetDragPlugin;

impl Plugin for AssetDragPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ActiveAssetDrag>().add_systems(
            Update,
            manage_asset_drag_ghost.run_if(in_state(crate::AppState::Editor)),
        );
    }
}

/// Drive the ghost and the grab cursor from the current [`ActiveAssetDrag`]:
/// while a drag is live, force a grabbing cursor and float a dimmed icon and
/// filename card under the pointer; tear both down when the drag ends.
fn manage_asset_drag_ghost(
    mut commands: Commands,
    mut drag: ResMut<ActiveAssetDrag>,
    mut cursor: ResMut<OverrideCursor>,
    windows: Query<&Window, With<PrimaryWindow>>,
    editor_font: Res<EditorFont>,
    icon_font: Res<IconFont>,
    mut ghost_nodes: Query<&mut Node, With<AssetDragGhost>>,
) {
    let active = drag.path.as_ref().or(drag.image.as_ref()).cloned();
    let cursor_pos = windows
        .single()
        .ok()
        .and_then(bevy::prelude::Window::cursor_position);

    let grabbing = Some(EntityCursor::System(SystemCursorIcon::Grabbing));
    if active.is_some() {
        if cursor.0.is_none() {
            cursor.0 = grabbing;
        }
    } else if cursor.0 == grabbing {
        cursor.0 = None;
    }

    match (active, drag.ghost) {
        (Some(path), None) => {
            let name = path
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let icon = file_browser::file_icon(&name);
            let pos = cursor_pos.unwrap_or(Vec2::ZERO);
            let ghost = spawn_asset_drag_ghost(
                &mut commands,
                &editor_font.0,
                &icon_font.0,
                icon,
                &name,
                pos,
            );
            drag.ghost = Some(ghost);
        }
        (Some(_), Some(ghost)) => {
            if let Some(pos) = cursor_pos
                && let Ok(mut node) = ghost_nodes.get_mut(ghost)
            {
                node.left = Val::Px(pos.x + 14.0);
                node.top = Val::Px(pos.y + 8.0);
            }
        }
        (None, Some(ghost)) => {
            commands.entity(ghost).try_despawn();
            drag.ghost = None;
        }
        (None, None) => {}
    }
}

/// Spawn the dimmed ghost card at `pos`. `Pickable::IGNORE` so it never
/// intercepts the drop target under the cursor.
fn spawn_asset_drag_ghost(
    commands: &mut Commands,
    font: &Handle<Font>,
    icon_font: &Handle<Font>,
    icon: icons::Icon,
    name: &str,
    pos: Vec2,
) -> Entity {
    commands
        .spawn((
            AssetDragGhost,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(pos.x + 14.0),
                top: Val::Px(pos.y + 8.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                padding: UiRect::axes(Val::Px(8.0), Val::Px(5.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(tokens::BORDER_RADIUS_MD)),
                max_width: Val::Px(220.0),
                overflow: Overflow::clip(),
                ..default()
            },
            BackgroundColor(tokens::PANEL_BG.with_alpha(0.85)),
            BorderColor::all(tokens::BORDER_SUBTLE.with_alpha(0.8)),
            GlobalZIndex(10_000),
            Pickable::IGNORE,
            children![
                (
                    Text::new(String::from(icon.unicode())),
                    TextFont {
                        font: icon_font.clone().into(),
                        font_size: tokens::ICON_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_SECONDARY.with_alpha(0.75)),
                    Pickable::IGNORE,
                ),
                (
                    Text::new(name.to_string()),
                    TextFont {
                        font: font.clone().into(),
                        font_size: tokens::TEXT_SIZE_SM,
                        ..default()
                    },
                    TextColor(tokens::TEXT_PRIMARY.with_alpha(0.75)),
                    Pickable::IGNORE,
                ),
            ],
        ))
        .id()
}
