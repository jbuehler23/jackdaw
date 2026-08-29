//! The viewport panel's mode: whether it shows the 3D world or the 2D canvas.
//!
//! One panel authors both, so the mode is the panel's own state rather than a
//! second panel. This module owns the mode itself, what a scene kind asks for,
//! the resources that carry a mode request across a frame boundary, and the
//! panel's identity: [`ViewportHost`], which names the two presentation
//! subtrees and says which of them the user is looking at.
//!
//! The presentations themselves stay in [`crate::viewport`] (a `SceneViewport`
//! projecting a 3D camera) and [`crate::viewport_2d`] (a zoomable stage showing
//! a 2D camera's image). Both are built for every panel and both keep their own
//! state component on the panel entity; the mode decides which column is
//! displayed and which camera renders.

use bevy::{prelude::*, ui_widgets::observe};
use jackdaw_feathers::tokens;

use crate::prelude::*;
use crate::scenes::operators::SceneKind;
use crate::viewport::{CameraFlyActive, ViewportPanelHost};
use crate::viewport_2d::{Ui2dPanActive, Viewport2dPanelHost};

/// What a viewport panel is showing.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ViewportMode {
    /// The 3D world: perspective camera, grid, world-space tools.
    #[default]
    ThreeD,
    /// The 2D canvas: an orthographic stage at the authored reference size.
    TwoD,
}

impl ViewportMode {
    /// The mode a scene of this kind is authored in. A 2D world scene and a
    /// UI screen are both drawn flat, so both open on the canvas.
    pub fn for_scene_kind(kind: SceneKind) -> Self {
        match kind {
            SceneKind::ThreeD => Self::ThreeD,
            SceneKind::TwoD | SceneKind::Ui => Self::TwoD,
        }
    }

    /// The mode a `mode` operator parameter names, or `None` when it names
    /// neither. Case and surrounding space are ignored, as they are for the
    /// other viewport parameters a scripted run writes by hand.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "3d" => Some(Self::ThreeD),
            "2d" => Some(Self::TwoD),
            _ => None,
        }
    }
}

/// The mode the active scene tab wants its viewport panels in.
///
/// Panel-independent on purpose: two panels open at once share one mode, so a
/// tab restored from its view state puts every panel in the mode the user last
/// chose rather than trying to remember which panel the choice was made in.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportModeIntent {
    pub mode: ViewportMode,
    /// Whether the user picked this mode, as opposed to it following from the
    /// scene's kind. A chosen mode is stored in the tab's view state and
    /// restored on the next activation; an unchosen one is recomputed.
    pub chosen: bool,
}

/// A mode request made while the dock had no viewport leaf to honour it on.
///
/// A scene created before the reconciler has built the layout asks for a mode
/// on an empty tree. The request is held here rather than dropped, and honoured
/// on the first frame a leaf exists.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingViewportFocus(pub ViewportMode);

/// A viewport panel, on its dock-leaf content entity.
///
/// The panel's identity: which mode it is in and where its two presentation
/// subtrees are. `ViewportPanelHost` and `Viewport2dPanelHost` sit beside it on
/// the same entity holding each presentation's own state.
#[derive(Component, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportHost {
    pub mode: ViewportMode,
    /// Whether [`Self::mode`] is one the user picked rather than one the
    /// scene's kind implied. Travels into the tab's view state on a swap.
    pub mode_chosen: bool,
    /// Root of the 3D presentation's column.
    pub three_d: Entity,
    /// Root of the 2D presentation's column.
    pub two_d: Entity,
}

/// Build a viewport panel in the mode `intent` asks for.
///
/// Both presentations are built whatever the mode is, and the mode shows one
/// and hides the other. Building only the wanted one would make a switch a
/// panel rebuild, which would drop the camera pose, the canvas framing and the
/// per-panel chrome every time the user flipped between them.
pub fn build_viewport_panel_in(world: &mut World, parent: Entity, intent: ViewportModeIntent) {
    let three_d = crate::viewport::build_3d_presentation(world, parent);
    let two_d = crate::viewport_2d::build_2d_presentation(world, parent);
    world.entity_mut(parent).insert(ViewportHost {
        mode: intent.mode,
        mode_chosen: intent.chosen,
        three_d,
        two_d,
    });
    // Directly, rather than waiting for the scheduled pass: a panel that spent
    // its first frame showing both columns would show the 2D stage stacked
    // under the 3D toolbar.
    if let Err(err) = world.run_system_cached(apply_viewport_mode) {
        error!("failed to apply the viewport mode to a new panel: {err}");
    }
}

