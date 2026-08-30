use bevy::feathers::constants::size::CHECKBOX_SIZE;
use bevy::feathers::controls::{
    ButtonVariant as FeathersButtonVariant, FeathersButton, FeathersCheckbox, FeathersToolButton,
};
use bevy::feathers::theme::{ThemeBackgroundColor, ThemeToken, ThemedText, UiTheme};
use bevy::input_focus::tab_navigation::TabIndex;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::Activate;
use jackdaw_scene_types::PropertyValue;
use lucide_icons::Icon;
use std::borrow::Cow;

use crate::icons::EditorFont;
use crate::tokens::{
    BORDER_RADIUS_MD, DESTRUCTIVE_RED, DESTRUCTIVE_RED_HOVER, TEXT_BODY_COLOR, TEXT_DISPLAY_COLOR,
    TEXT_MUTED_COLOR, TEXT_SIZE, TEXT_SIZE_SM,
};

#[derive(EntityEvent)]
pub struct ButtonClickEvent {
    pub entity: Entity,
}

/// Attached to a button to declare that clicking it should dispatch
/// the operator with this id, optionally passing concrete parameters.
/// The editor's click observer fires the operator; the tooltip
/// renderer formats the call signature for hover help, so two buttons
/// targeting the same operator with different args show different
/// signatures.
///
/// Feathers carries this as a plain component to keep the widget
/// crate independent of the operator API.
///
/// `Default` is derived so the component can be authored directly in a
/// `bsn!` scene, whose blanket `FromTemplate` impl needs `Clone +
/// Default`; see [`operator_button`]. A defaulted value has an empty
/// id and dispatches nothing, so it is only meaningful when constructed
/// with a real id via [`ButtonOperatorCall::new`].
#[derive(Component, Clone, Debug, Default)]
pub struct ButtonOperatorCall {
    pub id: Cow<'static, str>,
    pub params: Vec<(Cow<'static, str>, PropertyValue)>,
}

impl ButtonOperatorCall {
    /// Plain operator dispatch, no params.
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self {
            id: id.into(),
            params: Vec::new(),
        }
    }

    /// Add a parameter. Builder-style so call sites can chain.
    #[must_use]
    pub fn with_param(
        mut self,
        key: impl Into<Cow<'static, str>>,
        value: impl Into<PropertyValue>,
    ) -> Self {
        self.params.push((key.into(), value.into()));
        self
    }
}

/// A [`FeathersButton`] wired to dispatch operator `op_id` when
/// clicked, as a `Scene` for composing inside a `bsn!` `Children [ ... ]`
/// list.
///
/// The button carries a [`ButtonOperatorCall`], which the editor's
/// operator-button glue reads to dispatch the operator on the `Activate`
/// event, toggle [`InteractionDisabled`] whenever the operator reports
/// itself unavailable, and attach a hover tooltip via the
/// `Add, ButtonOperatorCall` observer.
pub fn operator_button(
    op_id: impl Into<Cow<'static, str>>,
    caption: impl Into<String>,
) -> impl Scene {
    operator_button_variant(op_id, caption, FeathersButtonVariant::Normal)
}

/// [`operator_button`] with an explicit button variant, e.g.
/// `ButtonVariant::Primary` for a call-to-action button.
pub fn operator_button_variant(
    op_id: impl Into<Cow<'static, str>>,
    caption: impl Into<String>,
    variant: FeathersButtonVariant,
) -> impl Scene {
    let op_id = op_id.into();
    let caption = caption.into();
    bsn! {
        @FeathersButton {
            @caption: bsn! { Text(caption) ThemedText },
            @variant: {variant}
        }
        ButtonOperatorCall::new(op_id)
    }
}

impl std::fmt::Display for ButtonOperatorCall {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.id)?;
        f.write_str("(")?;
        for (i, (k, v)) in self.params.iter().enumerate() {
            if i > 0 {
                f.write_str(", ")?;
            }
            write!(f, "{k}: {v}")?;
        }
        f.write_str(")")
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParseOpActionError {
    /// Input does not start with [`crate::menu_bar::OP_ACTION_PREFIX`].
    MissingPrefix,
}

impl std::fmt::Display for ParseOpActionError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::MissingPrefix => f.write_str("missing `op:` prefix"),
        }
    }
}

impl std::error::Error for ParseOpActionError {}

