//! The viewport panel's mode: whether it shows the 3D world or the 2D canvas.
//!
//! One panel authors both, so the mode is the panel's own state. The
//! presentations themselves live in [`crate::viewport`] and
//! [`crate::viewport_2d`]; both are built for every panel and the mode decides
//! which column is displayed and which camera renders.

use bevy::{
    prelude::*,
    ui::Checked,
    ui_widgets::{ValueChange, observe},
};
use jackdaw_feathers::segmented;

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
    /// neither. Case and surrounding space are ignored.
    pub fn parse(value: &str) -> Option<Self> {
        match value.trim().to_ascii_lowercase().as_str() {
            "3d" => Some(Self::ThreeD),
            "2d" => Some(Self::TwoD),
            _ => None,
        }
    }
}

/// The mode the active scene tab wants its viewport panels in. Panel-independent
/// on purpose: two panels open at once share one mode.
#[derive(Resource, Default, Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportModeIntent {
    pub mode: ViewportMode,
    /// Whether the user picked this mode, as opposed to it following from the
    /// scene's kind. A chosen mode is stored in the tab's view state and
    /// restored on the next activation; an unchosen one is recomputed.
    pub chosen: bool,
}

/// A mode request made while the dock had no viewport leaf to honour it on,
/// held here until the first frame a leaf exists.
#[derive(Resource, Debug, Clone, Copy, PartialEq, Eq)]
pub struct PendingViewportFocus(pub ViewportMode);

/// A viewport panel, on its dock-leaf content entity: which mode it is in and
/// where its two presentation subtrees are. `ViewportPanelHost` and
/// `Viewport2dPanelHost` sit beside it holding each presentation's own state.
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

/// The panel that answers for the editor's one 2D canvas: the first showing the
/// canvas, else the first panel of any mode.
pub fn primary_2d_host<'a>(
    hosts: impl Iterator<Item = (Entity, &'a ViewportHost)>,
) -> Option<Entity> {
    first_host_in(hosts, ViewportMode::TwoD)
}

/// The counterpart of [`primary_2d_host`] for what belongs to the world: the
/// first panel showing it, else the first panel of any mode.
pub fn primary_3d_host<'a>(
    hosts: impl Iterator<Item = (Entity, &'a ViewportHost)>,
) -> Option<Entity> {
    first_host_in(hosts, ViewportMode::ThreeD)
}

/// The first panel in `mode`, or the first panel of any mode when none is in
/// it, so a scene opened while every panel is elsewhere still lays out against
/// a real target.
fn first_host_in<'a>(
    hosts: impl Iterator<Item = (Entity, &'a ViewportHost)>,
    mode: ViewportMode,
) -> Option<Entity> {
    let mut first = None;
    for (entity, host) in hosts {
        if host.mode == mode {
            return Some(entity);
        }
        first.get_or_insert(entity);
    }
    first
}

/// Build a viewport panel in the mode `intent` asks for. Both presentations are
/// built whatever the mode is, so a switch is not a panel rebuild that would
/// drop the camera pose and the canvas framing.
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
    // its first frame showing both columns would stack them.
    if let Err(err) = world.run_system_cached(apply_viewport_mode) {
        error!("failed to apply the viewport mode to a new panel: {err}");
    }
}

/// Show the column the mode names, hide the other, and let only the shown one's
/// camera render. Nothing is despawned, so the chrome, the camera pose and the
/// canvas framing outlive a switch. An inactive camera still gets its target's
/// size, so a UI root on the hidden 2D camera stays laid out at its authored
/// reference resolution.
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

/// Put every open panel, and the tab's intent, in `mode`. `chosen` says whether
/// the user asked or the scene's kind implied it; only a chosen mode is stored
/// in the tab's view state. The dock is untouched, so this costs no reconcile;
/// use [`focus_viewport`] when the panel also has to come forward.
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

/// Put the viewport in `mode` and bring its tab forward. Best effort on the
/// dock: with no leaf to front the request is held in [`PendingViewportFocus`],
/// and no panel is added back to a workspace the user arranged without one.
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
    // re-runs the reconciler over the whole tree.
    if leaf.active != Some(tab) {
        tree.set_active(leaf_id, tab);
    }
    true
}

/// Honour a held request once the dock has a viewport leaf to honour it on, and
/// only then drop it. The mode is written again here, since the panels that
/// answer it were built after the request was held.
fn apply_pending_viewport_focus(world: &mut World) {
    let Some(pending) = world.get_resource::<PendingViewportFocus>().copied() else {
        return;
    };
    if front_viewport_tab(world) {
        set_viewport_mode(world, pending.0, false);
        world.remove_resource::<PendingViewportFocus>();
    }
}

/// Marker on one segment of the 3D|2D control, naming the panel it switches so
/// a segment in one panel's bar never moves another panel's.
#[derive(Component, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ViewportModeSegment {
    pub host: Entity,
    pub mode: ViewportMode,
}

/// The two-segment 3D|2D control. One is spawned into each presentation's bar,
/// so whichever bar the mode is showing carries the way back out of it.
pub(crate) fn viewport_mode_bar(host: Entity) -> impl Bundle {
    (
        segmented::segmented_bar(),
        observe(
            move |change: On<ValueChange<Entity>>,
                  segments: Query<&ViewportModeSegment>,
                  mut hosts: Query<&mut ViewportHost>,
                  mut intent: ResMut<ViewportModeIntent>| {
                let Ok(segment) = segments.get(change.value) else {
                    return;
                };
                let Ok(mut panel) = hosts.get_mut(host) else {
                    return;
                };
                let mode = segment.mode;
                *intent = ViewportModeIntent { mode, chosen: true };
                if panel.mode != mode || !panel.mode_chosen {
                    panel.mode = mode;
                    panel.mode_chosen = true;
                }
            },
        ),
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
        jackdaw_feathers::tooltip::Tooltip::title(tooltip),
        segmented::segment(label),
    )
}

/// Highlight the segment matching each panel's mode, in both of its bars.
fn update_viewport_mode_bar(
    hosts: Query<&ViewportHost>,
    mut segments: Query<(
        Entity,
        &ViewportModeSegment,
        &mut BackgroundColor,
        Has<Checked>,
    )>,
    mut commands: Commands,
) {
    for (entity, segment, mut background, checked) in &mut segments {
        let Ok(host) = hosts.get(segment.host) else {
            continue;
        };
        let active = host.mode == segment.mode;
        let color = segmented::segment_background(active);
        if background.0 != color {
            background.0 = color;
        }
        if checked != active {
            segmented::set_segment_checked(&mut commands, entity, active);
        }
    }
}

/// Register the operators this module owns.
pub(crate) fn add_to_extension(ctx: &mut ExtensionContext) {
    ctx.register_operator::<ViewportModeOp>();
}

/// Switch the viewport between the 3D world and the 2D canvas. Moves every open
/// panel, since an operator call names none. An empty dock is not a refusal: the
/// mode is recorded in [`ViewportModeIntent`] and the next panel opens in it.
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
    // Recorded even with nothing open to move: the intent is what a panel built
    // afterwards opens in.
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
                // Ahead of the hover pass, so a panel is hover-tested as what
                // it now shows.
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
