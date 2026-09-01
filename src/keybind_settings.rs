//! The keybind settings dialog.
//!
//! Every editor command is an operator, and an operator's chord lives in
//! the keymap, so the dialog is a view of the keymap: one row per
//! operator, showing the chords the working copy gives it, with a rebind
//! and a reset. Saving diffs the working copy against what the editor
//! ships with, writes the difference to the user keymap file, and
//! re-applies the result, so a rebind takes effect in the session that
//! made it rather than at the next launch.
//!
//! A command can hold more than one chord, and several ship that way, so
//! an editable row draws each chord with a remove of its own and offers
//! Add Chord beside Rebind. Rebind replaces every chord; both keep the
//! phase and context the command's rows already had, because when a
//! command fires is not something changing its chord should decide.
//!
//! Three kinds of row are not editable here. A *fixed* row is an
//! operator whose chord is attached at a raw binding site rather than
//! through the keymap (hold-repeat nudges, the draw-brush modal's own
//! keys); the dialog shows those so the chord is not invisible, and
//! cannot change them. A *menu only* row is an operator with no input
//! action behind it at all: it was registered to be reached from a menu,
//! a button or the command palette, and there is nothing for a chord to
//! attach to, so the dialog says so rather than offering a rebind that
//! would go nowhere. Both carry the reason in a tooltip, because the
//! heading alone says what the row is and not why. The camera rows are
//! the last users of the legacy [`KeybindRegistry`], which drives camera
//! fly directly; they keep their old behaviour, including their own file.
//!
//! A chord more than one command claims is marked on the rows it is
//! about, naming the other commands the way those rows name them. The
//! line above the list is for what the reader has to decide something
//! about: a conflict a rebind in this session made, a keymap file that
//! would not parse, a saved binding that could not be attached. Chords
//! shared since the editor shipped are arbitrated by availability, so
//! they are one line saying how many, not six saying which.

use std::collections::HashMap;

use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy_enhanced_input::prelude::{Binding, Bindings};
use jackdaw_api_internal::keymap::{
    DefaultKeymap, KeymapCapture, KeymapLoadProblem, KeymapPreset, PresetBinding, PresetContext,
    PresetInput, PresetPhase, PresetSpawnedBinding, UserKeymap, find_conflicts, key_code_from_name,
    key_code_name, mouse_button_name, resolve_keymap, save_user_keymap,
};
use jackdaw_api_internal::lifecycle::{OperatorAction, OperatorChordSite, OperatorEntity};
use jackdaw_commands::keybinds::{EditorAction, Keybind, KeybindRegistry};
use jackdaw_feathers::icons::Icon;
use jackdaw_feathers::tooltip::Tooltip;
use jackdaw_feathers::{
    button::{
        ButtonClickEvent, ButtonContentText, ButtonProps, ButtonVariant, IconButtonProps, button,
        button_caption, icon_button,
    },
    dialog::{
        CloseDialogEvent, DialogActionEvent, DialogChildrenSlot, EditorDialog, OpenDialogEvent,
    },
    text_edit::{self, TextEditProps, TextEditValue},
    tokens,
};

use crate::extension_lifecycle::LastKeymapApply;
use crate::operator_tooltip::{format_binding, format_preset_input};

pub struct KeybindSettingsPlugin;

impl Plugin for KeybindSettingsPlugin {
    fn build(&self, app: &mut App) {
        app.init_resource::<KeybindRecordingState>()
            .init_resource::<KeyFilterState>()
            .add_observer(open_keybind_settings)
            .add_observer(on_keybind_settings_save)
            .add_observer(on_rebind_click)
            .add_observer(on_reset_click)
            .add_observer(on_reset_all_click)
            .add_observer(on_key_filter_click)
            .add_observer(on_remove_chord_click)
            .add_systems(
                Update,
                (
                    populate_keybind_dialog,
                    capture_keybind_recording,
                    capture_key_filter,
                    apply_keybind_filter,
                    refresh_advisory,
                    refresh_chord_lists,
                    refresh_conflict_badges,
                    cleanup_on_dialog_close,
                )
                    .run_if(in_state(crate::AppState::Editor)),
            );
    }
}

#[derive(Event)]
pub struct OpenKeybindSettingsEvent;

/// Operators that have an input action, hold no chord in the keymap, and
/// are meant to.
///
/// The other half of "unbound" is derived rather than listed: an operator
/// with no action entity has nothing for a binding to point at, so the
/// applier could not give it a chord however the keymap were written, and
/// it needs no entry here. What is left is the operators that could hold a
/// chord: their chord lives at a raw binding site the preset format cannot
/// yet express (hold-repeat nudges, modifier-only gestures, a modal's own
/// keys), or they are reached only from a surface of their own.
///
/// Listed rather than inferred, because the difference between "give it a
/// chord" and "it never had one" is a decision. `keymap_user_overrides`
/// fails on an operator that needs the decision and has not had it, and on
/// an entry that has gone stale in either direction.
pub const UNBOUND_OPERATORS: &[&str] = &[
    "clip.timeline.step_left",
    "clip.timeline.step_right",
    "command_palette.toggle",
    "draw_brush.confirm",
    "hierarchy.open_context_menu",
    "mesh.mirror.add",
    "mesh.mirror.apply",
    "mesh.mirror.bisect",
    "mesh.symmetrize",
    "mirror.plane.drag",
    "mirror.plane.set",
    "modifier.add",
    "modifier.apply",
    "modifier.move_down",
    "modifier.move_up",
    "modifier.remove",
    "modifier.toggle",
    "tools.measure_distance",
    "tools.measure_distance.confirm",
    "transform.nudge_x_neg",
    "transform.nudge_x_pos",
    "transform.nudge_y_neg",
    "transform.nudge_y_pos",
    "transform.nudge_z_neg",
    "transform.nudge_z_pos",
    "viewport.draw_brush.cancel_cut",
    "viewport.draw_brush_modal",
];

/// Heading for the rows whose chord is attached outside the keymap.
const FIXED: &str = "Fixed";

/// Heading for the rows with no input action to attach a chord to.
const MENU_ONLY: &str = "Menu only";

/// The camera-fly actions, the last commands still driven by the legacy
/// [`KeybindRegistry`] rather than by an operator.
const CAMERA_ACTIONS: &[EditorAction] = &[
    EditorAction::CameraForward,
    EditorAction::CameraBackward,
    EditorAction::CameraLeft,
    EditorAction::CameraRight,
    EditorAction::CameraUp,
    EditorAction::CameraDown,
];

/// One operator as the dialog lists it: what it is called, where it
/// belongs, and the chords it holds that the dialog cannot change.
#[derive(Clone, Debug)]
pub struct KeybindRow {
    pub operator: String,
    pub label: String,
    pub description: String,
    /// Where the dialog lists this row: the part of the operator id
    /// before the first dot, capitalised, or `"Fixed"` for a row whose
    /// chord the dialog cannot change.
    pub category: String,
    /// Chords bound at a raw site rather than through the keymap. An
    /// operator with any of these is a fixed row.
    pub fixed: Vec<String>,
    /// Whether the operator has an input action for a chord to attach
    /// to. False for the operators that are only ever reached from a
    /// menu, a button or the command palette.
    pub bindable: bool,
}

impl KeybindRow {
    /// Whether this row's chords can be changed here.
    pub fn is_editable(&self) -> bool {
        self.bindable && self.fixed.is_empty()
    }

    /// Why this row offers no rebind, for the reader who wants to know
    /// what "Fixed" or "Menu only" is standing in for.
    ///
    /// Empty for an editable row. The words are in the dialog rather than
    /// only in this module's own documentation, because the person asking
    /// is looking at the row.
    pub fn reason(&self) -> String {
        if self.is_editable() {
            return String::new();
        }
        if !self.bindable {
            return "This command has no input action to attach a chord to. It is reached from a \
                    menu, a button, or the command palette."
                .to_string();
        }
        format!(
            "This command's chords are attached in code rather than through the keymap, so they \
             cannot be changed here: {}",
            self.fixed.join(", ")
        )
    }
}

/// The dialog's working copy of the keymap.
#[derive(Resource, Clone, Debug, Default)]
pub struct PendingKeymapChanges {
    /// One row per operator, in the order the dialog lists them.
    pub rows: Vec<KeybindRow>,
    /// The working keymap: what Save turns into the user keymap.
    pub bindings: Vec<PresetBinding>,
    /// What the editor ships with, so a reset has something to go back to.
    pub defaults: Vec<PresetBinding>,
    /// User rows naming an operator this build does not have. Carried
    /// through Save untouched so a disabled extension's rebinds are not
    /// deleted by the session that could not resolve them.
    pub unresolved: Vec<PresetBinding>,
    /// Working copy of the legacy camera bindings.
    pub camera: HashMap<EditorAction, Vec<Keybind>>,
}