/// Parse a menu/context-menu action string of the form
/// `op:OP_ID?key=value&key2=value2` into a [`ButtonOperatorCall`].
/// Values are stored as `PropertyValue::String`; the runtime
/// `OperatorParameters::as_int` / `as_bool` accessors coerce numeric
/// and bool params from string form. Future menu entries that need
/// typed values should construct the call directly with
/// [`ButtonOperatorCall::with_param`].
///
/// `&String` and `&Cow<str>` deref to `&str`, so this impl covers
/// every action-string source the editor currently has.
impl TryFrom<&str> for ButtonOperatorCall {
    type Error = ParseOpActionError;

    fn try_from(value: &str) -> Result<Self, Self::Error> {
        let rest = value
            .strip_prefix(crate::menu_bar::OP_ACTION_PREFIX)
            .ok_or(ParseOpActionError::MissingPrefix)?;
        let (op_id, query) = rest.split_once('?').unwrap_or((rest, ""));
        let mut call = ButtonOperatorCall::new(op_id.to_string());
        for kv in query.split('&').filter(|s| !s.is_empty()) {
            if let Some((k, v)) = kv.split_once('=') {
                call = call.with_param(k.to_string(), v.to_string());
            }
        }
        Ok(call)
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(Startup, register_button_theme_tokens)
        .add_systems(Update, (setup_button, paint_variant_background))
        .add_observer(fire_click_on_activate);
}

/// Design tokens for the button looks [`FeathersButtonVariant`] has no
/// entry for. Registered into [`UiTheme`] at startup, and set on the
/// button entity as a [`ThemeBackgroundColor`] whenever its
/// [`ButtonVariant`] names one of them.
pub const BUTTON_DESTRUCTIVE_BG: ThemeToken =
    ThemeToken::new_static("jackdaw.button.destructive.bg");
/// See [`BUTTON_DESTRUCTIVE_BG`].
pub const BUTTON_DESTRUCTIVE_BG_HOVER: ThemeToken =
    ThemeToken::new_static("jackdaw.button.destructive.bg.hover");
/// See [`BUTTON_DESTRUCTIVE_BG`].
pub const BUTTON_ACTIVE_BG: ThemeToken = ThemeToken::new_static("jackdaw.button.active.bg");
/// See [`BUTTON_DESTRUCTIVE_BG`].
pub const BUTTON_ACTIVE_ALT_BG: ThemeToken = ThemeToken::new_static("jackdaw.button.active-alt.bg");

/// Adds the editor's own button colours to the feathers theme, so a
/// variant feathers does not carry is still expressed as a theme token
/// on a plain [`FeathersButton`] rather than as a second button widget.
///
/// The theme resource is only present once `FeathersPlugins` is added,
/// and the editor replaces it wholesale at startup; taking it as an
/// option keeps a widget-only test app (which has neither) working.
fn register_button_theme_tokens(theme: Option<ResMut<UiTheme>>) {
    let Some(mut theme) = theme else {
        return;
    };
    let mut set = |token: ThemeToken, color: Srgba| {
        theme.0.color.insert(token, color.into());
    };
    set(BUTTON_DESTRUCTIVE_BG, DESTRUCTIVE_RED);
    set(BUTTON_DESTRUCTIVE_BG_HOVER, DESTRUCTIVE_RED_HOVER);
    // Solid surface grey; toolbar active-tool indicators and combobox
    // selected rows share this treatment.
    set(BUTTON_ACTIVE_BG, Srgba::new(0.314, 0.314, 0.314, 1.0));
    set(BUTTON_ACTIVE_ALT_BG, TEXT_BODY_COLOR.with_alpha(0.05));
}

#[derive(Component)]
pub struct EditorButton;

/// Marker on the text entity that holds a button's main content
/// string. External systems use this to mutate the displayed label
/// without re-spawning the button (e.g. the gizmo space toggle that
/// flips between "World" and "Local" while keeping the same button).
#[derive(Component)]
pub struct ButtonContentText;

/// The entity drawing `button`'s caption, wherever the widget put it.
///
/// The caption hangs in a clipping slot rather than directly off the
/// button, and a button that also carries an icon has more than one text
/// child, so the first text child may be the icon's glyph.
pub fn button_caption(
    button: Entity,
    children: &Query<&Children>,
    captions: &Query<(), With<ButtonContentText>>,
) -> Option<Entity> {
    let mut stack = vec![button];
    while let Some(entity) = stack.pop() {
        if captions.contains(entity) {
            return Some(entity);
        }
        if let Ok(kids) = children.get(entity) {
            stack.extend(kids.iter());
        }
    }
    None
}

/// The editor's button looks, each resolved onto a
/// [`FeathersButtonVariant`] by [`ButtonVariant::feathers`]. The four
/// that feathers has no equivalent for carry a theme token instead; see
/// [`ButtonVariant::background_token`].
#[derive(Component, Default, Clone, Copy, PartialEq, Eq, Debug)]
pub enum ButtonVariant {
    #[default]
    Default,
    Primary,
    Destructive,
    Ghost,
    Active,
    ActiveAlt,
    Disabled,
}

#[derive(Component, Default, Clone, Copy)]
pub enum ButtonSize {
    #[default]
    MD,
    Icon,
    IconSM,
}

impl ButtonVariant {
    /// The feathers variant this look is drawn with. `Destructive`,
    /// `Active` and `Disabled` sit on the plain-background variant and
    /// take their colour from [`ButtonVariant::background_token`];
    /// `Disabled` additionally carries `InteractionDisabled`, which is
    /// what actually greys it out and blocks its `Activate`.
    pub fn feathers(&self) -> FeathersButtonVariant {
        match self {
            Self::Default | Self::Disabled => FeathersButtonVariant::Normal,
            Self::Primary => FeathersButtonVariant::Primary,
            Self::Ghost | Self::Active | Self::ActiveAlt | Self::Destructive => {
                FeathersButtonVariant::Plain
            }
        }
    }

