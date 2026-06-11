# Play-in-editor

Play-in-editor (PIE) runs your game as a real process and streams its
frames into the editor, so you can play, inspect, and live-edit a running
build without leaving jackdaw. The game runs in its own process, so a
crash takes down the game, not the editor.

PIE keeps two surfaces strictly separate:

- The **Game panel** is a pure monitor of the running game's frame.
- The **Viewport** is always an editing surface for the authored scene.
  It never composites the game frame over your scene.

## What you need

PIE launches your game from its **run configurations**. A project that
plays in the editor has at least one runnable config in its `jackdaw.toml`,
and the config that streams into the editor builds with the `pie` feature.
See [Scene Management](scene-management.md) for where project config lives,
and your game crate's `jackdaw.toml` for the configs themselves.

How many configs you define is up to your game. Some games run a single
process; others split into several that you launch together. PIE treats
each launched process as an instance and streams the focused one.

Open the **Game** panel before you start. It docks in the bottom dock area
next to Assets.

## Launching

The play controls header carries **Play**, **Pause**, **Stop**, and
**Reload**, plus a window-mode button that reads **Embedded** or
**Windowed**. The window-mode button sets the mode for the next launch.

- **Embedded** (default): no separate game window opens. The game renders
  off-screen and streams into the Game panel at full frame rate. This is
  the mode you want for input capture and picking.
- **Windowed**: the game opens its own OS window. The Game panel still
  mirrors the active in-game camera once one exists, but menus do not
  stream and input capture is not offered.

Hit **Play** to launch. The Game panel starts streaming immediately,
beginning with whatever the game shows first (often a menu or title
screen). The outliner shows a **LIVE** badge with the running instance's
name.

When more than one instance is running, the **instance picker** in the
outliner header switches which one the Game panel and Live tree follow.

## Playing the game

The Game panel header has a **Play | Select** mode bar.

In **Play** mode, click inside the panel to engage input capture (or use
the **Play Input** header button). While captured:

- Keyboard and mouse forward to the game. WASD, mouse-look, scroll, clicks,
  and typing all reach it.
- Editor keybinds are suppressed. Tool keys and `Ctrl+S` go to the game,
  not the editor.
- A **Playing, Shift+Esc to release** chip shows, and the panel border
  takes the capture accent.

Plain `Esc` forwards to the game (so the in-game menu still opens). Press
`Shift+Esc` to release capture and return control to the editor. Capture
also releases on its own if you stop the game, switch instances, click away
to another application, or close the panel, and any keys you were holding
are released so nothing stays stuck down.

## Selecting entities from the frame

Switch the mode bar to **Select**. Game input stops, and the cursor becomes
a picker over the streamed frame.

- Click an object in the frame to select it. The selection appears in the
  outliner's **Live** tab and the inspector shows its live values.
- Picking reads the real frame through the game's own camera, so it needs
  no alignment and reaches runtime-only entities (the player character,
  spawned props) that have no authored counterpart.
- The game draws a bounding box around the picked entity, and the Live tree
  expands to reveal the selected row.
- Selecting a row in the Live tree moves the box to that entity.

Menu and UI elements are not pickable; they are not streamable scene
entities.

## Scene and Live trees

The outliner header has a **Scene | Live** tab switch:

- **Scene** shows the authored tree of the open scene file. This is the
  same hierarchy you edit when the game is not running.
- **Live** shows the entities the focused game instance currently has,
  including runtime-only ones. Authored entities the game has not spawned
  do not appear here.

The two trees are independent. When the game shows a menu, the Live tab
shows the menu's entities while the Scene tab still shows your authored
scene, and the Viewport keeps showing that scene with gizmos, fully
editable. You can select and edit authored entities in the Viewport or
Scene tab at any time without disturbing the running game's frame.

## Stopping and reloading

- **Stop** ends the game process. The Game panel returns to its idle state
  and the **LIVE** badge clears.
- **Reload** relaunches with the current window-mode setting, which is how
  you apply a change to the **Embedded** / **Windowed** button.
