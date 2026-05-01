use bevy::input_focus::InputFocus;
use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use jackdaw_jsn::PropertyValue;
use lucide_icons::Icon;
use std::borrow::Cow;

use crate::icons::EditorFont;
use crate::tokens::{
    BORDER_RADIUS_MD, PRIMARY_COLOR, TEXT_BODY_COLOR, TEXT_DISPLAY_COLOR, TEXT_MUTED_COLOR,
    TEXT_SIZE, TEXT_SIZE_SM,
};

use crate::cursor::HoverCursor;

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
#[derive(Component, Clone, Debug)]
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
    app.add_systems(Update, (setup_button, handle_hover, handle_button_click));
}

#[derive(Component)]
pub struct EditorButton;

/// Marker on the text entity that holds a button's main content
/// string. External systems use this to mutate the displayed label
/// without re-spawning the button (e.g. the gizmo space toggle that
/// flips between "World" and "Local" while keeping the same button).
#[derive(Component)]
pub struct ButtonContentText;

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
    pub fn bg_color(&self, hovered: bool) -> Srgba {
        use bevy::color::palettes::tailwind;
        match self {
            Self::Default => tailwind::ZINC_700,
            Self::Ghost | Self::ActiveAlt | Self::Disabled => TEXT_BODY_COLOR,
            Self::Primary => PRIMARY_COLOR,
            // Solid surface grey (Figma toolbar #505050). Toolbar
            // active-tool indicators and combobox selected rows
            // share this treatment.
            Self::Active => Srgba::new(0.314, 0.314, 0.314, 1.0),
            Self::Destructive => {
                if hovered {
                    tailwind::RED_600
                } else {
                    tailwind::RED_500
                }
            }
        }
    }
    pub fn bg_opacity(&self, hovered: bool) -> f32 {
        match self {
            Self::Disabled => 0.0,
            Self::Ghost => {
                if hovered {
                    0.05
                } else {
                    0.0
                }
            }
            // Solid #505050 in both states; no hover lift, the icon
            // colour does the differentiation.
            Self::Active => 1.0,
            Self::ActiveAlt => 0.05,
            Self::Default => {
                if hovered {
                    0.8
                } else {
                    0.5
                }
            }
            Self::Primary | Self::Destructive => {
                if hovered {
                    0.9
                } else {
                    1.0
                }
            }
        }
    }
    pub fn text_color(&self) -> Srgba {
        match self {
            Self::Default | Self::Ghost | Self::ActiveAlt => TEXT_BODY_COLOR,
            Self::Primary | Self::Destructive | Self::Active => TEXT_DISPLAY_COLOR,
            Self::Disabled => TEXT_MUTED_COLOR,
        }
    }
    pub fn border_color(&self) -> Srgba {
        use bevy::color::palettes::tailwind;
        match self {
            Self::Default | Self::Ghost | Self::Disabled => tailwind::ZINC_700,
            Self::Primary | Self::Active => PRIMARY_COLOR,
            Self::Destructive => tailwind::RED_500,
            Self::ActiveAlt => TEXT_BODY_COLOR,
        }
    }
    pub fn border(&self) -> Val {
        match self {
            Self::Default | Self::ActiveAlt => Val::Px(1.0),
            _ => Val::Px(0.0),
        }
    }
    pub fn border_opacity(&self, hovered: bool) -> f32 {
        match self {
            Self::Ghost => {
                if hovered {
                    1.0
                } else {
                    0.0
                }
            }
            Self::Disabled => 0.0,
            Self::ActiveAlt => 0.2,
            _ => 1.0,
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
    pub fn icon_size(&self) -> f32 {
        match self {
            Self::IconSM => 14.0,
            Self::Icon | Self::MD => 16.0,
        }
    }
}

#[derive(Component)]
struct ButtonConfig {
    content: String,
    left_icon: Option<Icon>,
    right_icon: Option<Icon>,
    subtitle: Option<String>,
    call_operator: Option<Cow<'static, str>>,
    initialized: bool,
}

pub struct ButtonProps {
    pub content: String,
    pub variant: ButtonVariant,
    pub call_operator: Option<Cow<'static, str>>,
    pub size: ButtonSize,
    pub align_left: bool,
    pub left_icon: Option<Icon>,
    pub right_icon: Option<Icon>,
    pub direction: FlexDirection,
    pub subtitle: Option<String>,
    pub border_radius: BorderRadius,
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
    pub color: Option<Srgba>,
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
    pub fn color(mut self, color: Srgba) -> Self {
        self.color = Some(color);
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

pub(crate) fn button_base(
    variant: ButtonVariant,
    size: ButtonSize,
    align_left: bool,
    direction: FlexDirection,
    border_radius: BorderRadius,
) -> impl Bundle {
    let is_column = direction == FlexDirection::Column;

    (
        Button,
        EditorButton,
        variant,
        size,
        Hovered::default(),
        HoverCursor(bevy::window::SystemCursorIcon::Pointer),
        Node {
            width: if align_left {
                percent(100)
            } else {
                size.width()
            },
            height: if is_column { Val::Auto } else { size.height() },
            padding: UiRect::axes(size.padding(), if is_column { px(6.0) } else { px(0.0) }),
            border: UiRect::all(variant.border()),
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
            ..default()
        },
        BackgroundColor(
            variant
                .bg_color(false)
                .with_alpha(variant.bg_opacity(false))
                .into(),
        ),
        BorderColor::all(
            variant
                .border_color()
                .with_alpha(variant.border_opacity(false)),
        ),
    )
}

pub fn button(props: ButtonProps) -> impl Bundle {
    let ButtonProps {
        content,
        variant,
        size,
        align_left,
        left_icon,
        right_icon,
        direction,
        subtitle,
        call_operator,
        border_radius,
    } = props;

    (
        button_base(variant, size, align_left, direction, border_radius),
        ButtonConfig {
            content,
            left_icon,
            right_icon,
            subtitle,
            call_operator,
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
            let left = if config.left_icon.is_some() || is_column {
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

        // Spawn the button's text/icon children through a queued
        // world-exclusive closure that first checks the button is
        // still alive. The lazy `with_children` spawn here used to
        // race against parent cascade-despawns: a deferred
        // `commands.entity(entity).with_children(...)` path would
        // queue child spawns with `ChildOf(entity)`, and if a
        // despawn of the button landed before these flushed, the
        // `ChildOf` insert hook would fire `add_related<ChildOf>`
        // on a dead parent, producing the
        // `Entity despawned ... is invalid` errors on every inspector
        // rebuild. The `get_entity_mut` guard + synchronous
        // `with_children` here closes that window; everything
        // happens atomically on one `&mut World` block.
        let left_icon = config.left_icon;
        let right_icon = config.right_icon;
        let content = config.content.clone();
        let subtitle = config.subtitle.clone();
        let call_operator = config.call_operator.clone();
        let variant = *variant;
        let size = *size;
        let font = font.clone();
        let icon_font_handle = icon_font.0.clone();
        commands.queue(move |world: &mut World| {
            let Ok(mut ec) = world.get_entity_mut(entity) else {
                return;
            };
            if let Some(id) = call_operator {
                ec.insert(ButtonOperatorCall::new(id));
            }
            ec.with_children(|parent| {
                if let Some(icon) = left_icon {
                    parent.spawn((
                        Text::new(icon.unicode()),
                        TextFont {
                            font: icon_font_handle.clone(),
                            font_size: size.icon_size(),
                            ..default()
                        },
                        TextColor(variant.text_color().into()),
                    ));
                }

                // Icon-sized buttons render only the icon; the
                // operator label still reaches the user through the
                // hover tooltip. Skipping the text child here means
                // callers don't have to mirror the same intent with
                // `with_content("")`.
                let icon_only = matches!(size, ButtonSize::Icon | ButtonSize::IconSM);
                if !content.is_empty() && !icon_only {
                    parent.spawn((
                        ButtonContentText,
                        Text::new(&content),
                        TextFont {
                            font: font.clone(),
                            font_size: TEXT_SIZE,
                            weight: FontWeight::MEDIUM,
                            ..default()
                        },
                        TextColor(variant.text_color().into()),
                        Node {
                            flex_grow: 1.0,
                            ..default()
                        },
                    ));
                }

                if let Some(ref subtitle) = subtitle {
                    parent.spawn((
                        Text::new(subtitle),
                        TextFont {
                            font: font.clone(),
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
                            font: icon_font_handle.clone(),
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

fn handle_hover(
    // Re-render when either the hover state OR the variant
    // changed. Without `Changed<ButtonVariant>` a toolbar button
    // whose variant flips Active <-> Ghost (driven by an external
    // system, e.g. `update_toolbar_button_variants`) only picks
    // up the new bg the next time the cursor crosses it; the
    // user sees stale highlights.
    changed: Query<(), Or<(Changed<Hovered>, Changed<ButtonVariant>)>>,
    mut buttons: Query<
        (
            Entity,
            &ButtonVariant,
            &Hovered,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        With<EditorButton>,
    >,
    focus: Res<InputFocus>,
) {
    let focus_changed = focus.is_changed();
    for (entity, variant, hovered, mut bg, mut border) in &mut buttons {
        if !(focus_changed || changed.get(entity).is_ok()) {
            continue;
        };

        let is_hovered = hovered.get() || focus.0 == Some(entity);
        bg.0 = variant
            .bg_color(is_hovered)
            .with_alpha(variant.bg_opacity(is_hovered))
            .into();
        *border = BorderColor::all(
            variant
                .border_color()
                .with_alpha(variant.border_opacity(is_hovered)),
        );
    }
}

fn handle_button_click(
    interactions: Query<
        (Entity, &Interaction, &ButtonVariant),
        (Changed<Interaction>, With<EditorButton>),
    >,
    mut commands: Commands,
) {
    for (entity, interaction, variant) in &interactions {
        if *interaction == Interaction::Pressed && *variant != ButtonVariant::Disabled {
            commands.trigger(ButtonClickEvent { entity });
        }
    }
}

/// Create an icon-only button using lucide icon font.
///
/// To dispatch an operator on click, spawn the returned bundle alongside an
/// [`ButtonOperatorCall`] component: `commands.spawn((icon_button(props, font),
/// ButtonOperatorCall::new("my.op")))`. A setter isn't provided on
/// [`IconButtonProps`] because `icon_button` has no staging/setup system;
/// the tuple-form keeps the API small.
// `+ use<>` on the return type opts out of Rust 2024's default
// `impl Trait` lifetime capture: the bundle clones `icon_font`
// internally (see `font: icon_font.clone()` in the body), so the
// returned `impl Bundle` carries no borrow of the input handle and
// can be returned through wrapper functions without leaking
// lifetimes.
pub fn icon_button(props: IconButtonProps, icon_font: &Handle<Font>) -> impl Bundle + use<> {
    let IconButtonProps {
        icon,
        color,
        variant,
        size,
        alpha,
    } = props;
    let alpha = alpha.unwrap_or(1.0);
    let icon_color = color.unwrap_or(variant.text_color()).with_alpha(alpha);

    (
        button_base(
            variant,
            size,
            false,
            FlexDirection::Row,
            BorderRadius::all(px(BORDER_RADIUS_MD)),
        ),
        children![(
            Text::new(icon.unicode()),
            TextFont {
                font: icon_font.clone(),
                font_size: size.icon_size(),
                ..default()
            },
            TextColor(Color::Srgba(icon_color)),
        )],
    )
}

pub fn set_button_variant(
    variant: ButtonVariant,
    bg: &mut BackgroundColor,
    border: &mut BorderColor,
) {
    bg.0 = variant
        .bg_color(false)
        .with_alpha(variant.bg_opacity(false))
        .into();
    *border = BorderColor::all(
        variant
            .border_color()
            .with_alpha(variant.border_opacity(false)),
    );
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
            right_icon: Default::default(),
            direction: Default::default(),
            subtitle: Default::default(),
            border_radius: BorderRadius::all(px(BORDER_RADIUS_MD)),
        }
    }
}