impl PendingKeymapChanges {
    /// The chords the working copy currently gives `operator`.
    pub fn chords_of(&self, operator: &str) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|binding| binding.operator == operator)
            .map(|binding| format_preset_input(&binding.input))
            .collect()
    }

    /// The phase and context `operator`'s rows are written in.
    ///
    /// Taken from the rows it already holds, so a command that fires on
    /// release keeps firing on release when its chord is changed. Writing
    /// `Press` for everything turned a release binding into a press one
    /// the first time it was rebound, with nothing saying so.
    fn shape_of(&self, operator: &str) -> (PresetPhase, PresetContext) {
        self.bindings
            .iter()
            .chain(self.defaults.iter())
            .find(|binding| binding.operator == operator)
            .map_or((PresetPhase::Press, PresetContext::Operators), |binding| {
                (binding.phase, binding.context)
            })
    }

    /// Bind `operator` to `input`, replacing every chord it had. The
    /// command whose chord this was keeps it: a shared chord is a thing
    /// the keymap allows, and taking one away from a command the user
    /// did not name is not this dialog's decision to make.
    pub fn rebind(&mut self, operator: &str, input: PresetInput) {
        let (phase, context) = self.shape_of(operator);
        self.bindings.retain(|binding| binding.operator != operator);
        self.bindings.push(PresetBinding {
            operator: operator.to_string(),
            input,
            phase,
            context,
        });
    }

    /// Give `operator` another chord alongside the ones it holds.
    ///
    /// A command can answer to more than one chord, and several ship that
    /// way - Delete and Backspace, the two zoom keys. A dialog that could
    /// only replace turned every one of those into a single chord the
    /// first time it was touched.
    pub fn add_chord(&mut self, operator: &str, input: PresetInput) {
        if self
            .bindings
            .iter()
            .any(|binding| binding.operator == operator && binding.input == input)
        {
            return;
        }
        let (phase, context) = self.shape_of(operator);
        self.bindings.push(PresetBinding {
            operator: operator.to_string(),
            input,
            phase,
            context,
        });
    }

    /// Drop `operator`'s chord at `index`, counting the way
    /// [`Self::chords_of`] lists them.
    pub fn remove_chord(&mut self, operator: &str, index: usize) {
        let Some(at) = self
            .bindings
            .iter()
            .enumerate()
            .filter(|(_, binding)| binding.operator == operator)
            .map(|(at, _)| at)
            .nth(index)
        else {
            return;
        };
        self.bindings.remove(at);
    }

    /// Put `operator` back on the chords it ships with.
    pub fn reset(&mut self, operator: &str) {
        self.bindings.retain(|binding| binding.operator != operator);
        self.bindings.extend(
            self.defaults
                .iter()
                .filter(|binding| binding.operator == operator)
                .cloned(),
        );
    }

    /// Put every operator back on the chords it ships with.
    pub fn reset_all(&mut self) {
        self.bindings = self.defaults.clone();
    }

    /// Which other commands already claim `input` on the same phase and
    /// context, by label, ignoring `operator` itself.
    pub fn also_bound_to(&self, operator: &str, input: &PresetInput) -> Vec<String> {
        let mut others: Vec<String> = self
            .bindings
            .iter()
            .filter(|binding| {
                binding.operator != operator
                    && binding.input == *input
                    && binding.phase == PresetPhase::Press
                    && binding.context == PresetContext::Operators
            })
            .map(|binding| self.label_of(&binding.operator))
            .collect();
        others.dedup();
        others
    }

    fn label_of(&self, operator: &str) -> String {
        self.rows
            .iter()
            .find(|row| row.operator == operator)
            .map_or_else(|| operator.to_string(), |row| row.label.clone())
    }

    /// The user keymap this working copy stands for: every operator
    /// whose rows differ from the ones it ships with, plus the rows this
    /// build could not resolve.
    pub fn to_user_keymap(&self) -> UserKeymap {
        let mut operators: Vec<&str> = self
            .bindings
            .iter()
            .chain(self.defaults.iter())
            .map(|binding| binding.operator.as_str())
            .collect();
        operators.sort_unstable();
        operators.dedup();

        let mut bindings = self.unresolved.clone();
        for operator in operators {
            let current: Vec<&PresetBinding> = self
                .bindings
                .iter()
                .filter(|binding| binding.operator == operator)
                .collect();
            let shipped: Vec<&PresetBinding> = self
                .defaults
                .iter()
                .filter(|binding| binding.operator == operator)
                .collect();
            if current != shipped {
                bindings.extend(current.into_iter().cloned());
            }
        }
        UserKeymap { bindings }
    }

    /// The chords more than one command claims.
    pub fn conflicts(&self) -> Vec<String> {
        find_conflicts(&KeymapPreset {
            name: "pending".into(),
            bindings: self.bindings.clone(),
        })
    }

    /// The chords more than one command claims in the shipped keymap.
    fn shipped_conflicts(&self) -> Vec<String> {
        find_conflicts(&KeymapPreset {
            name: "classic".into(),
            bindings: self.defaults.clone(),
        })
    }

    /// The chords more than one command claims that the shipped keymap
    /// did not: the ones this session's rebinds made.
    pub fn user_conflicts(&self) -> Vec<String> {
        let shipped: std::collections::HashSet<String> =
            self.shipped_conflicts().into_iter().collect();
        self.conflicts()
            .into_iter()
            .filter(|conflict| !shipped.contains(conflict))
            .collect()
    }

    /// How many shipped chords are claimed by more than one command.
    pub fn shipped_conflict_count(&self) -> usize {
        self.shipped_conflicts().len()
    }

    /// One line per chord of `operator` that another command also claims,
    /// naming that command the way the row next to it is named.
    ///
    /// The advisory used to be the only sign of a conflict, so finding
    /// which row it was about meant reading a list of operator ids at the
    /// top of a dialog listing labels. This is what the row itself says.
    pub fn conflicts_of(&self, operator: &str) -> Vec<String> {
        self.bindings
            .iter()
            .filter(|binding| binding.operator == operator)
            .filter_map(|binding| {
                let others = self.also_bound_to(operator, &binding.input);
                (!others.is_empty()).then(|| {
                    format!(
                        "{} - also {}",
                        format_preset_input(&binding.input),
                        others.join(", ")
                    )
                })
            })
            .collect()
    }
}

/// Tracks which operator or camera action is being re-recorded.
#[derive(Resource, Default)]
pub(crate) struct KeybindRecordingState {
    target: Option<RecordingTarget>,
    /// Set once a chord another command already claims has been pressed,
    /// so the second press of the same chord commits it.
    pending_confirm: Option<PresetInput>,
}

/// What the recorded chord does to the row it was started from.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum RecordingMode {
    /// Replace every chord the command holds.
    Replace,
    /// Keep them and add one more.
    Add,
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum RecordingTarget {
    Operator(String, RecordingMode),
    Camera(EditorAction, usize),
}

impl RecordingTarget {
    /// The operator this recording is for, if it is for one.
    fn operator(&self) -> Option<&str> {
        match self {
            Self::Operator(operator, _) => Some(operator),
            Self::Camera(..) => None,
        }
    }
}

impl KeybindRecordingState {
    /// Whether the dialog is waiting for the user to press the chord it
    /// is about to record. Every other keyboard claim stands down while
    /// it is, so the press reaches the recorder.
    pub(crate) fn is_recording(&self) -> bool {
        self.target.is_some()
    }
}

/// Inserted when the dialog is open, removed on close or save.
#[derive(Resource)]
struct KeybindSettingsOpen;

/// Marker for the text filter input.
#[derive(Component)]
struct KeybindFilterInput;

/// Marker on the key-capture filter button.
#[derive(Component)]
struct KeyFilterButton;

/// Resource tracking the captured key filter.
#[derive(Resource, Default)]
struct KeyFilterState {
    /// When true, next non-modifier keypress sets the filter key.
    capturing: bool,
    /// The currently active key filter, if any.
    active_key: Option<KeyCode>,
}

/// Marker on each row, storing what it is a row for.
#[derive(Component, Clone)]
struct KeybindRowTarget {
    operator: Option<String>,
    camera: Option<EditorAction>,
    category: String,
    /// Lower-cased name and chords, for the text filter.
    haystack: String,
}

