//! Small scenes the launcher pages are built from: text, chips, operator
//! buttons and the section frames.

use bevy::app::PropagateOver;
use bevy::feathers::controls::{ButtonVariant, FeathersButton, FeathersCheckbox, FeathersRadio};
use bevy::feathers::theme::ThemedText;
use bevy::prelude::*;
use bevy::text::{FontSize, FontSourceTemplate};
use bevy::ui::{Checked, InteractionDisabled};
use bevy::ui_widgets::popover::{Popover, PopoverAlign, PopoverPlacement, PopoverSide};
use jackdaw_feathers::button::ButtonOperatorCall;
use jackdaw_feathers::icons::{Icon, font_paths};
use jackdaw_feathers::tokens;

/// The operator a checkbox, radio or toggle switch runs when it changes,
/// with `enabled` set to its new value.
#[derive(Component, Clone, Debug, Default)]
pub struct ToggleOperatorCall(pub ButtonOperatorCall);

/// How a chip is coloured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Tone {
    Neutral,
    Success,
    Warning,
    Caution,
    Accent,
}

impl Tone {
    fn colors(self) -> (Color, Color) {
        match self {
            Self::Neutral => (tokens::CHIP_BG, tokens::TEXT_SECONDARY),
            Self::Success => (tokens::CHIP_SUCCESS_BG, tokens::CHIP_SUCCESS_TEXT),
            Self::Warning => (tokens::CHIP_WARNING_BG, tokens::CHIP_WARNING_TEXT),
            Self::Caution => (tokens::CHIP_CAUTION_BG, tokens::CHIP_CAUTION_TEXT),
            Self::Accent => (tokens::ACCENT_BLUE, tokens::TEXT_PRIMARY),
        }
    }
}

/// A line of text.
pub fn text(content: impl Into<String>, size: FontSize, color: Color) -> impl Scene {
    let content = content.into();
    bsn! {
        Text(content)
        TextFont { font_size: {size} }
        TextColor(color)
        Pickable::IGNORE
    }
}

/// Body text in the primary colour.
pub fn primary(content: impl Into<String>) -> impl Scene {
    text(content, tokens::TEXT_SIZE, tokens::TEXT_PRIMARY)
}

/// Body text in the secondary colour.
pub fn secondary(content: impl Into<String>) -> impl Scene {
    text(content, tokens::TEXT_SIZE, tokens::TEXT_SECONDARY)
}

/// Small text in the dim colour.
pub fn dim(content: impl Into<String>) -> impl Scene {
    text(
        content,
        tokens::TEXT_SIZE_SM,
        tokens::DOC_TAB_INACTIVE_LABEL,
    )
}

/// Small text that wraps inside its parent.
pub fn note(content: impl Into<String>, color: Color) -> impl Scene {
    let content = content.into();
    bsn! {
        Node { max_width: percent(100) }
        Text(content)
        TextFont { font_size: tokens::TEXT_SIZE_SM }
        TextColor(color)
        Pickable::IGNORE
    }
}

/// A path, in the dim colour, on one line clipped to the space it gets.
pub fn path_text(content: impl Into<String>) -> impl Scene {
    let content = content.into();
    bsn! {
        Node { overflow: Overflow::clip(), min_width: px(0), flex_shrink: 1.0 }
        Pickable::IGNORE
        Children [
            Text(content)
            TextFont { font_size: tokens::TEXT_SIZE_SM }
            TextColor(tokens::DOC_TAB_INACTIVE_LABEL)
            TextLayout { linebreak: LineBreak::NoWrap }
            Pickable::IGNORE
        ]
    }
}

/// A Lucide glyph inside a feathers button, kept out of the font the
/// button hands its captions.
pub fn button_glyph(glyph: Icon) -> impl Scene {
    let glyph = String::from(glyph.unicode());
    bsn! {
        Text(glyph)
        TextFont {
            font: FontSourceTemplate::Handle(font_paths::LUCIDE),
            font_size: FontSize::Px(tokens::ICON_SM_PX),
        }
        ~{PropagateOver::<TextFont>::default()}
        ThemedText
    }
}

/// A Lucide icon glyph.
pub fn icon(glyph: Icon, size: f32, color: Color) -> impl Scene {
    let glyph = String::from(glyph.unicode());
    bsn! {
        Text(glyph)
        TextFont {
            font: FontSourceTemplate::Handle(font_paths::LUCIDE),
            font_size: FontSize::Px(size),
        }
        TextColor(color)
        Pickable::IGNORE
    }
}

