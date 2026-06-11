//! Live Changes tray: a badge in the hierarchy header counts the tracked
//! live edits, and clicking it opens a floating panel listing each entry
//! with per-entry Save / Revert plus Apply All / Discard All. Buttons only
//! dispatch the `pie.live_edit_*` operators; the tray itself is plain view
//! state, like a context menu.
//!
//! Stopping play with tracked edits raises a modal reconciliation prompt
//! (Apply All / Discard / Review) so the entries are never silently left
//! behind; Review hands off to the tray for per-entry triage.

use bevy::picking::pointer::PointerButton;
use bevy::prelude::*;
use bevy::ui::ui_transform::UiGlobalTransform;
use bevy::ui_widgets::observe;
use jackdaw_api::prelude::*;
use jackdaw_feathers::tokens;
use jackdaw_jsn::SceneJsnAst;

use crate::{
    AppState,
    live_edits::{
        LiveEditAction, LiveEditKey, LiveEditLog, PieLiveEditRevertOp, PieLiveEditSaveOp,
        PieLiveEditsApplyAllOp, PieLiveEditsDiscardAllOp, truncate_json_for_display,
        truncate_json_to,
    },
    pie_mirror::PieViewMode,
    pie_projection::PieProjection,
};

/// Fixed tray width; the panel right-aligns under the badge.
const TRAY_WIDTH: f32 = 460.0;
/// Per-side character cap for the `baseline -> live` cell.
const TRAY_VALUE_CAP: usize = 24;
/// Fixed width of the stop reconciliation prompt panel.
const STOP_PROMPT_WIDTH: f32 = 520.0;
/// Backdrop dim behind the stop prompt panel, matching the editor dialogs.
const STOP_PROMPT_BACKDROP_OPACITY: f32 = 0.8;

/// Whether the Live Changes tray is open. Plain view state, toggled by the
/// badge; forced closed when the log empties or the view leaves Live.
#[derive(Resource, Default)]
pub struct TrayOpen(pub bool);

/// Whether the stop reconciliation prompt is open. Raised when play stops
/// with tracked edits still in the log; the prompt's buttons decide whether
/// the entries get applied, discarded, or reviewed in the tray. While it is
/// open, the log is never cleared out from under it (focus changes skip
/// their usual clear).
#[derive(Resource, Default)]
pub struct StopPrompt(pub bool);

/// Marker on the badge chip in the hierarchy header.
#[derive(Component)]
pub struct LiveEditsBadge;

/// Marker on the count label inside the badge.
#[derive(Component)]
struct LiveEditsBadgeLabel;

/// Marker on the tray panel root.
#[derive(Component)]
struct LiveEditsTray;

/// Marker on the stop prompt overlay root (the dimmed backdrop).
#[derive(Component)]
struct StopPromptOverlay;

/// Per-row presentation derived from whether the entry's entity still
/// resolves. Stale rows dim and disable both actions; the entry becomes
/// actionable again when its scene tab (or live entity) returns.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RowState {
    stale: bool,
    save_enabled: bool,
    revert_enabled: bool,
}

/// Classify one tray row. `resolves` is whether
/// [`crate::live_edits::resolve_entry_entity`] finds an entity for the
/// entry.
fn tray_row_state(resolves: bool) -> RowState {
    RowState {
        stale: !resolves,
        save_enabled: resolves,
        revert_enabled: resolves,
    }
}