/// Marker on category headers, storing category name for filtering.
#[derive(Component)]
struct KeybindCategoryHeader(String);

/// The text element showing an operator's chords. The camera rows still
/// draw one; an operator row draws a [`KeybindChordList`] instead, so each
/// chord can be removed on its own.
#[derive(Component)]
struct KeybindDisplayText(String);

/// The container holding one operator row's chords, rebuilt whenever the
/// working copy or the recording changes.
#[derive(Component)]
struct KeybindChordList(String);

/// Remove button for one chord of an operator: (operator, index into
/// `chords_of`).
#[derive(Component)]
struct KeybindRemoveChordButton(String, usize);

/// Add-another-chord button for an operator row.
#[derive(Component)]
struct KeybindAddChordButton(String);

/// The marker on a row whose chords another command also claims.
#[derive(Component)]
struct KeymapConflictBadge(String);

/// The text element showing a camera action's chords.
#[derive(Component)]
struct CameraDisplayText(EditorAction);

/// Rebind button for an operator row.
#[derive(Component)]
struct KeybindRebindButton(String);

/// Rebind button for a camera row: (action, binding index).
#[derive(Component)]
struct CameraRebindButton(EditorAction, usize);

/// Per-row reset to default button.
#[derive(Component)]
struct KeybindResetButton(String);

/// Per-row reset for a camera action.
#[derive(Component)]
struct CameraResetButton(EditorAction);

/// Reset All button marker.
#[derive(Component)]
struct KeybindResetAllButton;

/// The advisory line above the list.
#[derive(Component)]
struct KeymapAdvisoryText;

/// Flag to prevent double-populating.
#[derive(Component)]
struct KeybindDialogPopulated;

/// Capitalise the part of an operator id before its first dot, so
/// `brush.mesh.subdivide` groups under `Brush`.
fn category_of(operator_id: &str) -> String {
    let head = operator_id.split('.').next().unwrap_or(operator_id);
    let mut chars = head.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + &chars.as_str().replace('_', " "),
        None => head.to_string(),
    }
}

/// Read the dialog's rows out of the world: every registered operator,
/// with the chords attached to it outside the keymap.
fn collect_rows(world: &mut World) -> Vec<KeybindRow> {
    let mut fixed: HashMap<String, Vec<String>> = HashMap::new();
    let mut entries: Vec<(String, Vec<Entity>)> = world
        .query::<(&OperatorAction, &Bindings)>()
        .iter(world)
        .map(|(action, bindings)| (action.0.to_string(), bindings.iter().collect()))
        .collect();
    // Chords that reach an operator from an action that is not its own.
    // They are as pressable as the ones on its own action, so a dialog
    // that left them out would show a chord the user cannot find and
    // hide one they can press.
    entries.extend(
        world
            .query::<(&OperatorChordSite, &Bindings)>()
            .iter(world)
            .map(|(site, bindings)| (site.0.to_string(), bindings.iter().collect())),
    );
    for (operator, binding_entities) in entries {
        for binding_entity in binding_entities {
            if world.get::<PresetSpawnedBinding>(binding_entity).is_some() {
                continue;
            }
            let Some(binding) = world.get::<Binding>(binding_entity).copied() else {
                continue;
            };
            if let Some(label) = format_binding(binding) {
                fixed.entry(operator.clone()).or_default().push(label);
            }
        }
    }

    // An operator with no action entity has nothing for a binding to
    // point at, so the applier cannot give it a chord however the keymap
    // is written.
    let with_action: std::collections::HashSet<String> = world
        .query::<&OperatorAction>()
        .iter(world)
        .map(|action| action.0.to_string())
        .collect();
    for list in fixed.values_mut() {
        list.sort();
        list.dedup();
    }

    let mut rows: Vec<KeybindRow> = world
        .query::<&OperatorEntity>()
        .iter(world)
        .map(|op| {
            let raw = fixed.get(op.id()).cloned().unwrap_or_default();
            let bindable = with_action.contains(op.id());
            KeybindRow {
                operator: op.id().to_string(),
                label: op.label().to_string(),
                description: op.description().to_string(),
                // Grouping by where the row is listed rather than by what
                // the operator is called keeps the rows the dialog cannot
                // edit together; grouping by the id's head scatters them
                // through the list and repeats their header at every one.
                category: if !bindable {
                    MENU_ONLY.to_string()
                } else if raw.is_empty() {
                    category_of(op.id())
                } else {
                    FIXED.to_string()
                },
                fixed: raw,
                bindable,
            }
        })
        .collect();
    rows.sort_by(|a, b| {
        a.category
            .cmp(&b.category)
            .then_with(|| a.operator.cmp(&b.operator))
    });
    rows.dedup_by(|a, b| a.operator == b.operator);
    rows
}

/// Build the dialog's working copy from the world.
pub fn pending_from_world(world: &mut World) -> PendingKeymapChanges {
    let rows = collect_rows(world);
    let defaults = world
        .get_resource_or_init::<DefaultKeymap>()
        .to_classic_preset()
        .bindings;
    let user = world.get_resource_or_init::<UserKeymap>().clone();
    let resolved = resolve_keymap(
        &KeymapPreset {
            name: "classic".into(),
            bindings: defaults.clone(),
        },
        &user,
    );
    let known: std::collections::HashSet<&str> =
        rows.iter().map(|row| row.operator.as_str()).collect();
    let unresolved: Vec<PresetBinding> = user
        .bindings
        .iter()
        .filter(|binding| !known.contains(binding.operator.as_str()))
        .cloned()
        .collect();
    let camera = world
        .get_resource::<KeybindRegistry>()
        .map(|registry| registry.bindings.clone())
        .unwrap_or_default();
    PendingKeymapChanges {
        rows,
        bindings: resolved.bindings,
        defaults,
        unresolved,
        camera,
    }
}

fn open_keybind_settings(
    _event: On<OpenKeybindSettingsEvent>,
    mut commands: Commands,
    existing: Option<Res<KeybindSettingsOpen>>,
) {
    if existing.is_some() {
        return;
    }

    commands.queue(|world: &mut World| {
        let pending = pending_from_world(world);
        world.insert_resource(pending);
    });
    commands.insert_resource(KeybindSettingsOpen);

    let mut dialog_event = OpenDialogEvent::new("Keybinds", "Save")
        .with_max_width(px(760))
        .with_close_on_click_outside(false)
        .without_content_padding();
    dialog_event.close_on_esc = false;
    commands.trigger(dialog_event);
}

fn format_chords(chords: &[String]) -> String {
    if chords.is_empty() {
        return "Unbound".to_string();
    }
    chords.join(" / ")
}

fn format_bindings(bindings: &[Keybind]) -> String {
    if bindings.is_empty() {
        return "Unbound".to_string();
    }
    bindings
        .iter()
        .map(std::string::ToString::to_string)
        .collect::<Vec<_>>()
        .join(" / ")
}

/// What the advisory line says about a working copy and the last apply.
///
/// Only what the reader has to decide something about. A chord two
/// commands have shared since the editor shipped is arbitrated by their
/// availability and needs no decision, so it is one line saying how many
/// there are; the rows themselves carry which. A chord a rebind in this
/// session made shared is new, and is named in full.
pub fn advisory_text(
    pending: &PendingKeymapChanges,
    skipped: &[String],
    problem: &KeymapLoadProblem,
) -> String {
    let mut parts = Vec::new();
    if problem.is_some() {
        parts.push(problem.message.clone());
    }
    let user = pending.user_conflicts();
    if !user.is_empty() {
        parts.push(format!(
            "{} chords you have just bound are claimed by more than one command; each one fires and the commands decide between them: {}",
            user.len(),
            user.join("; ")
        ));
    }
    let shipped = pending.shipped_conflict_count();
    if shipped > 0 {
        parts.push(format!(
            "{shipped} shipped chords are shared by more than one command and arbitrated between them; the rows marked below say which."
        ));
    }
    if !skipped.is_empty() {
        parts.push(format!(
            "{} saved bindings could not be attached to a command and were left alone: {}",
            skipped.len(),
            skipped.join(", ")
        ));
    }
    parts.join("  ")
}

fn row_label(text: &str, width: f32) -> impl Bundle {
    (
        Text::new(text.to_string()),
        TextFont {
            font_size: tokens::TEXT_SIZE,
            ..Default::default()
        },
        TextColor(tokens::TEXT_PRIMARY),
        Node {
            width: px(width),
            flex_shrink: 0.0,
            ..Default::default()
        },
    )
}

fn chord_text_color(empty: bool) -> Color {
    if empty {
        tokens::TEXT_SECONDARY
    } else {
        tokens::TEXT_PRIMARY
    }
}