    /// The theme token painting this look's background, or `None` when
    /// the feathers variant already paints it.
    pub fn background_token(&self, hovered: bool) -> Option<ThemeToken> {
        match self {
            Self::Default | Self::Primary | Self::Ghost | Self::Disabled => None,
            Self::Destructive => Some(if hovered {
                BUTTON_DESTRUCTIVE_BG_HOVER
            } else {
                BUTTON_DESTRUCTIVE_BG
            }),
            // Solid in both states; no hover lift, the icon colour does
            // the differentiation.
            Self::Active => Some(BUTTON_ACTIVE_BG),
            Self::ActiveAlt => Some(BUTTON_ACTIVE_ALT_BG),
        }
    }

    pub fn text_color(&self) -> Srgba {
        match self {
            Self::Default | Self::Ghost | Self::ActiveAlt => TEXT_BODY_COLOR,
            Self::Primary | Self::Destructive | Self::Active => TEXT_DISPLAY_COLOR,
            Self::Disabled => TEXT_MUTED_COLOR,
        }
    }
}

impl ButtonSize {
    pub fn width(&self) -> Val {
        match self {
            // 22px frame fits inside the 30px-tall toolbar with 4px
            // vertical breathing. Glyph at `icon_size = 16` fills
            // ~73% of the frame which reads as a solid icon rather
            // than a small mark surrounded by black void; lucide
            // glyphs only fill about two-thirds of their em-box so
            // the visible-icon ratio lands closer to the Figma 55%.
            Self::Icon => Val::Px(22.0),
            Self::IconSM => Val::Px(20.0),
            Self::MD => Val::Auto,
        }
    }
    pub fn height(&self) -> Val {
        match self {
            Self::IconSM => Val::Px(20.0),
            _ => Val::Px(22.0),
        }
    }
    pub fn padding(&self) -> Val {
        match self {
            Self::MD => px(12.0),
            Self::Icon | Self::IconSM => px(0.0),
        }
    }
    pub fn icon_size(&self) -> FontSize {
        match self {
            Self::IconSM => FontSize::Px(14.0),
            Self::Icon | Self::MD => FontSize::Px(16.0),
        }
    }