/// Show the column the mode names, hide the other, and let only the shown
/// one's camera render.
///
/// Nothing is despawned: the chrome, the camera pose and the canvas framing all
/// belong to the panel and outlive a switch. `Display::None` is what takes the
/// hidden column out of layout; Bevy clamps a zero-sized `ViewportNode`'s
/// render target to one pixel rather than refusing it, and the camera is
/// switched off in the same pass, so nothing draws into that pixel.
///
/// An inactive camera still gets its target's size, so a UI root parked on the
/// hidden 2D camera stays laid out at its authored reference resolution.
pub(crate) fn apply_viewport_mode(
    hosts: Query<
        (
            Entity,
            &ViewportHost,
            &ViewportPanelHost,
            &Viewport2dPanelHost,
        ),
        Changed<ViewportHost>,
    >,
    mut nodes: Query<&mut Node>,
    mut cameras: Query<&mut Camera>,
    mut fly: ResMut<CameraFlyActive>,
    mut panning: ResMut<Ui2dPanActive>,
) {
    for (entity, host, three_d, two_d) in &hosts {
        let shows_3d = host.mode == ViewportMode::ThreeD;
        for (column, shown) in [(host.three_d, shows_3d), (host.two_d, !shows_3d)] {
            let Ok(mut node) = nodes.get_mut(column) else {
                continue;
            };
            let display = if shown { Display::Flex } else { Display::None };
            if node.display != display {
                node.display = display;
            }
        }
        for (camera, active) in [(three_d.camera, shows_3d), (two_d.camera, !shows_3d)] {
            if let Ok(mut camera) = cameras.get_mut(camera)
                && camera.is_active != active
            {
                camera.is_active = active;
            }
        }
        // A gesture cannot outlive the presentation it was started on.
        if shows_3d {
            if panning.0 == Some(entity) {
                panning.0 = None;
            }
        } else {
            fly.0 = false;
        }
    }
}

/// Put every open panel, and the tab's intent, in `mode`.
///
/// `chosen` says who asked: the user, through the switch or the operator, or
/// the scene's kind on activation. Only a chosen mode is stored in the tab's
/// view state, so a tab the user never switched follows its kind for good.
///
/// The dock is untouched, so this costs no reconcile; use [`focus_viewport`]
/// when the panel also has to come forward.
pub fn set_viewport_mode(world: &mut World, mode: ViewportMode, chosen: bool) {
    world.insert_resource(ViewportModeIntent { mode, chosen });
    let mut hosts = world.query::<&mut ViewportHost>();
    for mut host in hosts.iter_mut(world) {
        if host.mode != mode || host.mode_chosen != chosen {
            host.mode = mode;
            host.mode_chosen = chosen;
        }
    }
}

/// Put the viewport in `mode` and bring its tab forward.
///
/// Best effort on the dock: a workspace with no viewport panel is one the user
/// arranged that way, and nothing here adds the panel back. With no leaf to
/// front yet the request is held in [`PendingViewportFocus`] instead. The mode
/// is set either way, so panels already open follow immediately.
pub fn focus_viewport(world: &mut World, mode: ViewportMode) {
    set_viewport_mode(world, mode, false);
    if !front_viewport_tab(world) {
        world.insert_resource(PendingViewportFocus(mode));
    }
}

/// Bring the viewport's tab forward, reporting whether the dock had one to
/// bring.
fn front_viewport_tab(world: &mut World) -> bool {
    use jackdaw_panels::tree::{DockNode, DockTree};

    let Some(mut tree) = world.get_resource_mut::<DockTree>() else {
        return false;
    };
    let Some(leaf_id) = tree.find_leaf_with_window(crate::viewport::VIEWPORT_WINDOW_ID) else {
        return false;
    };
    let Some(leaf) = tree.get(leaf_id).and_then(DockNode::as_leaf) else {
        return false;
    };
    let Some(tab) = leaf
        .tabs()
        .find_map(|(window, tab)| (window == crate::viewport::VIEWPORT_WINDOW_ID).then_some(tab))
    else {
        return false;
    };
    // `set_active` writes unconditionally, and any write to the `DockTree`
    // resource re-runs the reconciler over the whole tree. Switching between
    // two tabs that both want the viewport must not pay for that when it is
    // already the panel in front.
    if leaf.active != Some(tab) {
        tree.set_active(leaf_id, tab);
    }
    true
}

/// Honour a held request once the dock has a viewport leaf to honour it on,
/// and only then drop it.
///
/// The mode is written again here, not just the front: a request is held
/// exactly when no leaf existed, so the panels that answer it are built
/// afterwards, in whatever mode their window descriptor starts them in.
fn apply_pending_viewport_focus(world: &mut World) {
    let Some(pending) = world.get_resource::<PendingViewportFocus>().copied() else {
        return;
    };
    if front_viewport_tab(world) {
        set_viewport_mode(world, pending.0, false);
        world.remove_resource::<PendingViewportFocus>();
    }
}

/// Marker on one segment of the 3D|2D control, naming the panel it switches.
///
/// The panel is carried rather than looked up, for the reason
/// [`crate::viewport_2d::Viewport2dModeSegment`] carries one: a segment in one
/// panel's bar must never move another panel's.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportModeSegment {
    pub host: Entity,
    pub mode: ViewportMode,
}