fn row_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        justify_content: JustifyContent::SpaceBetween,
        align_items: AlignItems::Center,
        width: percent(100),
        padding: UiRect::axes(px(tokens::SPACING_LG), px(tokens::SPACING_SM)),
        border: UiRect::bottom(px(1.0)),
        ..Default::default()
    }
}

fn row_right_node() -> Node {
    Node {
        flex_direction: FlexDirection::Row,
        align_items: AlignItems::Center,
        column_gap: px(tokens::SPACING_MD),
        flex_grow: 1.0,
        justify_content: JustifyContent::End,
        ..Default::default()
    }
}

fn spawn_category_header(commands: &mut Commands, category: &str) -> Entity {
    commands
        .spawn((
            KeybindCategoryHeader(category.to_string()),
            Node {
                padding: UiRect {
                    top: px(tokens::SPACING_LG),
                    bottom: px(tokens::SPACING_SM),
                    left: px(tokens::SPACING_LG),
                    right: px(tokens::SPACING_LG),
                },
                border: UiRect::bottom(px(2.0)),
                margin: UiRect::top(px(tokens::SPACING_SM)),
                ..Default::default()
            },
            BorderColor::all(tokens::BORDER_STRONG),
            children![(
                Text::new(category.to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE_LG,
                    weight: FontWeight::BOLD,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_PRIMARY),
            )],
        ))
        .id()
}

fn populate_keybind_dialog(
    mut commands: Commands,
    pending: Option<Res<PendingKeymapChanges>>,
    last_apply: Option<Res<LastKeymapApply>>,
    problem: Option<Res<KeymapLoadProblem>>,
    icon_font: Option<Res<jackdaw_feathers::icons::IconFont>>,
    slots: Query<Entity, (With<DialogChildrenSlot>, Added<DialogChildrenSlot>)>,
    populated: Query<(), With<KeybindDialogPopulated>>,
) {
    let Some(pending) = pending else { return };
    let skipped =
        last_apply.map_or_else(Vec::new, |report| report.0.skipped_unknown_operator.clone());
    let problem = problem.map(|it| it.clone()).unwrap_or_default();
    let icon_font = icon_font.map(|font| font.0.clone()).unwrap_or_default();

    for slot_entity in &slots {
        if !populated.is_empty() {
            continue;
        }

        commands.entity(slot_entity).insert(KeybindDialogPopulated);

        let wrapper = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                width: percent(100),
                ..Default::default()
            })
            .id();
        commands.entity(slot_entity).add_child(wrapper);

        // Filter bar: text input, key capture button, reset all.
        let filter_row = commands
            .spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: px(tokens::SPACING_MD),
                padding: UiRect::all(px(tokens::SPACING_LG)),
                width: percent(100),
                ..Default::default()
            })
            .id();
        let mut filter_props = TextEditProps::default()
            .with_placeholder("Search commands...")
            .allow_empty();
        filter_props.grow = true;
        let filter_input_wrapper = commands
            .spawn((
                KeybindFilterInput,
                Node {
                    flex_grow: 1.0,
                    ..Default::default()
                },
                children![text_edit::text_edit(filter_props)],
            ))
            .id();
        let key_filter_btn = commands
            .spawn((
                KeyFilterButton,
                button(ButtonProps::new("Key: Any").with_variant(ButtonVariant::Default)),
            ))
            .id();
        let reset_all_btn = commands
            .spawn((
                KeybindResetAllButton,
                button(
                    ButtonProps::new("Reset All to Defaults").with_variant(ButtonVariant::Default),
                ),
            ))
            .id();
        commands.entity(filter_row).add_children(&[
            filter_input_wrapper,
            key_filter_btn,
            reset_all_btn,
        ]);
        commands.entity(wrapper).add_child(filter_row);

        let advisory = commands
            .spawn((
                KeymapAdvisoryText,
                Text::new(advisory_text(&pending, &skipped, &problem)),
                TextFont {
                    font_size: tokens::TEXT_SIZE_SM,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_SECONDARY),
                Node {
                    padding: UiRect::axes(px(tokens::SPACING_LG), px(tokens::SPACING_SM)),
                    width: percent(100),
                    ..Default::default()
                },
            ))
            .id();
        commands.entity(wrapper).add_child(advisory);

        let scroll = commands
            .spawn(Node {
                flex_direction: FlexDirection::Column,
                max_height: px(460.0),
                overflow: Overflow::scroll_y(),
                width: percent(100),
                ..Default::default()
            })
            .id();
        commands.entity(wrapper).add_child(scroll);

        let mut current_category = String::new();
        for row in &pending.rows {
            let category = row.category.clone();
            if category != current_category {
                current_category = category.clone();
                let header = spawn_category_header(&mut commands, &category);
                commands.entity(scroll).add_child(header);
            }

            let chords = if row.bindable && row.fixed.is_empty() {
                pending.chords_of(&row.operator)
            } else {
                row.fixed.clone()
            };
            let chord_text = format_chords(&chords);
            let haystack = format!("{} {} {}", row.label, row.operator, chord_text).to_lowercase();

            let row_entity = commands
                .spawn((
                    KeybindRowTarget {
                        operator: Some(row.operator.clone()),
                        camera: None,
                        category: category.clone(),
                        haystack,
                    },
                    row_node(),
                    BorderColor::all(tokens::BORDER_COLOR),
                ))
                .id();

            let name_label = commands.spawn(row_label(&row.label, 240.0)).id();
            let right = commands.spawn(row_right_node()).id();

            // The badge sits with the row rather than in a paragraph at
            // the top, so which command shares a chord is answered where
            // the question is asked.
            let badge = commands
                .spawn((
                    KeymapConflictBadge(row.operator.clone()),
                    icon_button(IconButtonProps::new(Icon::TriangleAlert), &icon_font),
                    Hovered::default(),
                    Tooltip::title("Shared chord"),
                ))
                .id();
            commands
                .entity(badge)
                .entry::<Node>()
                .and_modify(|mut node| {
                    node.display = Display::None;
                });
            commands.entity(right).add_child(badge);

            if row.is_editable() {
                let chord_list = commands
                    .spawn((
                        KeybindChordList(row.operator.clone()),
                        Node {
                            flex_direction: FlexDirection::Row,
                            align_items: AlignItems::Center,
                            column_gap: px(tokens::SPACING_SM),
                            min_width: px(180.0),
                            ..Default::default()
                        },
                    ))
                    .id();
                let rebind_btn = commands
                    .spawn((
                        KeybindRebindButton(row.operator.clone()),
                        button(ButtonProps::new("Rebind").with_variant(ButtonVariant::Default)),
                    ))
                    .id();
                let add_btn = commands
                    .spawn((
                        KeybindAddChordButton(row.operator.clone()),
                        button(ButtonProps::new("Add Chord").with_variant(ButtonVariant::Ghost)),
                        Hovered::default(),
                        Tooltip::title("Add Chord").with_description(
                            "Give this command another chord alongside the ones it holds.",
                        ),
                    ))
                    .id();
                let reset_btn = commands
                    .spawn((
                        KeybindResetButton(row.operator.clone()),
                        button(ButtonProps::new("Reset").with_variant(ButtonVariant::Ghost)),
                    ))
                    .id();
                commands
                    .entity(right)
                    .add_children(&[chord_list, rebind_btn, add_btn, reset_btn]);
            } else {
                let chord_label = commands
                    .spawn((
                        KeybindDisplayText(row.operator.clone()),
                        Text::new(chord_text),
                        TextFont {
                            font_size: tokens::TEXT_SIZE,
                            ..Default::default()
                        },
                        TextColor(chord_text_color(chords.is_empty())),
                        Node {
                            min_width: px(180.0),
                            ..Default::default()
                        },
                    ))
                    .id();
                // The heading says the row cannot be changed; the tooltip
                // says why, which is the part a reader is missing.
                let note = commands
                    .spawn((
                        Text::new(row.category.clone()),
                        TextFont {
                            font_size: tokens::TEXT_SIZE_SM,
                            ..Default::default()
                        },
                        TextColor(tokens::TEXT_SECONDARY),
                        Hovered::default(),
                        Tooltip::title(row.category.clone()).with_description(row.reason()),
                    ))
                    .id();
                commands.entity(right).add_children(&[chord_label, note]);
            }

            commands
                .entity(row_entity)
                .add_children(&[name_label, right]);
            commands.entity(scroll).add_child(row_entity);
        }

        // The camera-fly rows, still on the legacy registry.
        let header = spawn_category_header(&mut commands, "Camera");
        commands.entity(scroll).add_child(header);
        for &action in CAMERA_ACTIONS {
            let bindings = pending.camera.get(&action).cloned().unwrap_or_default();
            let binding_text = format_bindings(&bindings);
            let haystack = format!("{action} {binding_text}").to_lowercase();

            let row_entity = commands
                .spawn((
                    KeybindRowTarget {
                        operator: None,
                        camera: Some(action),
                        category: "Camera".to_string(),
                        haystack,
                    },
                    row_node(),
                    BorderColor::all(tokens::BORDER_COLOR),
                ))
                .id();
            let name_label = commands.spawn(row_label(&action.to_string(), 240.0)).id();
            let right = commands.spawn(row_right_node()).id();
            let chord_label = commands
                .spawn((
                    CameraDisplayText(action),
                    Text::new(binding_text),
                    TextFont {
                        font_size: tokens::TEXT_SIZE,
                        ..Default::default()
                    },
                    TextColor(chord_text_color(bindings.is_empty())),
                    Node {
                        min_width: px(140.0),
                        ..Default::default()
                    },
                ))
                .id();
            let rebind_btn = commands
                .spawn((
                    CameraRebindButton(action, 0),
                    button(ButtonProps::new("Rebind").with_variant(ButtonVariant::Default)),
                ))
                .id();
            let reset_btn = commands
                .spawn((
                    CameraResetButton(action),
                    button(ButtonProps::new("Reset").with_variant(ButtonVariant::Ghost)),
                ))
                .id();
            commands
                .entity(right)
                .add_children(&[chord_label, rebind_btn, reset_btn]);
            commands
                .entity(row_entity)
                .add_children(&[name_label, right]);
            commands.entity(scroll).add_child(row_entity);
        }
    }
}