/// Build the badge chip for the hierarchy header. Hidden until the log has
/// entries and the view is Live (or play has stopped);
/// `update_live_edits_badge` flips the display and keeps the count label
/// current. Clicking toggles the tray.
pub fn live_edits_badge() -> impl Bundle {
    (
        LiveEditsBadge,
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(2.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            display: Display::None,
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(|click: On<Pointer<Click>>, mut open: ResMut<TrayOpen>| {
            if click.event().button != PointerButton::Primary {
                return;
            }
            open.0 = !open.0;
        }),
        children![(
            LiveEditsBadgeLabel,
            Text::new(String::new()),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(crate::default_style::LIVE_EDIT_ACCENT),
        )],
    )
}

/// The badge's count label text.
fn badge_label(count: usize) -> String {
    if count == 1 {
        "1 edit".to_string()
    } else {
        format!("{count} edits")
    }
}

/// Keep the badge's count label and visibility in sync with the log and
/// the view mode, and force the tray closed when the badge hides. Runs
/// every frame with guarded writes so a badge respawned by a layout
/// rebuild picks up the current state immediately.
fn update_live_edits_badge(
    mode: Res<PieViewMode>,
    play: Res<State<jackdaw_api::pie::PlayState>>,
    log: Res<LiveEditLog>,
    mut open: ResMut<TrayOpen>,
    mut badges: Query<(&mut Node, &Children), With<LiveEditsBadge>>,
    mut labels: Query<&mut Text, With<LiveEditsBadgeLabel>>,
) {
    let count = log.entries.len();
    // Entries survive a stop (they still apply through their authored node
    // ids), so the badge also shows after play ends; the stop prompt's
    // Review button relies on the tray being reachable then.
    let stopped = *play.get() == jackdaw_api::pie::PlayState::Stopped;
    let visible = count > 0 && (*mode == PieViewMode::Live || stopped);
    if !visible && open.0 {
        open.0 = false;
    }
    let label = badge_label(count);
    let display = if visible {
        Display::Flex
    } else {
        Display::None
    };
    for (mut node, children) in &mut badges {
        if node.display != display {
            node.display = display;
        }
        for child in children.iter() {
            if let Ok(mut text) = labels.get_mut(child)
                && text.0 != label
            {
                text.0 = label.clone();
            }
        }
    }
}

/// Rebuild the tray panel whenever it opens, closes, or the log changes.
/// The list is small, so a full despawn-and-respawn keeps it correct
/// without any per-row diffing.
fn rebuild_tray(
    mut commands: Commands,
    open: Res<TrayOpen>,
    log: Res<LiveEditLog>,
    ast: Res<SceneJsnAst>,
    projection: Res<PieProjection>,
    badges: Query<(&ComputedNode, &UiGlobalTransform), With<LiveEditsBadge>>,
    trays: Query<Entity, With<LiveEditsTray>>,
) {
    if !open.is_changed() && !log.is_changed() {
        return;
    }
    for tray in &trays {
        commands.entity(tray).despawn();
    }
    if !open.0 || log.is_empty() {
        return;
    }
    let Ok((computed, global_tf)) = badges.single() else {
        return;
    };
    let (_, _, pos) = global_tf.to_scale_angle_translation();
    let size = computed.size() * computed.inverse_scale_factor();
    let right = pos.x + size.x / 2.0;
    let top = pos.y + size.y / 2.0 + 4.0;

    let tray = commands
        .spawn((
            LiveEditsTray,
            crate::EditorEntity,
            Node {
                position_type: PositionType::Absolute,
                top: px(top),
                left: px(right - TRAY_WIDTH),
                width: px(TRAY_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(tokens::SPACING_SM)),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                row_gap: px(tokens::SPACING_XS),
                ..Default::default()
            },
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
            ZIndex(1000),
        ))
        .id();

    for (key, entry) in &log.entries {
        let resolves =
            crate::live_edits::resolve_entry_entity(&ast, &projection, key, entry).is_some();
        let state = tray_row_state(resolves);
        spawn_tray_row(&mut commands, tray, key, entry, state);
    }

    spawn_tray_footer(&mut commands, tray);
}

/// Spawn one entry row: label, `baseline -> live`, Save, Revert. Shared by
/// the tray and the stop prompt's entry list.
fn spawn_tray_row(
    commands: &mut Commands,
    parent: Entity,
    key: &LiveEditKey,
    entry: &crate::live_edits::LiveEditEntry,
    state: RowState,
) {
    let text_color = if state.stale {
        tokens::TEXT_DISABLED
    } else {
        tokens::TEXT_PRIMARY
    };
    let baseline = truncate_json_to(entry.baseline.as_ref(), TRAY_VALUE_CAP);
    let live = truncate_json_to(Some(&entry.live_value), TRAY_VALUE_CAP);
    let tooltip_baseline = truncate_json_for_display(entry.baseline.as_ref());
    let tooltip_live = truncate_json_for_display(Some(&entry.live_value));

    commands.spawn((
        ChildOf(parent),
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(tokens::SPACING_SM),
            padding: UiRect::axes(px(tokens::SPACING_XS), px(2.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            ..Default::default()
        },
        jackdaw_feathers::tooltip::Tooltip::title(format!("scene: {tooltip_baseline}"))
            .with_description(format!("live: {tooltip_live}")),
        children![
            (
                Node {
                    flex_grow: 1.0,
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                children![(
                    Text::new(entry.label.clone()),
                    TextFont {
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(text_color),
                )],
            ),
            (
                Node {
                    overflow: Overflow::clip(),
                    ..Default::default()
                },
                children![(
                    Text::new(format!("{baseline} -> {live}")),
                    TextFont {
                        font_size: tokens::FONT_SM,
                        ..Default::default()
                    },
                    TextColor(if state.stale {
                        tokens::TEXT_DISABLED
                    } else {
                        tokens::TEXT_SECONDARY
                    }),
                )],
            ),
            tray_entry_button(
                "Save",
                key.clone(),
                LiveEditAction::Save,
                state.save_enabled
            ),
            tray_entry_button(
                "Revert",
                key.clone(),
                LiveEditAction::Revert,
                state.revert_enabled,
            ),
        ],
    ));
}

/// Spawn the footer row with the Apply All / Discard All buttons.
fn spawn_tray_footer(commands: &mut Commands, tray: Entity) {
    commands.spawn((
        ChildOf(tray),
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            column_gap: px(tokens::SPACING_SM),
            padding: UiRect::axes(px(tokens::SPACING_XS), px(tokens::SPACING_XS)),
            ..Default::default()
        },
        children![
            tray_footer_button("Apply All", PieLiveEditsApplyAllOp::ID),
            tray_footer_button("Discard All", PieLiveEditsDiscardAllOp::ID),
        ],
    ));
}

/// A small text button that stashes the entry key on
/// [`LiveEditLog::pending_action`] and dispatches the matching operator,
/// the same channel the inspector's live-edit dot menu uses. Disabled
/// buttons keep the chip but render dimmed and ignore clicks.
fn tray_entry_button(
    label: &'static str,
    key: LiveEditKey,
    action: LiveEditAction,
    enabled: bool,
) -> impl Bundle {
    (
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(1.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(
            move |click: On<Pointer<Click>>,
                  mut commands: Commands,
                  mut log: ResMut<LiveEditLog>| {
                if !enabled || click.event().button != PointerButton::Primary {
                    return;
                }
                log.pending_action = Some((action, key.clone()));
                let operator_id = match action {
                    LiveEditAction::Save => PieLiveEditSaveOp::ID,
                    LiveEditAction::Revert => PieLiveEditRevertOp::ID,
                };
                commands
                    .operator(operator_id)
                    .settings(CallOperatorSettings {
                        execution_context: ExecutionContext::Invoke,
                        creates_history_entry: false,
                    })
                    .call();
            },
        ),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(if enabled {
                tokens::TEXT_PRIMARY
            } else {
                tokens::TEXT_DISABLED
            }),
        )],
    )
}

/// A footer button that dispatches `operator_id` directly. The bulk
/// operators read the log themselves, so no pending key is set.
fn tray_footer_button(label: &'static str, operator_id: &'static str) -> impl Bundle {
    (
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(1.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(move |click: On<Pointer<Click>>, mut commands: Commands| {
            if click.event().button != PointerButton::Primary {
                return;
            }
            commands
                .operator(operator_id)
                .settings(CallOperatorSettings {
                    execution_context: ExecutionContext::Invoke,
                    creates_history_entry: false,
                })
                .call();
        }),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    )
}

/// Raise the stop prompt when play stops with tracked edits still in the
/// log. Runs on the stop transition, which covers both the explicit stop
/// operator and the crash/self-exit reap. The log is left intact; the
/// prompt's buttons decide its fate.
pub fn open_stop_prompt_if_dirty(log: Res<LiveEditLog>, mut prompt: ResMut<StopPrompt>) {
    if !log.is_empty() {
        prompt.0 = true;
    }
}

/// Close the prompt and hand its entries to the tray for manual triage.
pub(crate) fn review_handoff(prompt: &mut StopPrompt, open: &mut TrayOpen) {
    prompt.0 = false;
    open.0 = true;
}

/// Hand the prompt off to the tray when play restarts while it is still
/// open. A new session would otherwise stream behind the dimmed backdrop
/// and mix its edits into the prompt list, so re-play acts as an implicit
/// Review: the prompt closes and the tray opens with the entries intact.
pub fn review_handoff_on_replay(mut prompt: ResMut<StopPrompt>, mut open: ResMut<TrayOpen>) {
    if prompt.0 {
        review_handoff(&mut prompt, &mut open);
    }
}

/// Run condition: the stop prompt is closed (or not yet initialized).
pub fn stop_prompt_closed(prompt: Option<Res<StopPrompt>>) -> bool {
    prompt.is_none_or(|p| !p.0)
}

/// The prompt's title line.
fn stop_prompt_title(count: usize) -> String {
    if count == 1 {
        "1 live edit not saved to scene".to_string()
    } else {
        format!("{count} live edits not saved to scene")
    }
}

/// Rebuild the stop prompt overlay whenever it opens, closes, or the log
/// changes (per-entry Save / Revert in the list mutates the log, so the
/// rows refresh in place). Closes itself when per-entry triage empties the
/// log.
fn rebuild_stop_prompt(
    mut commands: Commands,
    mut prompt: ResMut<StopPrompt>,
    log: Res<LiveEditLog>,
    ast: Res<SceneJsnAst>,
    projection: Res<PieProjection>,
    overlays: Query<Entity, With<StopPromptOverlay>>,
) {
    if prompt.0 && log.is_empty() {
        prompt.0 = false;
    }
    if !prompt.is_changed() && !log.is_changed() {
        return;
    }
    for overlay in &overlays {
        commands.entity(overlay).despawn();
    }
    if !prompt.0 {
        return;
    }

    // Full-window dimmed backdrop with the panel centered inside, the same
    // shape as the editor dialogs, layered above the tray.
    let backdrop = commands
        .spawn((
            StopPromptOverlay,
            crate::EditorEntity,
            Interaction::default(),
            Node {
                width: percent(100),
                height: percent(100),
                position_type: PositionType::Absolute,
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..Default::default()
            },
            BackgroundColor(Color::BLACK.with_alpha(STOP_PROMPT_BACKDROP_OPACITY)),
            // Above the feathers dialogs (200) so the modal sits on top.
            GlobalZIndex(210),
        ))
        .id();

    let panel = commands
        .spawn((
            ChildOf(backdrop),
            Node {
                width: px(STOP_PROMPT_WIDTH),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(px(tokens::SPACING_SM)),
                border: UiRect::all(px(1.0)),
                border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_MD)),
                row_gap: px(tokens::SPACING_XS),
                ..Default::default()
            },
            BackgroundColor(tokens::MENU_BG),
            BorderColor::all(tokens::BORDER_SUBTLE),
        ))
        .id();

    commands.spawn((
        ChildOf(panel),
        Node {
            padding: UiRect::axes(px(tokens::SPACING_XS), px(tokens::SPACING_XS)),
            ..Default::default()
        },
        children![(
            Text::new(stop_prompt_title(log.entries.len())),
            TextFont {
                font_size: tokens::FONT_MD,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    ));

    let mut stale_count = 0usize;
    for (key, entry) in &log.entries {
        let resolves =
            crate::live_edits::resolve_entry_entity(&ast, &projection, key, entry).is_some();
        let state = tray_row_state(resolves);
        if state.stale {
            stale_count += 1;
        }
        spawn_tray_row(&mut commands, panel, key, entry, state);
    }

    if stale_count > 0 {
        commands.spawn((
            ChildOf(panel),
            Node {
                padding: UiRect::axes(px(tokens::SPACING_XS), px(2.0)),
                ..Default::default()
            },
            children![(
                Text::new(format!(
                    "{stale_count} entries belong to another scene tab and apply when it is active"
                )),
                TextFont {
                    font_size: tokens::FONT_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
            )],
        ));
    }

    commands.spawn((
        ChildOf(panel),
        Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::FlexEnd,
            column_gap: px(tokens::SPACING_SM),
            padding: UiRect::axes(px(tokens::SPACING_XS), px(tokens::SPACING_XS)),
            ..Default::default()
        },
        children![
            stop_prompt_review_button(),
            stop_prompt_operator_button("Discard", PieLiveEditsDiscardAllOp::ID),
            stop_prompt_operator_button("Apply All", PieLiveEditsApplyAllOp::ID),
        ],
    ));
}

/// A prompt footer button that closes the prompt and dispatches
/// `operator_id`, the same channel as the tray's footer buttons.
fn stop_prompt_operator_button(label: &'static str, operator_id: &'static str) -> impl Bundle {
    (
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(1.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            flex_shrink: 0.0,
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(
            move |click: On<Pointer<Click>>,
                  mut commands: Commands,
                  mut prompt: ResMut<StopPrompt>| {
                if click.event().button != PointerButton::Primary {
                    return;
                }
                prompt.0 = false;
                commands
                    .operator(operator_id)
                    .settings(CallOperatorSettings {
                        execution_context: ExecutionContext::Invoke,
                        creates_history_entry: false,
                    })
                    .call();
            },
        ),
        children![(
            Text::new(label),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    )
}

/// The prompt's Review button: closes the prompt and opens the tray for
/// per-entry triage, leaving the log intact. Plain view-state toggles, the
/// same as the badge click.
fn stop_prompt_review_button() -> impl Bundle {
    (
        Interaction::default(),
        Node {
            align_items: AlignItems::Center,
            padding: UiRect::axes(px(tokens::SPACING_SM), px(1.0)),
            border: UiRect::all(px(1.0)),
            border_radius: BorderRadius::all(px(tokens::BORDER_RADIUS_SM)),
            flex_shrink: 0.0,
            margin: UiRect {
                right: Val::Auto,
                ..Default::default()
            },
            ..Default::default()
        },
        BackgroundColor(tokens::ELEVATED_BG),
        BorderColor::all(tokens::BORDER_SUBTLE),
        observe(
            move |click: On<Pointer<Click>>,
                  mut prompt: ResMut<StopPrompt>,
                  mut open: ResMut<TrayOpen>| {
                if click.event().button != PointerButton::Primary {
                    return;
                }
                review_handoff(&mut prompt, &mut open);
            },
        ),
        children![(
            Text::new("Review"),
            TextFont {
                font_size: tokens::FONT_SM,
                ..Default::default()
            },
            TextColor(tokens::TEXT_PRIMARY),
        )],
    )
}

/// Registers the tray view state and the badge / tray / stop prompt sync
/// systems. Chained so a badge-forced close rebuilds the tray the same
/// frame.
pub struct LiveEditsUiPlugin;

impl Plugin for LiveEditsUiPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<TrayOpen>()
            .init_resource::<StopPrompt>()
            .add_systems(
                Update,
                (update_live_edits_badge, rebuild_tray, rebuild_stop_prompt)
                    .chain()
                    .run_if(in_state(AppState::Editor)),
            );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn resolving_rows_enable_both_actions() {
        let state = tray_row_state(true);
        assert!(!state.stale);
        assert!(state.save_enabled);
        assert!(state.revert_enabled);
    }

    #[test]
    fn stale_rows_disable_both_actions() {
        let state = tray_row_state(false);
        assert!(state.stale);
        assert!(!state.save_enabled);
        assert!(!state.revert_enabled);
    }

    #[test]
    fn badge_label_pluralizes() {
        assert_eq!(badge_label(1), "1 edit");
        assert_eq!(badge_label(3), "3 edits");
    }

    #[test]
    fn stop_prompt_title_pluralizes() {
        assert_eq!(stop_prompt_title(1), "1 live edit not saved to scene");
        assert_eq!(stop_prompt_title(3), "3 live edits not saved to scene");
    }

    fn log_with_one_entry() -> LiveEditLog {
        let mut log = LiveEditLog::default();
        log.entries.push((
            LiveEditKey {
                bits: 7,
                type_path: "game::Health".to_string(),
                field_path: "current".to_string(),
            },
            crate::live_edits::LiveEditEntry {
                node_id: None,
                baseline: None,
                live_value: serde_json::json!(50.0),
                label: "player / Health.current".to_string(),
            },
        ));
        log
    }

    #[test]
    fn stop_with_edits_opens_the_prompt_and_keeps_the_log() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(log_with_one_entry());
        world.init_resource::<StopPrompt>();

        world
            .run_system_once(open_stop_prompt_if_dirty)
            .expect("system runs");

        assert!(world.resource::<StopPrompt>().0, "the prompt opened");
        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "the stop hook leaves the log intact"
        );
    }

    #[test]
    fn stop_with_clean_log_stays_quiet() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.init_resource::<LiveEditLog>();
        world.init_resource::<StopPrompt>();

        world
            .run_system_once(open_stop_prompt_if_dirty)
            .expect("system runs");

        assert!(!world.resource::<StopPrompt>().0);
    }

    #[test]
    fn review_handoff_closes_prompt_and_opens_tray() {
        let mut prompt = StopPrompt(true);
        let mut open = TrayOpen(false);

        review_handoff(&mut prompt, &mut open);

        assert!(!prompt.0, "the prompt closed");
        assert!(open.0, "the tray opened");
    }

    #[test]
    fn replay_while_prompt_open_hands_off_to_tray() {
        use bevy::ecs::system::RunSystemOnce;

        let mut world = World::new();
        world.insert_resource(log_with_one_entry());
        world.insert_resource(StopPrompt(true));
        world.init_resource::<TrayOpen>();

        world
            .run_system_once(review_handoff_on_replay)
            .expect("system runs");

        assert!(!world.resource::<StopPrompt>().0, "the prompt closed");
        assert!(world.resource::<TrayOpen>().0, "the tray opened");
        assert_eq!(
            world.resource::<LiveEditLog>().entries.len(),
            1,
            "the handoff leaves the log intact"
        );
    }
}