    /// How wide a left-icon slot is, for a button reserving one it does
    /// not fill. Matches [`Self::icon_size`], so a row with an icon and
    /// a row without start their captions in the same place.
    pub fn icon_slot(&self) -> f32 {
        match self {
            Self::IconSM => 14.0,
            Self::Icon | Self::MD => 16.0,
        }
    }
}

/// Everything the crate's setup pass needs to turn a freshly spawned
/// entity into a [`FeathersButton`]: which scene to apply, and the
/// children to fill the button with.
#[derive(Component)]
struct ButtonConfig {
    content: String,
    left_icon: Option<Icon>,
    left_icon_space: bool,
    left_checkbox: Option<bool>,
    right_icon: Option<Icon>,
    subtitle: Option<String>,
    call_operator: Option<Cow<'static, str>>,
    /// Apply `FeathersToolButton` rather than `FeathersButton`: the
    /// smaller frame feathers offers for icon-sized controls.
    tool: bool,
    /// Draw the leading icon in this colour instead of the variant's
    /// text colour.
    icon_color: Option<Color>,
    /// Draw the leading icon with this font instead of the crate's
    /// `IconFont` resource.
    icon_font: Option<Handle<Font>>,
    initialized: bool,
}

pub struct ButtonProps {
    pub content: String,
    pub variant: ButtonVariant,
    pub call_operator: Option<Cow<'static, str>>,
    pub size: ButtonSize,
    pub align_left: bool,
    pub left_icon: Option<Icon>,
    /// Keep the leading slot's room even with nothing in it, so a run
    /// of buttons where only some carry an icon or a box still starts
    /// every caption in the same place.
    pub left_icon_space: bool,
    /// Lead the button with a `FeathersCheckbox` in this state.
    pub left_checkbox: Option<bool>,
    pub right_icon: Option<Icon>,
    pub direction: FlexDirection,
    pub subtitle: Option<String>,
    pub border_radius: BorderRadius,
    /// Spawn with `Display::None`, for a button an appearance system
    /// reveals once the state it belongs to is reached.
    pub hidden: bool,
}

impl ButtonProps {
    pub fn new(content: impl Into<String>) -> Self {
        Self {
            content: content.into(),
            ..default()
        }
    }
    pub fn with_variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn with_size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }
    pub fn align_left(mut self) -> Self {
        self.align_left = true;
        self
    }
    pub fn with_left_icon(mut self, icon: Icon) -> Self {
        self.left_icon = Some(icon);
        self
    }
    /// Reserve the leading slot's room without putting anything in it.
    /// See [`ButtonProps::left_icon_space`].
    pub fn reserving_left_icon(mut self) -> Self {
        self.left_icon_space = true;
        self
    }
    /// Lead the button with a checkbox showing `checked`. The box is
    /// the native feathers control, made inert so the button itself
    /// takes the click and the box only reports state. See
    /// [`ButtonProps::left_checkbox`].
    pub fn with_left_checkbox(mut self, checked: bool) -> Self {
        self.left_checkbox = Some(checked);
        self
    }
    pub fn with_right_icon(mut self, icon: Icon) -> Self {
        self.right_icon = Some(icon);
        self
    }
    pub fn with_direction(mut self, direction: FlexDirection) -> Self {
        self.direction = direction;
        self
    }
    pub fn with_subtitle(mut self, subtitle: impl Into<String>) -> Self {
        self.subtitle = Some(subtitle.into());
        self
    }
    pub fn with_border_radius(mut self, radius: BorderRadius) -> Self {
        self.border_radius = radius;
        self
    }
    /// Spawn the button hidden. See [`ButtonProps::hidden`].
    pub fn hidden(mut self) -> Self {
        self.hidden = true;
        self
    }
    /// Override the button's main label. Useful in combination with
    /// `ButtonProps::from_operator::<Op>()` (defined in
    /// `jackdaw_api::ui`) when the operator's `LABEL` is too long for
    /// a tight toolbar slot, or empty when the icon alone is enough.
    pub fn with_content(mut self, content: impl Into<String>) -> Self {
        self.content = content.into();
        self
    }
    /// Dispatch an operator by id when this button is clicked. The
    /// editor provides the observer that actually calls
    /// `world.operator(id).call()`; feathers only stores the id.
    pub fn call_operator(mut self, id: impl Into<Cow<'static, str>>) -> Self {
        self.call_operator = Some(id.into());
        self
    }
}

pub struct IconButtonProps {
    pub icon: Icon,
    pub color: Option<Color>,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub alpha: Option<f32>,
}

impl IconButtonProps {
    pub fn new(icon: Icon) -> Self {
        Self {
            icon,
            color: None,
            variant: ButtonVariant::Default,
            size: ButtonSize::Icon,
            alpha: None,
        }
    }
    pub fn color(mut self, color: impl Into<Color>) -> Self {
        self.color = Some(color.into());
        self
    }
    pub fn variant(mut self, variant: ButtonVariant) -> Self {
        self.variant = variant;
        self
    }
    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = Some(alpha);
        self
    }
    pub fn with_size(mut self, size: ButtonSize) -> Self {
        self.size = size;
        self
    }
}

/// The layout the editor writes over the one [`FeathersButton`]'s own
/// scene sets: feathers sizes every button for a form row, the editor
/// needs toolbar-sized squares, left-aligned combobox shells and
/// two-line column buttons as well.
fn button_node(
    size: ButtonSize,
    align_left: bool,
    direction: FlexDirection,
    border_radius: BorderRadius,
    hidden: bool,
) -> Node {
    let is_column = direction == FlexDirection::Column;

    Node {
        display: if hidden { Display::None } else { Display::Flex },
        width: if align_left {
            percent(100)
        } else {
            size.width()
        },
        height: if is_column { Val::Auto } else { size.height() },
        padding: UiRect::axes(size.padding(), if is_column { px(6.0) } else { px(0.0) }),
        border_radius,
        flex_direction: direction,
        column_gap: px(6.0),
        row_gap: px(6.0),
        justify_content: if align_left {
            JustifyContent::Start
        } else {
            JustifyContent::Center
        },
        align_items: if is_column {
            AlignItems::Start
        } else {
            AlignItems::Center
        },
        // A left-aligned button is the combobox shape: label left,
        // chevron right. In a panel too narrow for the label, the
        // label is what gives way, and what it cannot show is cut at
        // the button's own edge rather than pushing the chevron past
        // the panel's.
        min_width: if align_left { px(0.0) } else { Val::Auto },
        overflow: if align_left {
            Overflow::clip_x()
        } else {
            Overflow::visible()
        },
        ..default()
    }
}