fn refresh_advisory(
    pending: Option<Res<PendingKeymapChanges>>,
    last_apply: Option<Res<LastKeymapApply>>,
    problem: Option<Res<KeymapLoadProblem>>,
    mut advisory: Query<&mut Text, With<KeymapAdvisoryText>>,
) {
    let Some(pending) = pending else { return };
    if !pending.is_changed() {
        return;
    }
    let problem = problem.map(|it| it.clone()).unwrap_or_default();
    let skipped =
        last_apply.map_or_else(Vec::new, |report| report.0.skipped_unknown_operator.clone());
    let text = advisory_text(&pending, &skipped, &problem);
    for mut node_text in &mut advisory {
        node_text.0 = text.clone();
    }
}

/// Draw each editable row's chords, one removable chip per chord.
///
/// Driven by the working copy rather than told by whatever changed it, so
/// a rebind, an added chord, a removed one, a reset and a reset-all all
/// reach the row through the same path.
fn refresh_chord_lists(
    mut commands: Commands,
    pending: Option<Res<PendingKeymapChanges>>,
    recording: Res<KeybindRecordingState>,
    icon_font: Option<Res<jackdaw_feathers::icons::IconFont>>,
    lists: Query<(Entity, &KeybindChordList, Option<&Children>)>,
) {
    let Some(pending) = pending else { return };
    // A list is spawned some frames after the working copy was put in the
    // world, so "the copy changed" on its own would leave every row it
    // drew for empty: a list with nothing in it has not been drawn yet.
    let stale = pending.is_changed() || recording.is_changed();
    let icon_font = icon_font.map(|font| font.0.clone()).unwrap_or_default();
    let waiting = recording
        .target
        .as_ref()
        .and_then(RecordingTarget::operator);
    let pending_confirm = recording.pending_confirm.clone();

    for (entity, list, drawn) in &lists {
        if !stale && drawn.is_some_and(|children| !children.is_empty()) {
            continue;
        }
        let Ok(mut row) = commands.get_entity(entity) else {
            continue;
        };
        row.despawn_related::<Children>();

        if waiting == Some(list.0.as_str()) {
            let prompt = match &pending_confirm {
                Some(input) => format!(
                    "{} is also bound to {} - press again to bind anyway",
                    format_preset_input(input),
                    pending.also_bound_to(&list.0, input).join(", ")
                ),
                None => "Press a key...".to_string(),
            };
            commands.spawn((
                Text::new(prompt),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(tokens::TEXT_ACCENT),
                ChildOf(entity),
            ));
            continue;
        }

        let chords = pending.chords_of(&list.0);
        if chords.is_empty() {
            commands.spawn((
                Text::new("Unbound".to_string()),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(chord_text_color(true)),
                ChildOf(entity),
            ));
            continue;
        }
        for (index, chord) in chords.into_iter().enumerate() {
            commands.spawn((
                Text::new(chord),
                TextFont {
                    font_size: tokens::TEXT_SIZE,
                    ..Default::default()
                },
                TextColor(chord_text_color(false)),
                ChildOf(entity),
            ));
            commands.spawn((
                KeybindRemoveChordButton(list.0.clone(), index),
                icon_button(IconButtonProps::new(Icon::X), &icon_font),
                Hovered::default(),
                Tooltip::title("Remove Chord"),
                ChildOf(entity),
            ));
        }
    }
}

/// Show the badge on each row whose chords another command also claims,
/// and say which commands in the row's own vocabulary.
fn refresh_conflict_badges(
    pending: Option<Res<PendingKeymapChanges>>,
    mut badges: Query<(&KeymapConflictBadge, &mut Node, &mut Tooltip)>,
    added: Query<(), Added<KeymapConflictBadge>>,
) {
    let Some(pending) = pending else { return };
    // Same as the chord lists: a badge spawned after the working copy
    // changed has never been told what it is about.
    if !pending.is_changed() && added.is_empty() {
        return;
    }
    for (badge, mut node, mut tooltip) in &mut badges {
        let conflicts = pending.conflicts_of(&badge.0);
        node.display = if conflicts.is_empty() {
            Display::None
        } else {
            Display::Flex
        };
        tooltip.description = conflicts.join("\n");
    }
}

fn on_remove_chord_click(
    event: On<ButtonClickEvent>,
    buttons: Query<&KeybindRemoveChordButton>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    pending: Option<ResMut<PendingKeymapChanges>>,
) {
    let Ok(button) = buttons.get(event.entity) else {
        return;
    };
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }
    let Some(mut pending) = pending else { return };
    pending.remove_chord(&button.0, button.1);
}

fn on_key_filter_click(
    event: On<ButtonClickEvent>,
    key_filter_buttons: Query<&ChildOf, With<KeyFilterButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut key_filter: ResMut<KeyFilterState>,
    mut registry: ResMut<KeybindRegistry>,
    recording: Res<KeybindRecordingState>,
    children_query: Query<&Children>,
    captions: Query<(), With<ButtonContentText>>,
    mut texts: Query<&mut Text>,
) {
    let Ok(_) = key_filter_buttons.get(event.entity) else {
        return;
    };
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }
    if recording.is_recording() {
        return;
    }

    if key_filter.active_key.is_some() {
        key_filter.active_key = None;
        key_filter.capturing = false;
        set_button_text(
            event.entity,
            "Key: Any",
            &children_query,
            &captions,
            &mut texts,
        );
    } else if key_filter.capturing {
        key_filter.capturing = false;
        registry.recording = false;
        set_button_text(
            event.entity,
            "Key: Any",
            &children_query,
            &captions,
            &mut texts,
        );
    } else {
        key_filter.capturing = true;
        registry.recording = true;
        set_button_text(
            event.entity,
            "Press a key...",
            &children_query,
            &captions,
            &mut texts,
        );
    }
}

/// Write a caption into a button. The caption hangs in a clipping slot under
/// the button rather than off it directly, so it is found by its own marker;
/// walking the button's direct children finds nothing.
fn set_button_text(
    button_entity: Entity,
    label: &str,
    children_query: &Query<&Children>,
    captions: &Query<(), With<ButtonContentText>>,
    texts: &mut Query<&mut Text>,
) {
    if let Some(caption) = button_caption(button_entity, children_query, captions)
        && let Ok(mut text) = texts.get_mut(caption)
    {
        text.0 = label.to_string();
    }
}

