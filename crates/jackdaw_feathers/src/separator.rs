use bevy::prelude::*;

use crate::tokens::TEXT_BODY_COLOR;

const DEFAULT_ALPHA: f32 = 0.1;

#[derive(Component)]
pub struct EditorSeparator;

#[derive(Clone, Copy, Default)]
pub enum SeparatorDirection {
    #[default]
    Vertical,
    Horizontal,
}

#[derive(Clone, Copy, Default)]
pub struct SeparatorProps {
    pub direction: SeparatorDirection,
    pub alpha: f32,
}

impl SeparatorProps {
    pub fn vertical() -> Self {
        Self {
            direction: SeparatorDirection::Vertical,
            alpha: DEFAULT_ALPHA,
        }
    }

    pub fn horizontal() -> Self {
        Self {
            direction: SeparatorDirection::Horizontal,
            alpha: DEFAULT_ALPHA,
        }
    }

    pub fn with_alpha(mut self, alpha: f32) -> Self {
        self.alpha = alpha;
        self
    }
}

pub fn separator(props: SeparatorProps) -> impl Bundle {
    let (width, height, align_self) = match props.direction {
        // Vertical separators stretch to their parent row's height,
        // so a separator inside a 30px toolbar spans the full 30px.
        SeparatorDirection::Vertical => (px(1), Val::Auto, AlignSelf::Stretch),
        SeparatorDirection::Horizontal => (percent(100), px(1), AlignSelf::default()),
    };

    (
        EditorSeparator,
        Node {
            width,
            height,
            align_self,
            ..default()
        },
        BackgroundColor(TEXT_BODY_COLOR.with_alpha(props.alpha).into()),
    )
}

impl EditorSeparator {
    pub fn vertical() -> impl Bundle {
        separator(SeparatorProps::vertical())
    }

    pub fn horizontal() -> impl Bundle {
        separator(SeparatorProps::horizontal())
    }
}
