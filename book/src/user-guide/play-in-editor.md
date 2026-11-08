# Play-in-editor

Play-in-editor (PIE) runs your game as a real process and streams its
frames into the editor, so you can play a running build and edit its scene
without leaving jackdaw. The game runs in its own process, so a
crash takes down the game, not the editor.

PIE keeps two surfaces strictly separate:

- The **Game panel** is a pure monitor of the running game's frame.
- The **Viewport** is always an editing surface for the authored scene.
  It never composites the game frame over your scene.

## What you need

Nothing compiles at play time once the project is built. The editor
builds your game as an ordinary cargo binary when the project opens;
Play launches that same executable and connects to it over IPC.

That build goes to `.jackdaw/target/game` inside your project, never to
the project's own `target/`. The editor builds the plain package, while
the build you run from your terminal usually carries features of your
own; keeping them apart means neither overwrites the other's binary, at
the cost of compiling the dependencies once more. `.jackdaw/` is
gitignored, so none of it is committed; a project that does not ignore it
should.

The editor compiles your dependencies optimised (`opt-level = 2`, line
tables only) and your own crate as your profile says, because a game
spends its load and its frames in Bevy and the runtime. A project that sets
a dev `opt-level` for itself or its dependencies keeps that setting, whether
in its manifest, a `.cargo/config.toml` in its folder or a parent, cargo's
home, or `CARGO_PROFILE_DEV_OPT_LEVEL`. So the first build compiles every
dependency once, which takes several minutes, and later builds only compile
your crate. A plain `cargo run` follows your manifest; new projects start
with the same profile in theirs.

To opt out, set `opt-level` under `[profile.dev.package."*"]`: `0` builds
dependencies as cargo would by default. While the override stands, stepping
into a dependency in a debugger is limited: the code is optimised and carries
line tables only, so it has no local variables to show. Setting `debug = true`
under `[profile.dev.package."*"]` keeps full debug info, on code still
optimised.

The Play menu lists the **run configurations** in your
project's `jackdaw.toml` (`[[run]]` entries carrying a name, environment
variables, arguments, an instance count, and a working directory). A
project with no `jackdaw.toml` still plays: the editor synthesizes a
single default run. See [Configuration](../reference/configuration.md)
for the fields.

How many configs you define is up to your game. Some games run a single
process; others split into several that you launch together. Configs
differ only in launch environment, never in what gets built. PIE treats
each launched process as an instance and streams the focused one.

## Your own launch environment

A run config is committed, so it can only say what is true for everyone
working on the game. The variables that point a launch at a particular
server, or carry a sign-in token, or ask for an offline mode, are yours
alone. Those go in **Play Settings**, which the `play.settings` operator
opens:

- **Environment** is a line of `NAME=value` words, added to every game
  the editor launches and overriding a run config's own.
- **Arguments** is a line of words, appended to the ones the run config
  names.

Both are kept with the project's other editor preferences, under
`.jackdaw/`, which a project does not commit, and the editor never writes
or logs their values anywhere else. To set them without the dialog:

    play.settings env="REALM=dev TOKEN=..." args="--offline"

Either parameter replaces what is held; naming neither opens the dialog.

The **Game** panel docks in the bottom dock area next to Assets. An embedded
Play brings it to the front, since the game only streams while the panel is
showing.

## Launching

The play controls in the title bar are **Play**, **Pause**, **Step** and
**Stop**, with the Play menu on the arrow beside Play.

| Key | Action |
| --- | --- |
| `F5` | Play, or resume a paused game |
| `Ctrl+F5` (`Cmd+F5` on macOS) | Restart |
| `F7` | Pause or resume |
| `F8` | Stop |
| `F10` | Step one tick while paused |

Every key can be changed in the keymap. `F5` does nothing while a game is
running, so a second press never restarts it by accident.

Hit **Play** to launch, from the toolbar or the Play button in the Game
panel. The Game panel comes to the front as the build starts. While the game
builds, Play reads **Building...**
and **Stop** cancels the launch, stopping the build. Once the game runs, Play
fills with the accent colour, the viewport's edge takes the accent too, and
the status bar reads **Playing**, followed by the run configuration's name
when it has one. The Game panel starts
streaming immediately, beginning with whatever the game shows first (often a
menu or title screen).