fn capture_key_filter(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut key_filter: ResMut<KeyFilterState>,
    mut registry: ResMut<KeybindRegistry>,
    recording: Res<KeybindRecordingState>,
    key_filter_btns: Query<Entity, With<KeyFilterButton>>,
    children_query: Query<&Children>,
    captions: Query<(), With<ButtonContentText>>,
    mut texts: Query<&mut Text>,
) {
    if !key_filter.capturing {
        return;
    }
    if recording.is_recording() {
        return;
    }

    // Right-click or Escape cancels.
    if mouse.just_pressed(MouseButton::Right) || keyboard.just_pressed(KeyCode::Escape) {
        key_filter.capturing = false;
        registry.recording = false;
        for btn in &key_filter_btns {
            set_button_text(btn, "Key: Any", &children_query, &captions, &mut texts);
        }
        return;
    }

    for key in keyboard.get_just_pressed() {
        if is_modifier(*key) {
            continue;
        }

        key_filter.capturing = false;
        key_filter.active_key = Some(*key);
        registry.recording = false;

        let label = format!(
            "Key: {} (click to clear)",
            jackdaw_commands::keybinds::key_display_name(*key)
        );
        for btn in &key_filter_btns {
            set_button_text(btn, &label, &children_query, &captions, &mut texts);
        }
        return;
    }
}

fn is_modifier(key: KeyCode) -> bool {
    matches!(
        key,
        KeyCode::ControlLeft
            | KeyCode::ControlRight
            | KeyCode::ShiftLeft
            | KeyCode::ShiftRight
            | KeyCode::AltLeft
            | KeyCode::AltRight
            | KeyCode::SuperLeft
            | KeyCode::SuperRight
    )
}

/// Show/hide rows and category headers based on both text and key filters.
fn apply_keybind_filter(
    filter_wrappers: Query<&Children, With<KeybindFilterInput>>,
    text_values: Query<&TextEditValue, Changed<TextEditValue>>,
    all_text_values: Query<&TextEditValue>,
    key_filter: Res<KeyFilterState>,
    pending: Option<Res<PendingKeymapChanges>>,
    mut rows: Query<(&KeybindRowTarget, &mut Node)>,
    mut headers: Query<(&KeybindCategoryHeader, &mut Node), Without<KeybindRowTarget>>,
) {
    let filter_text = filter_wrappers.iter().find_map(|children| {
        children
            .iter()
            .find_map(|child| all_text_values.get(child).ok())
    });
    let Some(filter_value) = filter_text else {
        return;
    };

    let text_changed = filter_wrappers
        .iter()
        .any(|children| children.iter().any(|child| text_values.get(child).is_ok()));
    if !text_changed && !key_filter.is_changed() {
        return;
    }
    let Some(pending) = pending else { return };

    let text_query = filter_value.0.trim().to_lowercase();
    let key_filter_active = key_filter.active_key;
    let key_name = key_filter_active.map(key_code_name);

    let mut visible_categories: std::collections::HashSet<String> =
        std::collections::HashSet::new();

    for (row, mut node) in &mut rows {
        let text_match = text_query.is_empty()
            || row.haystack.contains(&text_query)
            || row.category.to_lowercase().contains(&text_query);

        let key_match = match (&key_name, &row.operator, row.camera) {
            (None, _, _) => true,
            (Some(name), Some(operator), _) => pending.bindings.iter().any(|binding| {
                binding.operator == *operator
                    && matches!(&binding.input, PresetInput::Key { key, .. } if key == name)
            }),
            (Some(_), None, Some(action)) => pending
                .camera
                .get(&action)
                .is_some_and(|binds| binds.iter().any(|b| Some(b.key) == key_filter_active)),
            (Some(_), None, None) => false,
        };

        let visible = text_match && key_match;
        node.display = if visible {
            visible_categories.insert(row.category.clone());
            Display::Flex
        } else {
            Display::None
        };
    }

    for (header, mut node) in &mut headers {
        let has_filters = !text_query.is_empty() || key_filter_active.is_some();
        node.display = if !has_filters || visible_categories.contains(&header.0) {
            Display::Flex
        } else {
            Display::None
        };
    }
}

#[expect(
    clippy::too_many_arguments,
    reason = "one handler covering the three ways a recording starts"
)]
fn on_rebind_click(
    event: On<ButtonClickEvent>,
    operator_buttons: Query<&KeybindRebindButton>,
    add_buttons: Query<&KeybindAddChordButton>,
    camera_buttons: Query<&CameraRebindButton>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut capture: ResMut<KeymapCapture>,
    mut registry: ResMut<KeybindRegistry>,
    mut camera_texts: Query<
        (&CameraDisplayText, &mut Text, &mut TextColor),
        Without<KeybindDisplayText>,
    >,
) {
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }

    let operator = operator_buttons
        .get(event.entity)
        .map(|btn| (btn.0.clone(), RecordingMode::Replace))
        .or_else(|_| {
            add_buttons
                .get(event.entity)
                .map(|btn| (btn.0.clone(), RecordingMode::Add))
        });
    if let Ok((operator, mode)) = operator {
        recording_state.target = Some(RecordingTarget::Operator(operator, mode));
        recording_state.pending_confirm = None;
        capture.recording = true;
        registry.recording = true;
        return;
    }

    if let Ok(btn) = camera_buttons.get(event.entity) {
        recording_state.target = Some(RecordingTarget::Camera(btn.0, btn.1));
        recording_state.pending_confirm = None;
        capture.recording = true;
        registry.recording = true;
        for (display, mut text, mut color) in &mut camera_texts {
            if display.0 == btn.0 {
                text.0 = "Press a key...".to_string();
                color.0 = tokens::TEXT_ACCENT;
            }
        }
    }
}

/// The chord the user is pressing, as a keymap row's input.
///
/// Returns `None` while only modifiers are down, and for a key or button
/// with no name in the preset format: an unnamed key would serialize to
/// something that cannot be parsed back, so it is refused at the point
/// of capture rather than written into the file.
pub fn capture_input(
    keyboard: &ButtonInput<KeyCode>,
    mouse: &ButtonInput<MouseButton>,
) -> Option<PresetInput> {
    let ctrl = keyboard.any_pressed([KeyCode::ControlLeft, KeyCode::ControlRight]);
    let shift = keyboard.any_pressed([KeyCode::ShiftLeft, KeyCode::ShiftRight]);
    let alt = keyboard.any_pressed([KeyCode::AltLeft, KeyCode::AltRight]);
    let super_ = keyboard.any_pressed([KeyCode::SuperLeft, KeyCode::SuperRight]);

    let apply = |input: PresetInput| {
        let mut input = input;
        if ctrl {
            input = input.ctrl();
        }
        if shift {
            input = input.shift();
        }
        if alt {
            input = input.alt();
        }
        if super_ {
            input = input.super_();
        }
        input
    };

    for key in keyboard.get_just_pressed() {
        if is_modifier(*key) {
            continue;
        }
        let name = key_code_name(*key);
        if key_code_from_name(&name).is_none() {
            warn!("ignoring a key with no name in the keymap format: {key:?}");
            continue;
        }
        return Some(apply(PresetInput::key(&name)));
    }

    for button in mouse.get_just_pressed() {
        // Right-click is the recorder's own cancel gesture.
        if *button == MouseButton::Right {
            continue;
        }
        let Some(name) = mouse_button_name(*button) else {
            continue;
        };
        return Some(apply(PresetInput::mouse(&name)));
    }

    None
}

#[expect(
    clippy::too_many_arguments,
    reason = "one recorder covering the operator rows and the legacy camera rows"
)]
fn capture_keybind_recording(
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse: Res<ButtonInput<MouseButton>>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut capture: ResMut<KeymapCapture>,
    mut registry: ResMut<KeybindRegistry>,
    pending: Option<ResMut<PendingKeymapChanges>>,
    mut camera_texts: Query<
        (&CameraDisplayText, &mut Text, &mut TextColor),
        Without<KeybindDisplayText>,
    >,
    mut commands: Commands,
    dialog_exists: Query<(), With<EditorDialog>>,
    settings_open: Option<Res<KeybindSettingsOpen>>,
) {
    let Some(target) = recording_state.target.clone() else {
        if settings_open.is_some()
            && !dialog_exists.is_empty()
            && keyboard.just_pressed(KeyCode::Escape)
        {
            commands.trigger(CloseDialogEvent);
        }
        return;
    };
    let Some(mut pending) = pending else {
        return;
    };

    // Right-click cancels, including out of a conflict confirmation.
    if mouse.just_pressed(MouseButton::Right) {
        recording_state.target = None;
        recording_state.pending_confirm = None;
        capture.recording = false;
        registry.recording = false;
        redraw_target(&target, &pending, &mut camera_texts);
        return;
    }

    let Some(input) = capture_input(&keyboard, &mouse) else {
        return;
    };

    match &target {
        RecordingTarget::Operator(operator, mode) => {
            let confirmed = recording_state.pending_confirm.as_ref() == Some(&input);
            let others = pending.also_bound_to(operator, &input);
            if !others.is_empty() && !confirmed {
                recording_state.pending_confirm = Some(input.clone());
                return;
            }
            match mode {
                RecordingMode::Replace => pending.rebind(operator, input),
                RecordingMode::Add => pending.add_chord(operator, input),
            }
        }
        RecordingTarget::Camera(action, index) => {
            let PresetInput::Key {
                key,
                ctrl,
                shift,
                alt,
                ..
            } = &input
            else {
                return;
            };
            let Some(key) = key_code_from_name(key) else {
                return;
            };
            let new_bind = Keybind {
                key,
                ctrl: *ctrl,
                shift: *shift,
                alt: *alt,
                mouse: None,
            };
            let binds = pending.camera.entry(*action).or_default();
            if *index < binds.len() {
                binds[*index] = new_bind;
            } else {
                *binds = vec![new_bind];
            }
        }
    }

    recording_state.target = None;
    recording_state.pending_confirm = None;
    capture.recording = false;
    registry.recording = false;
    redraw_target(&target, &pending, &mut camera_texts);
}

