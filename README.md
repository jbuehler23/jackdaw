<img width="1917" height="1033" alt="image" src="https://github.com/user-attachments/assets/fa53542b-de0a-420b-bde0-bdcb180992a5" />

## Usage

Add `jackdaw::EditorPlugin` to your app.

```rust
use bevy::prelude::*;
use jackdaw::EditorPlugin;

fn main() -> AppExit {
    App::new()
        .add_plugins((DefaultPlugins, EditorPlugin))
        ...
        .run()
}
```

Or have a look at the [examples]https://github.com/jbuehler23/jackdaw/tree/main/examples) for more advanced useages.

## Keyboard Shortcuts

### Navigation

| Key | Action |
|-----|--------|
| RMB + Drag | Look around |
| WASD | Move (forward / left / back / right) |
| Q / E | Move up / down |
| Shift | Double speed |
| Scroll | Dolly forward / back |
| RMB + Scroll | Adjust move speed |
| F | Focus selected |
| Ctrl+1-9 | Save camera bookmark |
| 1-9 | Restore camera bookmark |

### Selection

| Key | Action |
|-----|--------|
| LMB | Select entity |
| Ctrl+Click | Toggle multi-select |
| Shift+LMB Drag | Box select |

### Transform

| Key | Action |
|-----|--------|
| Esc | Translate mode |
| R | Rotate mode |
| T | Scale mode |
| X | Toggle local / world space |
| MMB | Toggle snap |
| Ctrl (during drag) | Toggle snap |
| Arrows | Nudge (grid-unit move) |
| Alt+Arrows | 90° rotate |
| PageUp / PageDown | Nudge vertical |

### Entity

| Key | Action |
|-----|--------|
| Delete / Backspace | Delete selected |
| Ctrl+D | Duplicate |
| Ctrl+C | Copy components |
| Ctrl+V | Paste components |
| H | Toggle visibility |
| Alt+G | Reset position |
| Alt+R | Reset rotation |
| Alt+S | Reset scale |

### Brush Editing

| Key | Action |
|-----|--------|
| 1 | Vertex mode |
| 2 | Edge mode |
| 3 | Face mode |
| 4 | Clip mode |
| X / Y / Z | Constrain axis |
| Shift+Click | Multi-select |
| Delete | Delete selected element |
| Enter | Apply clip plane |
| Esc | Exit brush edit |

### Brush Draw

| Key | Action |
|-----|--------|
| B | Draw brush (add) |
| C | Draw brush (cut) |
| Tab | Toggle add / cut mode |
| Click | Place vertex / advance phase |
| Enter | Close polygon |
| Backspace | Remove last vertex |
| Esc / Right-click | Cancel drawing |

### View

| Key | Action |
|-----|--------|
| Ctrl+Shift+W | Toggle wireframe |
| [ | Decrease grid size |
| ] | Increase grid size |
| Ctrl+Alt+Scroll | Change grid size |

### File

| Key | Action |
|-----|--------|
| Ctrl+S | Save scene |
| Ctrl+O | Open scene |
| Ctrl+Shift+N | New scene |
| Ctrl+Z | Undo |
| Ctrl+Shift+Z | Redo |