/// An editor button: a [`FeathersButton`] carrying the editor's variant,
/// size and content.
///
/// The returned bundle is the button's configuration, not the button
/// itself. [`FeathersButton`] is a scene component, so it can only be
/// spawned through the scene API; the crate's setup pass applies that
/// scene to the entity, writes the editor's layout back over the
/// scene's own and fills in the caption, icons and lead checkbox.
pub fn button(props: ButtonProps) -> impl Bundle {
    let ButtonProps {
        content,
        variant,
        size,
        align_left,
        left_icon,
        left_icon_space,
        left_checkbox,
        right_icon,
        direction,
        subtitle,
        call_operator,
        border_radius,
        hidden,
    } = props;

    (
        EditorButton,
        variant,
        size,
        button_node(size, align_left, direction, border_radius, hidden),
        ButtonConfig {
            content,
            left_icon,
            left_icon_space,
            left_checkbox,
            right_icon,
            subtitle,
            call_operator,
            tool: false,
            icon_color: None,
            icon_font: None,
            initialized: false,
        },
    )
}

fn setup_button(
    mut commands: Commands,
    editor_font: Res<EditorFont>,
    icon_font: Res<crate::icons::IconFont>,
    mut buttons: Query<
        (
            Entity,
            &mut ButtonConfig,
            &ButtonVariant,
            &ButtonSize,
            &mut Node,
        ),
        Added<ButtonConfig>,
    >,
) {
    let font = editor_font.0.clone();

    for (entity, mut config, variant, size, mut node) in &mut buttons {
        if config.initialized {
            continue;
        }
        config.initialized = true;

        let is_column = node.flex_direction == FlexDirection::Column;
        let icon_only = matches!(size, ButtonSize::Icon | ButtonSize::IconSM);
        // Icon-only buttons keep symmetric zero-padding so the glyph
        // sits in the dead centre of the square frame; otherwise an
        // icon child would inflate one side and shift the glyph off
        // the centre line.
        let (left_padding, right_padding) = if icon_only {
            (size.padding(), size.padding())
        } else {
            let left = if config.left_icon.is_some()
                || config.left_icon_space
                || config.left_checkbox.is_some()
                || is_column
            {
                px(6.0)
            } else {
                size.padding()
            };
            let right = if config.right_icon.is_some() || is_column {
                px(6.0)
            } else {
                size.padding()
            };
            (left, right)
        };
        node.padding = UiRect::axes(left_padding, node.padding.top);
        node.padding.right = right_padding;

        // Build the button through a queued world-exclusive closure that
        // first checks it is still alive. The lazy `with_children` spawn
        // here used to race against parent cascade-despawns: a deferred
        // `commands.entity(entity).with_children(...)` path would queue
        // child spawns with `ChildOf(entity)`, and if a despawn of the
        // button landed before these flushed, the `ChildOf` insert hook
        // would fire `add_related<ChildOf>` on a dead parent, producing
        // the `Entity despawned ... is invalid` errors on every
        // inspector rebuild. The `get_entity_mut` guard + synchronous
        // `with_children` here closes that window; everything happens
        // atomically on one `&mut World` block.
        let left_icon = config.left_icon;
        let left_icon_space = config.left_icon_space;
        let left_checkbox = config.left_checkbox;
        let right_icon = config.right_icon;
        let content = config.content.clone();
        let subtitle = config.subtitle.clone();
        let call_operator = config.call_operator.clone();
        let tool = config.tool;
        let icon_color = config.icon_color;
        let variant = *variant;
        let size = *size;
        let font = font.clone();
        let icon_font_handle = config
            .icon_font
            .clone()
            .unwrap_or_else(|| icon_font.0.clone());
        commands.queue(move |world: &mut World| {
            if world.get_entity(entity).is_err() {
                return;
            }
            apply_feathers_button(world, entity, variant, tool);
            if let Some(checked) = left_checkbox {
                spawn_inert_checkbox(world, entity, checked);
            }
            let Ok(mut ec) = world.get_entity_mut(entity) else {
                return;
            };
            if let Some(id) = call_operator {
                ec.insert(ButtonOperatorCall::new(id));
            }
            ec.with_children(|parent| {
                match left_icon {
                    Some(icon) => {
                        parent.spawn((
                            Text::new(icon.unicode()),
                            TextFont {
                                font: icon_font_handle.clone().into(),
                                font_size: size.icon_size(),
                                ..default()
                            },
                            TextColor(icon_color.unwrap_or(variant.text_color().into())),
                        ));
                    }
                    None if left_icon_space => {
                        parent.spawn(Node {
                            width: CHECKBOX_SIZE,
                            flex_shrink: 0.0,
                            ..default()
                        });
                    }
                    None => {}
                }

                // Icon-sized buttons render only the icon; the
                // operator label still reaches the user through the
                // hover tooltip. Skipping the text child here means
                // callers don't have to mirror the same intent with
                // `with_content("")`.
                let icon_only = matches!(size, ButtonSize::Icon | ButtonSize::IconSM);
                if !content.is_empty() && !icon_only {
                    // The caption sits in a slot of its own because a
                    // node cannot cut its own text: the slot takes the
                    // room left over, and a caption longer than that is
                    // cut at the slot's edge instead of running over the
                    // icon beside it.
                    parent
                        .spawn(Node {
                            flex_grow: 1.0,
                            flex_shrink: 1.0,
                            min_width: px(0.0),
                            align_items: AlignItems::Center,
                            overflow: Overflow::clip_x(),
                            ..default()
                        })
                        .with_children(|slot| {
                            slot.spawn((
                                ButtonContentText,
                                Text::new(&content),
                                TextFont {
                                    font: font.clone().into(),
                                    font_size: TEXT_SIZE,
                                    weight: FontWeight::MEDIUM,
                                    ..default()
                                },
                                TextColor(variant.text_color().into()),
                                // One line, always: a wrapped caption
                                // would grow the button's height.
                                TextLayout {
                                    linebreak: bevy::text::LineBreak::NoWrap,
                                    ..default()
                                },
                            ));
                        });
                }

                if let Some(ref subtitle) = subtitle {
                    parent.spawn((
                        Text::new(subtitle),
                        TextFont {
                            font: font.clone().into(),
                            font_size: TEXT_SIZE_SM,
                            ..default()
                        },
                        TextColor(TEXT_MUTED_COLOR.into()),
                        Node {
                            margin: UiRect::top(px(-6.0)),
                            ..default()
                        },
                    ));
                }

                if let Some(icon) = right_icon {
                    parent.spawn((
                        Text::new(icon.unicode()),
                        TextFont {
                            font: icon_font_handle.clone().into(),
                            font_size: size.icon_size(),
                            ..default()
                        },
                        TextColor(variant.text_color().into()),
                    ));
                }
            });
        });
    }
}