fn redraw_target(
    target: &RecordingTarget,
    pending: &PendingKeymapChanges,
    camera_texts: &mut Query<
        (&CameraDisplayText, &mut Text, &mut TextColor),
        Without<KeybindDisplayText>,
    >,
) {
    match target {
        // An operator row's chords are drawn by `refresh_chord_lists`,
        // which follows the working copy rather than being told.
        RecordingTarget::Operator(..) => {}
        RecordingTarget::Camera(action, _) => {
            let binds = pending.camera.get(action).cloned().unwrap_or_default();
            let text_str = format_bindings(&binds);
            let color_value = chord_text_color(binds.is_empty());
            for (display, mut text, mut color) in camera_texts.iter_mut() {
                if display.0 == *action {
                    text.0 = text_str.clone();
                    color.0 = color_value;
                }
            }
        }
    }
}

fn on_reset_click(
    event: On<ButtonClickEvent>,
    operator_buttons: Query<&KeybindResetButton>,
    camera_buttons: Query<&CameraResetButton>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    pending: Option<ResMut<PendingKeymapChanges>>,
    mut camera_texts: Query<
        (&CameraDisplayText, &mut Text, &mut TextColor),
        Without<KeybindDisplayText>,
    >,
) {
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }
    let Some(mut pending) = pending else {
        return;
    };

    if let Ok(btn) = operator_buttons.get(event.entity) {
        pending.reset(&btn.0);
        redraw_target(
            &RecordingTarget::Operator(btn.0.clone(), RecordingMode::Replace),
            &pending,
            &mut camera_texts,
        );
        return;
    }

    if let Ok(btn) = camera_buttons.get(event.entity) {
        let defaults = KeybindRegistry::default();
        let default_bindings = defaults.bindings.get(&btn.0).cloned().unwrap_or_default();
        pending.camera.insert(btn.0, default_bindings);
        redraw_target(
            &RecordingTarget::Camera(btn.0, 0),
            &pending,
            &mut camera_texts,
        );
    }
}

fn on_reset_all_click(
    event: On<ButtonClickEvent>,
    reset_all_buttons: Query<(), With<KeybindResetAllButton>>,
    parents: Query<&ChildOf>,
    dialogs: Query<(), With<EditorDialog>>,
    pending: Option<ResMut<PendingKeymapChanges>>,
    mut texts: Query<(&KeybindDisplayText, &mut Text, &mut TextColor)>,
    mut camera_texts: Query<
        (&CameraDisplayText, &mut Text, &mut TextColor),
        Without<KeybindDisplayText>,
    >,
) {
    if reset_all_buttons.get(event.entity).is_err() {
        return;
    }
    if !is_in_dialog(event.entity, &parents, &dialogs) {
        return;
    }
    let Some(mut pending) = pending else {
        return;
    };

    pending.reset_all();
    pending.camera = KeybindRegistry::default().bindings;

    for (display, mut text, mut color) in &mut texts {
        let chords = pending.chords_of(&display.0);
        text.0 = format_chords(&chords);
        color.0 = chord_text_color(chords.is_empty());
    }
    for (display, mut text, mut color) in &mut camera_texts {
        let binds = pending.camera.get(&display.0).cloned().unwrap_or_default();
        text.0 = format_bindings(&binds);
        color.0 = chord_text_color(binds.is_empty());
    }
}

fn on_keybind_settings_save(
    _event: On<DialogActionEvent>,
    mut commands: Commands,
    pending: Option<Res<PendingKeymapChanges>>,
    settings_open: Option<Res<KeybindSettingsOpen>>,
    mut capture: ResMut<KeymapCapture>,
    mut registry: ResMut<KeybindRegistry>,
) {
    if settings_open.is_none() {
        return;
    }

    if let Some(pending) = pending {
        registry.bindings = pending.camera.clone();
        let user = pending.to_user_keymap();
        save_user_keymap(&user);
        // Re-applied from an exclusive command so it runs once the dialog
        // is gone and the working copy has been dropped, rather than in
        // the middle of the click that saved it.
        commands.queue(move |world: &mut World| {
            world.insert_resource(user);
            crate::extension_lifecycle::apply_active_keymap(world);
        });
    }
    registry.recording = false;
    capture.recording = false;

    crate::keybinds::save_keybinds(&registry);

    commands.remove_resource::<PendingKeymapChanges>();
    commands.remove_resource::<KeybindSettingsOpen>();
}

fn cleanup_on_dialog_close(
    mut commands: Commands,
    settings_open: Option<Res<KeybindSettingsOpen>>,
    dialogs: Query<(), With<EditorDialog>>,
    mut registry: ResMut<KeybindRegistry>,
    mut recording_state: ResMut<KeybindRecordingState>,
    mut capture: ResMut<KeymapCapture>,
    mut key_filter: ResMut<KeyFilterState>,
) {
    if settings_open.is_none() {
        return;
    }
    if !dialogs.is_empty() {
        return;
    }

    registry.recording = false;
    recording_state.target = None;
    recording_state.pending_confirm = None;
    capture.recording = false;
    *key_filter = KeyFilterState::default();
    commands.remove_resource::<PendingKeymapChanges>();
    commands.remove_resource::<KeybindSettingsOpen>();
}