**Pause** freezes the game's clock and shows as pressed; Play then reads
**Resume**. While paused, **Step** advances the game by one fixed tick. The
game keeps rendering and keeps taking your edits while paused, so you can
change a value and step to see it act.

The Play menu holds:

- one row per run configuration instance, ticked while it runs; click a row
  to launch or stop that instance on its own
- the scene Play will run: the scene in front, unless a run configuration
  names one
- **Rebuild**, which rebuilds the project so new components appear in the
  editor
- **Restart**, which rebuilds and relaunches the running game
- **Play in separate window**, ticked when the next launch opens the game in
  its own OS window instead of the Game panel. Without it (the default), the
  game renders off-screen and streams into the Game panel at full frame rate,
  which is the mode you want for input capture and picking. In its own window
  the Game panel still mirrors the active in-game camera once one exists, but
  menus do not stream and input capture is not offered. The setting is kept
  per developer with the Play settings.

## The Game panel header

- The **view menu** sets the size the game renders at. **Free** fills the
  panel. **16:9**, **4:3** and **1:1** keep that aspect and letterbox it
  inside the panel. **1280 x 720** and **1920 x 1080** render at that size and
  scale it to fit. The menu also holds **Mute**, which silences the running
  game and the games Play launches until you unmute; a **muted** chip shows
  while it is on.
- The **instance picker** shows when more than one instance is running. It
  names the instance on show, such as **Server** or **Client 2**, and lists the
  others to switch to.
- The **pop-out** button at the right moves the game to its own window, or
  back into the panel. It shares the **Play in separate window** setting, and
  a running game restarts in the new place. Play in a separate window when you
  want real performance numbers; the panel is not a performance view.

The view size and mute are kept per developer with the Play settings.

## Playing the game

When the game shows its first frame, the Game panel takes input on its own,
unless you clicked or pressed a key in the editor while it was starting, so a
long build never takes input from you once you have gone back to editing.
Otherwise click inside the Game panel to engage input capture. While captured:

- Keyboard and mouse forward to the game. WASD, mouse-look, scroll, clicks,
  and typing all reach it.
- Editor keybinds are suppressed. Tool keys and `Ctrl+S` go to the game,
  not the editor.
- **Shift+Esc to release** shows over the game for a moment, and the panel
  border takes the capture accent.

Plain `Esc` forwards to the game (so the in-game menu still opens). Press
`Shift+Esc` to release capture and return control to the editor. The Stop
key (`F8`) also works while captured: it releases capture and stops the game,
so a game that binds `F8` itself does not get it while playing in the editor. Capture
also releases on its own if you stop the game, switch instances, click away
to another application, or close the panel, and any keys you were holding
are released so nothing stays stuck down.

## Editing while playing

The editor stays as it is when stopped. The outliner, Components panel and
viewport all work on the scene document, so anything you do during Play is an
ordinary edit: it undoes, marks the tab as modified, and saves with the scene.
Each edit also reaches every running instance straight away, including adds,
deletes and reparents.

Dragging a field in the Components panel moves the game on every tick. The edit
joins the document when you let go, and a cancelled drag puts the game back. A
transform gizmo drag reaches the game when you let go.

Selecting an entity draws a bounding box in the game's frame around every entity
spawned from that node by the scene in front. A child inside a prefab instance boxes
that instance's child, and a node in an open prefab tab boxes it in every instance.

The editor does not show entities the game spawned at runtime, or the values a
running game holds.

## Stopping and restarting

- **Stop** ends the game process. The Game panel returns to its idle state,
  the panel that was in front before Play comes back, and the viewport's edge
  clears. Edits made during Play stay in the document. A game that exits or
  crashes on its own puts the controls back to stopped too.
- When a launch goes wrong, the Game panel stays in front and says why: the
  exit code and the last lines the game printed for a crash, a note that the
  game must add `JackdawPlugin` (with the `pie` feature of `jackdaw_runtime`)
  when it never connects, or that the build failed, with a button that shows
  the build errors. **Restart** in the panel launches the game that went away
  again.
- **Restart** relaunches with the current window setting, which is how you
  apply a change to **Play in separate window**.