/// Turn `entity` into a real [`FeathersButton`], or a
/// [`FeathersToolButton`] when `tool` is set.
///
/// Both are scene components, so the scene API is the only way to spawn
/// them.
fn apply_feathers_button(world: &mut World, entity: Entity, variant: ButtonVariant, tool: bool) {
    let feathers = variant.feathers();
    // The scene writes its own form-row layout over the entity, so the
    // layout the button was spawned with is put back afterwards.
    let node = world.get::<Node>(entity).cloned();
    let applied = {
        let Ok(mut button) = world.get_entity_mut(entity) else {
            return;
        };
        if tool {
            button.apply_scene(bsn! { @FeathersToolButton { @variant: {feathers} } })
        } else {
            button.apply_scene(bsn! { @FeathersButton { @variant: {feathers} } })
        }
    };
    if let Err(error) = applied {
        error!("a button did not spawn: {error}");
        return;
    }
    let Ok(mut button) = world.get_entity_mut(entity) else {
        return;
    };
    if let Some(node) = node {
        button.insert(node);
    }
    if variant == ButtonVariant::Disabled {
        button.insert(InteractionDisabled);
    }
}

/// Put a native feathers checkbox under `entity`, showing `checked`.
///
/// The box reports state and nothing else: the row it sits in is the
/// thing being clicked, so the whole box subtree is `Pickable::IGNORE`
/// and its tab stop is dropped. `InteractionDisabled` would do the same
/// job but repaints the box in the disabled tokens, which reads as a
/// setting that cannot be changed rather than one the row changes.
pub fn spawn_inert_checkbox(world: &mut World, entity: Entity, checked: bool) {
    let box_entity = match world.spawn_scene(bsn! { @FeathersCheckbox }) {
        Ok(spawned) => spawned.id(),
        Err(error) => {
            error!("a button's leading checkbox did not spawn: {error}");
            return;
        }
    };

    let mut checkbox = world.entity_mut(box_entity);
    checkbox.insert(ChildOf(entity));
    checkbox.remove::<TabIndex>();
    if checked {
        checkbox.insert(Checked);
    }

    let mut pending = vec![box_entity];
    while let Some(next) = pending.pop() {
        if let Some(children) = world.get::<Children>(next) {
            pending.extend(children.iter());
        }
        world.entity_mut(next).insert(Pickable::IGNORE);
    }
}