fn is_in_dialog(
    start: Entity,
    parents: &Query<&ChildOf>,
    dialogs: &Query<(), With<EditorDialog>>,
) -> bool {
    let mut current = start;
    loop {
        if dialogs.get(current).is_ok() {
            return true;
        }
        let Ok(child_of) = parents.get(current) else {
            return false;
        };
        current = child_of.parent();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use jackdaw_feathers::icons::{EditorFont, IconFont};

    /// The filter button, ticked far enough for its caption to exist, with
    /// the capture pass as the only system running.
    fn app_with_filter_button() -> (App, Entity) {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            jackdaw_feathers::button::plugin,
        ));
        app.init_asset::<bevy::text::Font>();
        app.init_resource::<bevy::input_focus::InputFocus>();
        app.insert_resource(IconFont(Handle::default()));
        app.insert_resource(EditorFont(Handle::default()));
        app.init_resource::<ButtonInput<KeyCode>>();
        app.init_resource::<ButtonInput<MouseButton>>();
        app.init_resource::<KeyFilterState>();
        app.init_resource::<KeybindRecordingState>();
        app.init_resource::<KeybindRegistry>();
        app.add_systems(Update, capture_key_filter);
        let button_entity = app
            .world_mut()
            .spawn((
                KeyFilterButton,
                button(ButtonProps::new("Key: Any").with_variant(ButtonVariant::Default)),
            ))
            .id();
        app.update();
        app.update();
        (app, button_entity)
    }

    /// What the button is currently drawing.
    fn caption(app: &App, button_entity: Entity) -> String {
        let mut stack = vec![button_entity];
        while let Some(entity) = stack.pop() {
            if app.world().get::<ButtonContentText>(entity).is_some()
                && let Some(text) = app.world().get::<Text>(entity)
            {
                return text.0.clone();
            }
            if let Some(children) = app.world().get::<Children>(entity) {
                stack.extend(children.iter());
            }
        }
        panic!("the filter button draws a caption");
    }

    /// Armed capture has no other sign of itself: the button's caption is
    /// the whole feedback, and it hangs in a clipping slot below the
    /// button's direct children.
    #[test]
    fn a_captured_key_reaches_the_buttons_caption() {
        let (mut app, button_entity) = app_with_filter_button();
        assert_eq!(caption(&app, button_entity), "Key: Any");

        app.world_mut().resource_mut::<KeyFilterState>().capturing = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F5);
        app.update();

        assert_eq!(
            caption(&app, button_entity),
            "Key: F5 (click to clear)",
            "the captured key is what the button says it is filtering on",
        );
    }

    /// Backing out of capture puts the resting caption back.
    #[test]
    fn cancelling_capture_puts_the_resting_caption_back() {
        let (mut app, button_entity) = app_with_filter_button();
        app.world_mut().resource_mut::<KeyFilterState>().capturing = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::F5);
        app.update();
        assert_eq!(caption(&app, button_entity), "Key: F5 (click to clear)");

        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .clear();
        app.world_mut().resource_mut::<KeyFilterState>().capturing = true;
        app.world_mut()
            .resource_mut::<ButtonInput<KeyCode>>()
            .press(KeyCode::Escape);
        app.update();

        assert_eq!(caption(&app, button_entity), "Key: Any");
    }

    fn row(operator: &str, key: &str) -> PresetBinding {
        PresetBinding {
            operator: operator.to_string(),
            input: PresetInput::key(key),
            phase: PresetPhase::Press,
            context: PresetContext::Operators,
        }
    }

    fn pending_with(defaults: Vec<PresetBinding>) -> PendingKeymapChanges {
        PendingKeymapChanges {
            rows: vec![
                KeybindRow {
                    operator: "history.undo".into(),
                    label: "Undo".into(),
                    description: String::new(),
                    category: "History".into(),
                    fixed: Vec::new(),
                    bindable: true,
                },
                KeybindRow {
                    operator: "history.redo".into(),
                    label: "Redo".into(),
                    description: String::new(),
                    category: "History".into(),
                    fixed: Vec::new(),
                    bindable: true,
                },
            ],
            bindings: defaults.clone(),
            defaults,
            unresolved: Vec::new(),
            camera: HashMap::new(),
        }
    }

    #[test]
    fn a_rebind_replaces_every_chord_the_operator_had() {
        let mut pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.undo", "KeyU"),
            row("history.redo", "KeyY"),
        ]);
        pending.rebind("history.undo", PresetInput::key("F9"));
        assert_eq!(pending.chords_of("history.undo"), vec!["F9".to_string()]);
        assert_eq!(pending.chords_of("history.redo"), vec!["Y".to_string()]);
    }

    #[test]
    fn resetting_one_operator_restores_only_that_one() {
        let mut pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.redo", "KeyY"),
        ]);
        pending.rebind("history.undo", PresetInput::key("F9"));
        pending.rebind("history.redo", PresetInput::key("F10"));
        pending.reset("history.undo");
        assert_eq!(pending.chords_of("history.undo"), vec!["Z".to_string()]);
        assert_eq!(
            pending.chords_of("history.redo"),
            vec!["F10".to_string()],
            "reset must not reach the row next to it"
        );
    }

    #[test]
    fn resetting_all_restores_the_shipped_keymap() {
        let defaults = vec![row("history.undo", "KeyZ"), row("history.redo", "KeyY")];
        let mut pending = pending_with(defaults.clone());
        pending.rebind("history.undo", PresetInput::key("F9"));
        pending.rebind("history.redo", PresetInput::key("F10"));
        pending.reset_all();
        assert_eq!(pending.bindings, defaults);
        assert_eq!(pending.to_user_keymap(), UserKeymap::default());
    }

    /// Only what the user changed reaches the file: a saved keymap that
    /// copied the defaults would freeze them, and a later change to a
    /// shipped chord would never reach anyone who had opened this dialog.
    #[test]
    fn saving_writes_only_the_operators_that_changed() {
        let mut pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.redo", "KeyY"),
        ]);
        pending.rebind("history.undo", PresetInput::key("F9"));
        let user = pending.to_user_keymap();
        assert_eq!(user.bindings, vec![row("history.undo", "F9")]);
    }

    /// A binding for a command that is not loaded right now is the
    /// user's, not this session's to delete.
    #[test]
    fn saving_carries_an_unresolvable_row_through_untouched() {
        let mut pending = pending_with(vec![row("history.undo", "KeyZ")]);
        pending.unresolved = vec![row("some.extension.op", "F12")];
        assert_eq!(
            pending.to_user_keymap().bindings,
            vec![row("some.extension.op", "F12")]
        );

        pending.rebind("history.undo", PresetInput::key("F9"));
        assert_eq!(
            pending.to_user_keymap().bindings,
            vec![row("some.extension.op", "F12"), row("history.undo", "F9")]
        );
    }

    #[test]
    fn a_chord_another_command_holds_is_reported_by_label() {
        let pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.redo", "KeyY"),
        ]);
        assert_eq!(
            pending.also_bound_to("history.redo", &PresetInput::key("KeyZ")),
            vec!["Undo".to_string()]
        );
        assert!(
            pending
                .also_bound_to("history.undo", &PresetInput::key("KeyZ"))
                .is_empty(),
            "an operator does not conflict with itself"
        );
    }

    /// The dialog reports a shared chord and binds it anyway: both
    /// commands keep it, and their availability decides between them.
    #[test]
    fn binding_a_shared_chord_leaves_the_other_command_holding_it() {
        let mut pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.redo", "KeyY"),
        ]);
        pending.rebind("history.redo", PresetInput::key("KeyZ"));
        assert_eq!(pending.chords_of("history.undo"), vec!["Z".to_string()]);
        assert_eq!(pending.chords_of("history.redo"), vec!["Z".to_string()]);
        assert_eq!(
            pending.conflicts().len(),
            1,
            "the shared chord must be reported"
        );
    }

    #[test]
    fn the_advisory_names_the_conflicts_and_the_skips() {
        let mut pending = pending_with(vec![
            row("history.undo", "KeyZ"),
            row("history.redo", "KeyY"),
        ]);
        pending.rebind("history.redo", PresetInput::key("KeyZ"));
        let text = advisory_text(
            &pending,
            &["some.extension.op".to_string()],
            &KeymapLoadProblem::default(),
        );
        assert!(text.contains("claimed by more than one command"), "{text}");
        assert!(text.contains("some.extension.op"), "{text}");
    }

    #[test]
    fn an_unchanged_keymap_has_nothing_to_advise() {
        let pending = pending_with(vec![row("history.undo", "KeyZ")]);
        assert_eq!(
            advisory_text(&pending, &[], &KeymapLoadProblem::default()),
            ""
        );
    }

    #[test]
    fn capture_refuses_a_key_the_keymap_format_cannot_name() {
        let mut keyboard = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keyboard.press(KeyCode::Unidentified(
            bevy::input::keyboard::NativeKeyCode::Unidentified,
        ));
        assert_eq!(
            capture_input(&keyboard, &mouse),
            None,
            "a key with no name in the format must never reach a saved row"
        );

        let mut keyboard = ButtonInput::<KeyCode>::default();
        keyboard.press(KeyCode::F9);
        assert_eq!(
            capture_input(&keyboard, &mouse),
            Some(PresetInput::key("F9"))
        );
    }

    #[test]
    fn capture_reads_the_modifiers_that_are_held() {
        let mut keyboard = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keyboard.press(KeyCode::ControlLeft);
        keyboard.press(KeyCode::ShiftLeft);
        keyboard.press(KeyCode::KeyC);
        assert_eq!(
            capture_input(&keyboard, &mouse),
            Some(PresetInput::key("KeyC").ctrl().shift())
        );
    }

    #[test]
    fn capture_ignores_a_press_that_is_only_modifiers() {
        let mut keyboard = ButtonInput::<KeyCode>::default();
        let mouse = ButtonInput::<MouseButton>::default();
        keyboard.press(KeyCode::ControlLeft);
        assert_eq!(capture_input(&keyboard, &mouse), None);
    }

    #[test]
    fn capture_reads_a_mouse_button_by_its_preset_name() {
        let keyboard = ButtonInput::<KeyCode>::default();
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Middle);
        assert_eq!(
            capture_input(&keyboard, &mouse),
            Some(PresetInput::mouse("Middle"))
        );

        // Right-click is how a recording is cancelled, so it never records.
        let mut mouse = ButtonInput::<MouseButton>::default();
        mouse.press(MouseButton::Right);
        assert_eq!(capture_input(&keyboard, &mouse), None);
    }

    #[test]
    fn a_category_is_the_head_of_the_operator_id() {
        assert_eq!(category_of("brush.mesh.subdivide"), "Brush");
        assert_eq!(category_of("history.undo"), "History");
        assert_eq!(category_of("command_palette.toggle"), "Command palette");
    }
}
