//! Which modifier keyboard shortcuts are built on, and what the modifiers
//! are called, on the platform the editor runs on.

use bevy::prelude::*;

/// The desktop convention a shortcut follows.
///
/// macOS builds shortcuts on Cmd (the Super key) and leaves Ctrl to the
/// pointer, everything else builds them on Ctrl. Code that needs the host's
/// answer calls [`ShortcutPlatform::host`]; taking the platform as a value
/// lets either convention be exercised from any build.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShortcutPlatform {
    /// Cmd is the primary modifier.
    MacOs,
    /// Ctrl is the primary modifier.
    Standard,
}

impl ShortcutPlatform {
    /// The platform this build targets.
    pub const fn host() -> Self {
        if cfg!(target_os = "macos") {
            Self::MacOs
        } else {
            Self::Standard
        }
    }

    /// Whether the primary modifier is Super (Cmd) rather than Ctrl.
    pub const fn primary_is_super(self) -> bool {
        matches!(self, Self::MacOs)
    }

    /// The left and right keys of the primary modifier.
    pub const fn primary_keys(self) -> [KeyCode; 2] {
        if self.primary_is_super() {
            [KeyCode::SuperLeft, KeyCode::SuperRight]
        } else {
            [KeyCode::ControlLeft, KeyCode::ControlRight]
        }
    }

    /// Whether either key of the primary modifier is held.
    pub fn primary_pressed(self, keyboard: &ButtonInput<KeyCode>) -> bool {
        keyboard.any_pressed(self.primary_keys())
    }

    /// The primary modifier's name in shortcut text.
    pub const fn primary_label(self) -> &'static str {
        if self.primary_is_super() {
            self.super_label()
        } else {
            "Ctrl"
        }
    }

    /// The Super key's name in shortcut text.
    pub const fn super_label(self) -> &'static str {
        match self {
            Self::MacOs => "Cmd",
            Self::Standard => "Super",
        }
    }

    /// The names of the held modifiers, in the order shortcut text lists them.
    pub fn modifier_labels(
        self,
        ctrl: bool,
        shift: bool,
        alt: bool,
        super_: bool,
    ) -> Vec<&'static str> {
        [
            (ctrl, "Ctrl"),
            (shift, "Shift"),
            (alt, "Alt"),
            (super_, self.super_label()),
        ]
        .into_iter()
        .filter_map(|(held, name)| held.then_some(name))
        .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn macos_shortcuts_are_built_on_cmd() {
        let mac = ShortcutPlatform::MacOs;
        assert!(mac.primary_is_super());
        assert_eq!(
            mac.primary_keys(),
            [KeyCode::SuperLeft, KeyCode::SuperRight]
        );
        assert_eq!(mac.primary_label(), "Cmd");
    }

    #[test]
    fn other_shortcuts_are_built_on_ctrl() {
        let standard = ShortcutPlatform::Standard;
        assert!(!standard.primary_is_super());
        assert_eq!(
            standard.primary_keys(),
            [KeyCode::ControlLeft, KeyCode::ControlRight]
        );
        assert_eq!(standard.primary_label(), "Ctrl");
    }

    #[test]
    fn ctrl_keeps_its_name_on_macos() {
        assert_eq!(
            ShortcutPlatform::MacOs.modifier_labels(true, true, false, true),
            ["Ctrl", "Shift", "Cmd"]
        );
        assert_eq!(
            ShortcutPlatform::Standard.modifier_labels(true, false, true, true),
            ["Ctrl", "Alt", "Super"]
        );
    }

    #[test]
    fn primary_pressed_reads_only_the_primary_keys() {
        let mut keyboard = ButtonInput::<KeyCode>::default();
        keyboard.press(KeyCode::ControlLeft);
        assert!(ShortcutPlatform::Standard.primary_pressed(&keyboard));
        assert!(!ShortcutPlatform::MacOs.primary_pressed(&keyboard));
        keyboard.release(KeyCode::ControlLeft);
        keyboard.press(KeyCode::SuperRight);
        assert!(ShortcutPlatform::MacOs.primary_pressed(&keyboard));
        assert!(!ShortcutPlatform::Standard.primary_pressed(&keyboard));
    }
}