/// A small rounded label.
pub fn chip(content: impl Into<String>, tone: Tone) -> impl Scene {
    let content = content.into();
    let (background, color) = tone.colors();
    bsn! {
        Node {
            height: px(20),
            padding: UiRect::horizontal(px(6)),
            align_items: AlignItems::Center,
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(background)
        Pickable::IGNORE
        Children [
            Text(content)
            TextFont { font_size: tokens::TEXT_SIZE_SM }
            TextColor(color)
            TextLayout { linebreak: LineBreak::NoWrap }
            Pickable::IGNORE
        ]
    }
}

/// A button that runs `call` when clicked.
pub fn button(
    caption: impl Into<String>,
    call: ButtonOperatorCall,
    variant: ButtonVariant,
) -> impl Scene {
    let caption = caption.into();
    bsn! {
        @FeathersButton {
            @caption: bsn! { Text(caption) ThemedText },
            @variant: {variant},
        }
        ~{call}
    }
}

/// A button with a leading icon.
pub fn icon_button(
    glyph: Icon,
    caption: impl Into<String>,
    call: ButtonOperatorCall,
    variant: ButtonVariant,
) -> impl Scene {
    let caption = caption.into();
    let has_caption = !caption.is_empty();
    bsn! {
        @FeathersButton {
            @caption: bsn_list! {
                @{button_glyph(glyph)}
                --
                Node {
                    display: {if has_caption { Display::Flex } else { Display::None }},
                    margin: UiRect::left(px(6)),
                }
                Text(caption)
                ThemedText
            },
            @variant: {variant},
        }
        ~{call}
    }
}

/// A button showing `caption` followed by a chevron, for a control that
/// opens a choice.
pub fn picker_button(caption: impl Into<String>, call: ButtonOperatorCall) -> impl Scene {
    let caption = caption.into();
    bsn! {
        @FeathersButton {
            @caption: bsn_list! {
                Node { flex_grow: 1.0, min_width: px(0), overflow: Overflow::clip() }
                Children [
                    Text(caption)
                    TextFont { font_size: tokens::TEXT_SIZE }
                    TextLayout { linebreak: LineBreak::NoWrap }
                    ~{PropagateOver::<TextFont>::default()}
                    ThemedText
                ]
                --
                Node { margin: UiRect::left(px(8)) }
                Children [ @{button_glyph(Icon::ChevronDown)} ]
            },
        }
        Node { justify_content: JustifyContent::SpaceBetween }
        ~{call}
    }
}

/// A row of filter buttons, the chosen one highlighted.
pub fn filter_button(
    caption: &'static str,
    selected: bool,
    call: ButtonOperatorCall,
) -> impl Scene {
    let variant = if selected {
        ButtonVariant::Primary
    } else {
        ButtonVariant::Normal
    };
    button(caption, call, variant)
}

/// A checkbox that runs `call` with `enabled` when changed.
pub fn checkbox(
    caption: impl Into<String>,
    checked: bool,
    disabled: bool,
    call: ButtonOperatorCall,
) -> impl Scene {
    let caption = caption.into();
    bsn! {
        @FeathersCheckbox {
            @caption: bsn! { Text(caption) ThemedText },
        }
        ~{ToggleOperatorCall(call)}
        @{toggle_state(checked, disabled)}
    }
}

/// `Checked` and `InteractionDisabled` as a control starts with them.
pub fn toggle_state(checked: bool, disabled: bool) -> impl Scene {
    (
        checked.then_some(bsn! { Checked }),
        disabled.then_some(bsn! { InteractionDisabled }),
    )
}

/// A radio button that runs `call` when chosen.
pub fn radio(checked: bool, call: ButtonOperatorCall) -> impl Scene {
    bsn! {
        @FeathersRadio
        ~{ToggleOperatorCall(call)}
        @{toggle_state(checked, false)}
    }
}

/// A rounded section with a header line and its content under it.
pub fn section(header: impl SceneList, body: impl SceneList) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            width: percent(100),
            flex_shrink: 0.0,
            border_radius: BorderRadius::all(px(4)),
        }
        BackgroundColor(tokens::ELEVATED_BG)
        Children [
            Node {
                min_height: px(30),
                padding: UiRect::horizontal(px(10)),
                align_items: AlignItems::Center,
                column_gap: px(8),
                flex_wrap: FlexWrap::Wrap,
            }
            Children [ {header} ]
            --
            Node {
                flex_direction: FlexDirection::Column,
                row_gap: px(6),
                padding: UiRect::all(px(6)),
                margin: UiRect::new(px(1), px(1), px(0), px(1)),
                border_radius: BorderRadius::bottom(px(4)),
            }
            BackgroundColor(tokens::PANEL_BG)
            Children [ {body} ]
        ]
    }
}

/// A flexible space that pushes what follows to the end of a row.
pub fn spacer() -> impl Scene {
    bsn! {
        Node { flex_grow: 1.0 }
        Pickable::IGNORE
    }
}

/// A row of children with a gap.
pub fn row(gap: f32, children: impl SceneList) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::Center,
            column_gap: px(gap),
            min_width: px(0),
        }
        Children [ {children} ]
    }
}

/// A column of children with a gap.
pub fn column(gap: f32, children: impl SceneList) -> impl Scene {
    bsn! {
        Node {
            flex_direction: FlexDirection::Column,
            row_gap: px(gap),
            min_width: px(0),
        }
        Children [ {children} ]
    }
}

/// Placement for a menu opened from the right edge of a row: under or
/// over its button, lined up with the button's right edge so it stays in
/// the window.
pub fn end_aligned_popover() -> Popover {
    let placement = |side| PopoverPlacement {
        side,
        align: PopoverAlign::End,
        gap: 2.0,
    };
    Popover {
        positions: vec![placement(PopoverSide::Bottom), placement(PopoverSide::Top)],
        window_margin: 10.0,
    }
}

/// A list of boxed scenes, for children whose kinds differ.
pub type Scenes = Vec<Box<dyn SceneList>>;

/// Box a scene so lists of different scenes can be built.
pub fn boxed(scene: impl Scene) -> Box<dyn SceneList> {
    Box::new(bsn_list! { @{ scene } })
}