/// Keep the feathers variant in step with the editor's, and paint the
/// looks feathers has no variant for.
///
/// `bevy_feathers` owns the button's colours: it writes
/// [`ThemeBackgroundColor`] from its own [`FeathersButtonVariant`] on
/// every hover, press and disable. The four editor looks it has no
/// entry for ride on top of that as a token of the editor's own, set
/// here in `Update` so it lands after the feathers pass in `PreUpdate`.
///
/// A variant flipped to `Disabled` gains `InteractionDisabled`, but
/// flipping away from it does not drop that component: operator
/// availability drives the same component from the other side, and
/// clearing it here would fight it. A button that toggles between
/// enabled and disabled should drive `InteractionDisabled` directly.
fn paint_variant_background(
    mut commands: Commands,
    changed: Query<(Entity, &ButtonVariant), (Changed<ButtonVariant>, With<EditorButton>)>,
    buttons: Query<(Entity, &ButtonVariant, &Hovered, &ThemeBackgroundColor), With<EditorButton>>,
) {
    for (entity, variant) in &changed {
        // A menu row can be despawned between this pass and the flush
        // that applies it, so every write here is fallible.
        let mut button = commands.entity(entity);
        button.try_insert(variant.feathers());
        if *variant == ButtonVariant::Disabled {
            button.try_insert(InteractionDisabled);
        }
    }

    for (entity, variant, hovered, background) in &buttons {
        let Some(token) = variant.background_token(hovered.get()) else {
            continue;
        };
        if background.0 != token {
            commands
                .entity(entity)
                .try_insert(ThemeBackgroundColor(token));
        }
    }
}

/// Bridge the widget's own activation event to [`ButtonClickEvent`],
/// which the editor's click handlers observe. `Activate` covers both the
/// pointer release over the button and Enter/Space while it holds focus,
/// and `bevy_ui_widgets` withholds it from a button carrying
/// `InteractionDisabled`.
fn fire_click_on_activate(
    activate: On<Activate>,
    buttons: Query<(), With<EditorButton>>,
    mut commands: Commands,
) {
    if buttons.contains(activate.entity) {
        commands.trigger(ButtonClickEvent {
            entity: activate.entity,
        });
    }
}

/// Create an icon-only button using lucide icon font.
///
/// This is the [`FeathersToolButton`] shape: the smaller frame feathers
/// offers for a control that is a glyph rather than a caption.
///
/// To dispatch an operator on click, spawn the returned bundle alongside an
/// [`ButtonOperatorCall`] component: `commands.spawn((icon_button(props, font),
/// ButtonOperatorCall::new("my.op")))`. A setter isn't provided on
/// [`IconButtonProps`] because `icon_button` has no staging/setup system;
/// the tuple-form keeps the API small.
// `+ use<>` on the return type opts out of Rust 2024's default
// `impl Trait` lifetime capture: the bundle clones `icon_font`
// internally, so the returned `impl Bundle` carries no borrow of the
// input handle and can be returned through wrapper functions without
// leaking lifetimes.
pub fn icon_button(props: IconButtonProps, icon_font: &Handle<Font>) -> impl Bundle + use<> {
    let IconButtonProps {
        icon,
        color,
        variant,
        size,
        alpha,
    } = props;
    let alpha = alpha.unwrap_or(1.0);
    let icon_color = color
        .unwrap_or_else(|| variant.text_color().into())
        .with_alpha(alpha);
    (
        EditorButton,
        variant,
        size,
        button_node(
            size,
            false,
            FlexDirection::Row,
            BorderRadius::all(px(BORDER_RADIUS_MD)),
            false,
        ),
        ButtonConfig {
            content: String::new(),
            left_icon: Some(icon),
            left_icon_space: false,
            left_checkbox: None,
            right_icon: None,
            subtitle: None,
            call_operator: None,
            tool: true,
            icon_color: Some(icon_color),
            icon_font: Some(icon_font.clone()),
            initialized: false,
        },
    )
}

