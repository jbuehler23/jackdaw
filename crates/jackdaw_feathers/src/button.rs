use bevy::picking::hover::Hovered;
use bevy::prelude::*;
use lucide_icons::Icon;
use std::borrow::Cow;

use crate::icons::EditorFont;
use crate::tokens::{
    CORNER_RADIUS_LG, PRIMARY_COLOR, TEXT_BODY_COLOR, TEXT_DISPLAY_COLOR, TEXT_MUTED_COLOR,
    TEXT_SIZE, TEXT_SIZE_SM,
};

use crate::cursor::HoverCursor;

#[derive(EntityEvent)]
pub struct ButtonClickEvent {
    pub entity: Entity,
}

/// Attached to a button to declare that clicking it should dispatch
/// the operator with this id. The editor registers the dispatch
/// observer; feathers just carries the id so widgets can declare
/// their intent without depending on the operator API.
#[derive(Component, Clone, Debug)]
pub struct CallOperator(pub Cow<'static, str>);

impl CallOperator {
    pub fn new(id: impl Into<Cow<'static, str>>) -> Self {
        Self(id.into())
    }
}

pub fn plugin(app: &mut App) {
    app.add_systems(Update, (setup_button, handle_hover, handle_button_click));
}

#[derive(Component)]
pub struct EditorButton;

#[derive(Component, Default, Clone, Copy, PartialEq)]
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
        match (self, hovered) {
            (Self::Default, _) => tailwind::ZINC_700,
            (Self::Ghost | Self::ActiveAlt | Self::Disabled, _) => TEXT_BODY_COLOR,
            (Self::Primary | Self::Active, _) => PRIMARY_COLOR,
            (Self::Destructive, false) => tailwind::RED_500,
            (Self::Destructive, true) => tailwind::RED_600,
        }
    }
    pub fn bg_opacity(&self, hovered: bool) -> f32 {
        #[expect(
            clippy::match_same_arms,
            reason = "We want to tweak the values that happen to be the same differently"
        )]
        match (self, hovered) {
            (Self::Ghost, false) | (Self::Disabled, _) => 0.0,
            (Self::Active, false) => 0.1,
            (Self::Active, true) => 0.15,
            (Self::ActiveAlt, _) => 0.05,
            (Self::Default, false) => 0.5,
            (Self::Default, true) => 0.8,
            (Self::Ghost, true) => 0.05,
            (Self::Primary | Self::Destructive, false) => 1.0,
            (Self::Primary | Self::Destructive, true) => 0.9,
        }
    }
    pub fn text_color(&self) -> Srgba {
        match self {
            Self::Default | Self::Ghost | Self::ActiveAlt => TEXT_BODY_COLOR,
            Self::Primary | Self::Destructive => TEXT_DISPLAY_COLOR,
            Self::Active => PRIMARY_COLOR.lighter(0.05),
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
        match (self, hovered) {
            (Self::Ghost, false) | (Self::Disabled, _) => 0.0,
            (Self::ActiveAlt, _) => 0.2,
            _ => 1.0,
        }
    }
}

impl ButtonSize {
    fn width(&self) -> Val {
        match self {
            Self::Icon => Val::Px(28.0),
            Self::IconSM => Val::Px(24.0),
            Self::MD => Val::Auto,
        }
    }
    fn height(&self) -> Val {
        match self {
            Self::IconSM => Val::Px(24.0),
            _ => Val::Px(28.0),
        }
    }
    fn padding(&self) -> Val {
        match self {
            Self::MD => px(12.0),
            Self::Icon | Self::IconSM => px(0.0),
        }
    }
    fn icon_size(&self) -> f32 {
        match self {
            Self::IconSM => 14.0,
            _ => 16.0,
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

#[derive(Default)]
pub struct ButtonProps {
    pub content: String,
    pub variant: ButtonVariant,
    pub size: ButtonSize,
    pub align_left: bool,
    pub left_icon: Option<Icon>,
    pub right_icon: Option<Icon>,
    pub direction: FlexDirection,
    pub subtitle: Option<String>,
    pub call_operator: Option<Cow<'static, str>>,
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
            border_radius: BorderRadius::all(CORNER_RADIUS_LG),
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
    } = props;

    (
        button_base(variant, size, align_left, direction),
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
        let left_padding = if config.left_icon.is_some() || is_column {
            px(6.0)
        } else {
            size.padding()
        };
        let right_padding = if config.right_icon.is_some() || is_column {
            px(6.0)
        } else {
            size.padding()
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
        // `Entity despawned … is invalid` errors on every inspector
        // rebuild. The `get_entity_mut` guard + synchronous
        // `with_children` here closes that window — everything
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
                ec.insert(CallOperator(id));
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

                if !content.is_empty() {
                    parent.spawn((
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
    mut buttons: Query<
        (
            &ButtonVariant,
            &Hovered,
            &mut BackgroundColor,
            &mut BorderColor,
        ),
        (Changed<Hovered>, With<EditorButton>),
    >,
) {
    for (variant, hovered, mut bg, mut border) in &mut buttons {
        let is_hovered = hovered.get();
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
/// [`CallOperator`] component: `commands.spawn((icon_button(props, font),
/// CallOperator::new("my.op")))`. A setter isn't provided on
/// [`IconButtonProps`] because `icon_button` has no staging/setup system;
/// the tuple-form keeps the API small.
pub fn icon_button(props: IconButtonProps, icon_font: &Handle<Font>) -> impl Bundle {
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
        button_base(variant, size, false, FlexDirection::Row),
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
