# Viewport navigation

The viewport uses a fly-camera scheme that should feel
familiar if you have used Hammer, TrenchBroom, or Unreal's
perspective view. Right-mouse-button to look, WASD to move.

The full key list lives in
[Keyboard Shortcuts](keyboard-shortcuts.md); this page is the
plain-English version.

## Look and move

Hold the right mouse button to enter look mode. While held:

- `W` / `A` / `S` / `D` move along the view direction.
- `Q` / `E` move down and up in world space.
- `Shift` doubles speed.
- The mouse wheel adjusts speed live, so you can scroll up
  while flying around to cover a level quickly.

Releasing RMB drops you back into normal cursor mode.

## Dolly without entering look mode

If you don't want to lift your hand, scrolling without RMB
held dollies the camera forward and back along the look axis.
Useful for small framing tweaks while a tool is active.

## Focus selection

Press `F` with one or more entities selected to recenter the
camera on the selection bounds. The camera keeps its current
yaw and pitch; only translation changes. Good for when you
have flown off into the void and need to come back.

## Camera bookmarks

The viewport has nine bookmark slots:

- `Ctrl+1` through `Ctrl+9` saves the current camera pose to
  a slot.
- `1` through `9` restores it.

Bookmarks are session-only right now. They live in an
in-memory `CameraBookmarks` resource and reset on editor
restart. Persisting them into the project file is on the
list; not done yet.

## View modes and the grid

- `Ctrl+Shift+W` toggles wireframe.
- `[` and `]` step the grid size down and up. Numbers print
  in the status bar.
- `Ctrl+Alt+Scroll` is the same step, mouse-driven.

The grid size also drives the snap distance for translate
operations, so changing it doesn't just affect the visuals.

## Mouse look feels off

If the viewport rotates faster or slower than you expect,
that is the `bevy_enhanced_input` mouse sensitivity, not a
jackdaw setting. We don't expose it in the UI yet (see
[Open Challenges](../developer-guide/open-challenges.md));
file an issue if it's blocking you and we'll surface it.