/// The two-segment 3D|2D control, built like the Edit|Interact bar beside it
/// (`crate::viewport_2d::viewport_2d_mode_bar`) and the Game panel's
/// Play/Select bar.
///
/// One is spawned into each presentation's bar, so whichever bar the mode is
/// showing carries the way back out of it.
pub(crate) fn viewport_mode_bar(host: Entity) -> impl Bundle {
    (
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            border: UiRect::all(px(1)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            overflow: Overflow::clip(),
            flex_shrink: 0.0,
            ..default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        children![
            viewport_mode_segment(
                host,
                ViewportMode::ThreeD,
                "3D",
                "3D: author the scene in the world viewport",
            ),
            viewport_mode_segment(
                host,
                ViewportMode::TwoD,
                "2D",
                "2D: author the scene on the canvas",
            ),
        ],
    )
}

/// One clickable segment inside the 3D|2D control.
fn viewport_mode_segment(
    host: Entity,
    mode: ViewportMode,
    label: &'static str,
    tooltip: &'static str,
) -> impl Bundle {
    (
        ViewportModeSegment { host, mode },
        Interaction::default(),
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            justify_content: JustifyContent::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            ..default()
        },
        BackgroundColor(Color::NONE),
        observe(
            move |click: On<Pointer<Click>>,
                  disabled: Query<(), With<bevy::ui::InteractionDisabled>>,
                  mut hosts: Query<&mut ViewportHost>,
                  mut intent: ResMut<ViewportModeIntent>| {
                if disabled.contains(click.event_target()) {
                    return;
                }
                let Ok(mut panel) = hosts.get_mut(host) else {
                    return;
                };
                *intent = ViewportModeIntent { mode, chosen: true };
                if panel.mode != mode || !panel.mode_chosen {
                    panel.mode = mode;
                    panel.mode_chosen = true;
                }
            },
        ),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::TEXT_SIZE_SM,
                ..default()
            },
            TextColor(tokens::TEXT_SECONDARY),
        )],
    )
}

/// Highlight the segment matching each panel's mode, in both of its bars.
fn update_viewport_mode_bar(
    hosts: Query<&ViewportHost>,
    mut segments: Query<(&ViewportModeSegment, &mut BackgroundColor)>,
) {
    for (segment, mut background) in &mut segments {
        let Ok(host) = hosts.get(segment.host) else {
            continue;
        };
        let color = if host.mode == segment.mode {
            tokens::TOOLBAR_ACTIVE_BG
        } else {
            Color::NONE
        };
        if background.0 != color {
            background.0 = color;
        }
    }
}

/// Register the operators this module owns.
pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportModeOp>();
}

/// Switch the viewport between the 3D world and the 2D canvas.
///
/// Every open panel, like `viewport2d.mode` and `viewport2d.frame`: an operator
/// call names no panel, and a scripted run that moved one the user could not
/// identify would be worse than one that moved them all. The bar's own segments
/// are per panel, because that gesture does name its panel.
#[operator(
    id = "viewport.mode",
    label = "Set Viewport Mode",
    description = "Switch the viewport between the 3D world and the 2D canvas.",
    allows_undo = false,
    params(mode(String, doc = "`3d` for the world viewport, `2d` for the canvas."))
)]
pub(crate) fn viewport_mode(
    params: In<OperatorParameters>,
    mut hosts: Query<&mut ViewportHost>,
    mut intent: ResMut<ViewportModeIntent>,
) -> OperatorResult {
    let Some(mode) = params.as_str("mode").and_then(ViewportMode::parse) else {
        warn!("viewport.mode: 'mode' must be `3d` or `2d`");
        return OperatorResult::Cancelled;
    };
    if hosts.is_empty() {
        warn!("viewport.mode: no viewport panel is open");
        return OperatorResult::Cancelled;
    }
    *intent = ViewportModeIntent { mode, chosen: true };
    for mut host in &mut hosts {
        if host.mode != mode || !host.mode_chosen {
            host.mode = mode;
            host.mode_chosen = true;
        }
    }
    OperatorResult::Finished
}

pub struct ViewportHostPlugin;

impl Plugin for ViewportHostPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<ViewportModeIntent>().add_systems(
            Update,
            (
                // Chained, and ahead of the hover pass: a held request becomes
                // a mode, the mode reaches the columns and the cameras, and
                // only then is a panel hover-tested as what it now shows.
                (apply_pending_viewport_focus, apply_viewport_mode)
                    .chain()
                    .before(crate::EditorInteractionSystems),
                update_viewport_mode_bar,
            ),
        );
    }
}

#[cfg(test)]
mod viewport_mode_tests {
    use super::*;

    #[test]
    fn a_ui_scene_is_authored_on_the_canvas() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::Ui),
            ViewportMode::TwoD
        );
    }

    #[test]
    fn a_2d_world_scene_is_authored_on_the_canvas() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::TwoD),
            ViewportMode::TwoD
        );
    }

    #[test]
    fn a_3d_scene_is_authored_in_the_world_viewport() {
        assert_eq!(
            ViewportMode::for_scene_kind(SceneKind::ThreeD),
            ViewportMode::ThreeD
        );
    }

    #[test]
    fn a_mode_parameter_names_either_mode() {
        assert_eq!(ViewportMode::parse("3d"), Some(ViewportMode::ThreeD));
        assert_eq!(ViewportMode::parse(" 2D "), Some(ViewportMode::TwoD));
        assert_eq!(ViewportMode::parse("canvas"), None);
    }
}