impl Default for ButtonProps {
    fn default() -> Self {
        Self {
            content: Default::default(),
            variant: Default::default(),
            call_operator: Default::default(),
            size: Default::default(),
            align_left: Default::default(),
            left_icon: Default::default(),
            left_icon_space: Default::default(),
            left_checkbox: Default::default(),
            right_icon: Default::default(),
            direction: Default::default(),
            subtitle: Default::default(),
            border_radius: BorderRadius::all(px(BORDER_RADIUS_MD)),
            hidden: false,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::icons::{EditorFont, IconFont};

    /// An app with just enough for the button's setup pass: the scene
    /// API it applies `FeathersButton` through, and the two font
    /// resources its children read.
    fn app() -> App {
        let mut app = App::new();
        app.add_plugins((
            bevy::app::TaskPoolPlugin::default(),
            bevy::asset::AssetPlugin::default(),
            bevy::scene::ScenePlugin,
            plugin,
        ));
        app.init_asset::<bevy::text::Font>();
        app.insert_resource(IconFont(Handle::default()));
        app.insert_resource(EditorFont(Handle::default()));
        app
    }

    fn spawn_and_settle(app: &mut App, bundle: impl Bundle) -> Entity {
        let entity = app.world_mut().spawn(bundle).id();
        app.update();
        app.update();
        entity
    }

    #[test]
    fn a_button_is_a_feathers_button() {
        let mut app = app();
        let entity = spawn_and_settle(&mut app, button(ButtonProps::new("Save")));

        let button = app.world().entity(entity);
        assert!(
            button.contains::<FeathersButton>(),
            "the editor's button is the feathers one",
        );
        assert!(
            button.contains::<bevy::ui_widgets::Button>(),
            "which brings the headless widget that emits `Activate`",
        );
        assert!(
            !button.contains::<FeathersToolButton>(),
            "a captioned button is not the tool-button shape",
        );
    }

    #[test]
    fn an_icon_button_is_a_feathers_tool_button() {
        let mut app = app();
        let entity = spawn_and_settle(
            &mut app,
            icon_button(IconButtonProps::new(Icon::X), &Handle::default()),
        );

        let button = app.world().entity(entity);
        assert!(
            button.contains::<FeathersToolButton>(),
            "an icon-only button is the tool-button shape",
        );
        assert!(
            button.contains::<FeathersButton>(),
            "which the tool button is itself built on",
        );
    }

    /// The editor's looks resolve onto the three feathers carries, and
    /// the four it does not are painted from a token of the editor's own.
    #[test]
    fn every_variant_resolves_onto_a_feathers_variant() {
        assert_eq!(
            ButtonVariant::Default.feathers(),
            FeathersButtonVariant::Normal,
        );
        assert_eq!(
            ButtonVariant::Primary.feathers(),
            FeathersButtonVariant::Primary,
        );
        assert_eq!(
            ButtonVariant::Ghost.feathers(),
            FeathersButtonVariant::Plain
        );

        assert!(ButtonVariant::Default.background_token(false).is_none());
        assert!(ButtonVariant::Ghost.background_token(true).is_none());
        assert_eq!(
            ButtonVariant::Destructive.background_token(true),
            Some(BUTTON_DESTRUCTIVE_BG_HOVER),
        );
        assert_eq!(
            ButtonVariant::Active.background_token(false),
            Some(BUTTON_ACTIVE_BG),
        );
    }

    /// A disabled button carries the component feathers reads to grey it
    /// out and to withhold `Activate`.
    #[test]
    fn a_disabled_button_is_disabled_the_way_feathers_reads_it() {
        let mut app = app();
        let entity = spawn_and_settle(
            &mut app,
            button(ButtonProps::new("Save").with_variant(ButtonVariant::Disabled)),
        );

        assert!(
            app.world().entity(entity).contains::<InteractionDisabled>(),
            "the disabled look is the disabled state",
        );
    }

    /// Activating a button raises the editor's click event, which every
    /// click handler in the editor observes.
    #[test]
    fn activating_a_button_raises_the_click_event() {
        #[derive(Resource, Default)]
        struct Clicked(Vec<Entity>);

        let mut app = app();
        app.init_resource::<Clicked>();
        app.add_observer(
            |click: On<ButtonClickEvent>, mut clicked: ResMut<Clicked>| {
                clicked.0.push(click.entity);
            },
        );
        let entity = spawn_and_settle(&mut app, button(ButtonProps::new("Save")));

        app.world_mut().trigger(Activate { entity });
        app.update();

        assert_eq!(app.world().resource::<Clicked>().0, vec![entity]);
    }
}
