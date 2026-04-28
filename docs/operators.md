## Available Operators

### Cancel Tool (`modal.cancel`)

Cancels the currently active tool

### Apply Texture (`material.apply_texture`)

**flags**: *undo*

Apply a texture material to the selected faces or brushes

### Delete Keyframes (`clip.delete_keyframes`)

**flags**: *undo*

Remove the selected animation keyframes.

### Draw Brush (`viewport.draw_brush_modal`)

**flags**: *undo*, *modal*



### mesh.add_brush (`mesh.add_brush`)

**flags**: *undo*



### Draw Brush (Confirm) (`draw_brush.confirm`)

Confirms the current draw brush operation

### Join (Convex Merge) (`brush.join`)

**flags**: *undo*

Merge all selected brushes into a single convex-hull brush. Requires at least two `Brush` entities in `Selection::entities`; availability (`can_run_binary_brush_op`) is false otherwise. The first selected brush keeps its entity id and transform; others are despawned.

### CSG Subtract (`brush.csg_subtract`)

**flags**: *undo*

Subtract the non-first selected brushes from the first. Requires at least two `Brush` entities in `Selection::entities` (first is the target, rest are cutters); availability (`can_run_binary_brush_op`) is false otherwise. The target may be split into multiple fragment brushes.

### CSG Intersect (`brush.csg_intersect`)

**flags**: *undo*

Replace the selected brushes with the solid shared by all of them. Requires at least two `Brush` entities in `Selection::entities`; availability (`can_run_binary_brush_op`) is false otherwise. When the intersection is empty the impl exits silently without mutating the world.

### Extend to Brush (`brush.extend_face_to_brush`)

**flags**: *undo*

Extend a face of the primary brush to conform to the shape of the other selected brushes. Two entry paths:
• `EditMode::BrushEdit(Face)` with a face selected on `BrushSelection` and ≥ 1 other brush in `Selection::entities`.
• `EditMode::Object` with ≥ 2 brushes in `Selection::entities` and a remembered face on the primary.
Availability (`can_run_extend_face`) is false when neither entry path applies. The Object-mode path additionally tries to resolve a hovered face via raycast once invoked; if that also fails it returns `Cancelled`.

### Toggle Add/Cut (`viewport.draw_brush.toggle_mode`)

Flip between adding and cutting while drawing.

### Commit Polygon (`viewport.draw_brush.commit_polygon`)

Close the polygon and start extruding it.

### Remove Last Vertex (`viewport.draw_brush.remove_last_vertex`)

Take back the last polygon point you placed.

### Cancel Cut (`viewport.draw_brush.cancel_cut`)

Bail out of the current cut.

### New (`scene.new`)

**flags**: *undo*



### Open (`scene.open`)

**flags**: *undo*



### Save (`scene.save`)



### Save As... (`scene.save_as`)



### Save Selection as Template (`scene.save_selection_as_template`)



### Open Recent... (`scene.open_recent`)

**flags**: *undo*



### Undo (`history.undo`)



### Redo (`history.redo`)



### Extensions... (`app.open_extensions`)



### Keybinds... (`app.open_keybinds`)



### Toggle Hot Reload (`app.toggle_hot_reload`)



### Home (`app.go_home`)



### Toggle Wireframe (`view.toggle_wireframe`)

**flags**: *undo*



### Toggle Bounding Boxes (`view.toggle_bounding_boxes`)

**flags**: *undo*



### Cycle Bounding Box Mode (`view.cycle_bounding_box_mode`)

**flags**: *undo*



### Toggle Face Grid (`view.toggle_face_grid`)

**flags**: *undo*



### Toggle Brush Wireframe (`view.toggle_brush_wireframe`)

**flags**: *undo*



### Toggle Brush Outline (`view.toggle_brush_outline`)

**flags**: *undo*



### Toggle Alignment Guides (`view.toggle_alignment_guides`)

**flags**: *undo*



### Toggle Collider Gizmos (`view.toggle_collider_gizmos`)

**flags**: *undo*



### Toggle Hierarchy Arrows (`view.toggle_hierarchy_arrows`)

**flags**: *undo*



### Increase Grid (`grid.increase`)

**flags**: *undo*



### Decrease Grid (`grid.decrease`)

**flags**: *undo*



### Gizmo Translate (`gizmo.mode.translate`)

**flags**: *undo*



### Gizmo Rotate (`gizmo.mode.rotate`)

**flags**: *undo*



### Gizmo Scale (`gizmo.mode.scale`)

**flags**: *undo*



### Toggle Gizmo Space (`gizmo.space.toggle`)

**flags**: *undo*



### Object Mode (`edit_mode.object`)

**flags**: *undo*



### Vertex Mode (`edit_mode.vertex`)

**flags**: *undo*



### Edge Mode (`edit_mode.edge`)

**flags**: *undo*



### Face Mode (`edit_mode.face`)

**flags**: *undo*



### Clip Mode (`edit_mode.clip`)

**flags**: *undo*



### Exit Edit Mode (`brush.exit_edit_mode`)

Stop editing the brush and return to selecting whole entities.

### Delete (`entity.delete`)

**flags**: *undo*



### Duplicate (`entity.duplicate`)

**flags**: *undo*



### Copy Components (`entity.copy_components`)



### Paste Components (`entity.paste_components`)

**flags**: *undo*



### Toggle Visibility (`entity.toggle_visibility`)



### Hide Unselected (`entity.hide_unselected`)



### Unhide All (`entity.unhide_all`)



### Cube (`entity.add.cube`)

**flags**: *undo*



### Sphere (`entity.add.sphere`)

**flags**: *undo*



### Point Light (`entity.add.point_light`)

**flags**: *undo*



### Directional Light (`entity.add.directional_light`)

**flags**: *undo*



### Spot Light (`entity.add.spot_light`)

**flags**: *undo*



### Camera (`entity.add.camera`)

**flags**: *undo*



### Empty (`entity.add.empty`)

**flags**: *undo*



### Navmesh (`entity.add.navmesh`)

**flags**: *undo*



### Terrain (`entity.add.terrain`)

**flags**: *undo*



### Prefab (`entity.add.prefab`)

**flags**: *undo*



### Reset Position (`transform.reset_position`)

**flags**: *undo*



### Reset Rotation (`transform.reset_rotation`)

**flags**: *undo*



### Reset Scale (`transform.reset_scale`)

**flags**: *undo*



### Rotate 90° Yaw CCW (`transform.rotate_90_yaw_ccw`)

**flags**: *undo*



### Rotate 90° Yaw CW (`transform.rotate_90_yaw_cw`)

**flags**: *undo*



### Rotate 90° Pitch CCW (`transform.rotate_90_pitch_ccw`)

**flags**: *undo*



### Rotate 90° Pitch CW (`transform.rotate_90_pitch_cw`)

**flags**: *undo*



### Rotate 90° Roll CCW (`transform.rotate_90_roll_ccw`)

**flags**: *undo*



### Rotate 90° Roll CW (`transform.rotate_90_roll_cw`)

**flags**: *undo*



### Nudge −X (`transform.nudge_x_neg`)

**flags**: *undo*



### Nudge +X (`transform.nudge_x_pos`)

**flags**: *undo*



### Nudge −Y (`transform.nudge_y_neg`)

**flags**: *undo*



### Nudge +Y (`transform.nudge_y_pos`)

**flags**: *undo*



### Nudge −Z (`transform.nudge_z_neg`)

**flags**: *undo*



### Nudge +Z (`transform.nudge_z_pos`)

**flags**: *undo*



### Physics Tool (`physics.activate`)

**flags**: *undo*, *modal*

Drop physics-enabled objects into the scene like a hammer.

### Rename Entity (`hierarchy.rename_begin`)

**flags**: *undo*, *modal*

Rename the selected entity in the hierarchy.

### Box Select (`selection.box_select`)

**flags**: *undo*, *modal*

Drag a rectangle to select entities inside it.

### Place Clip Point (`brush.clip.place_point`)

Raycast the cursor against the selected brush, snap, and add the resulting local-space point to `ClipState`. Availability (`can_place_point`) requires clip mode, a selected brush, and fewer than three existing points.

### Cycle Clip Mode (`brush.clip.cycle_mode`)

Cycle `ClipState.mode` through KeepFront → KeepBack → Split. Availability (`can_apply_or_cycle`) requires clip mode and a computed preview plane.

### Apply Clip (`brush.clip.apply`)

Apply the preview plane to the selected brush per the current `ClipState.mode` (KeepFront / KeepBack / Split). Availability (`can_apply_or_cycle`) requires clip mode and a computed preview plane.

### Clear Clip Points (`brush.clip.clear`)

Reset `ClipState` to its default (no points, KeepFront mode). Availability (`can_clear`) requires clip mode with non-default state and no active modal.

### Delete Element (`brush.delete_element`)

**flags**: *undo*

Delete the selected vertex / edge / face from the active brush. Dispatch follows the current `BrushEditMode`. The brush must retain at least four vertices (a tetrahedron); availability (`can_run_element_op`) is false otherwise.

### Nudge Up (`brush.nudge_up`)

**flags**: *undo*

Nudge the selected sub-element +Y by one grid step. Dispatch follows `BrushEditMode`; availability (`can_run_element_op`) gates on the brush-edit gate.

### Nudge Down (`brush.nudge_down`)

**flags**: *undo*

Nudge the selected sub-element -Y by one grid step. Dispatch follows `BrushEditMode`; availability (`can_run_element_op`) gates on the brush-edit gate.

### Drag Face (`brush.face.drag`)

**flags**: *modal*

Pick a brush face under the cursor and drag it (push/pull or shift+extrude). Modal: commits on LMB release, cancels on Escape or right-click. Auto-enters face-edit mode from object mode as a quick action; the drag-end / cancel restores Object mode in that case.

### Drag Vertex (`brush.vertex.drag`)

**flags**: *modal*

Pick a brush vertex (or shift-pick a midpoint to split) and drag it. Modal: X / Y / Z toggle axis constraints during the drag, LMB release commits, Escape or right-click cancels.

### Drag Edge (`brush.edge.drag`)

**flags**: *modal*

Pick a brush edge and drag it. Modal: X / Y / Z toggle axis constraints, LMB release commits, Escape or right-click cancels.

### Gizmo Drag (`gizmo.drag`)

**flags**: *modal*

Drag the active transform gizmo to translate / rotate / scale the primary selection. Modal: commits on LMB release, cancels on Escape (restoring the start transform). Mode and axis come from the toolbar's `GizmoMode` resource and the click-time `GizmoHoverState`.

### Sculpt Terrain (`terrain.sculpt`)

**flags**: *modal*

Apply the active sculpt tool while LMB is held. Modal: commits the height delta as a single undo entry on release; Escape restores the pre-stroke heights.

### Fetch Scene (`navmesh.fetch`)

**flags**: *undo*

Fetch the latest scene mesh from the connected game.

### Build (`navmesh.build`)

**flags**: *undo*

Bake a navmesh for the current scene.

### Save (`navmesh.save`)

**flags**: *undo*

Save the baked navmesh to disk.

### Load (`navmesh.load`)

**flags**: *undo*

Load a navmesh from disk.

### Toggle Visual (`navmesh.toggle_visual`)

**flags**: *undo*

Show or hide the navmesh visual mesh.

### Toggle Obstacles (`navmesh.toggle_obstacles`)

**flags**: *undo*

Show or hide the navmesh obstacle markers.

### Toggle Detail Mesh (`navmesh.toggle_detail`)

**flags**: *undo*

Show or hide the navmesh detail mesh.

### Toggle Polygon Mesh (`navmesh.toggle_poly`)

**flags**: *undo*

Show or hide the navmesh polygon mesh.

### Play (`pie.play`)

**flags**: *undo*

Start the game running in the editor.

### Pause (`pie.pause`)

**flags**: *undo*

Pause the running game.

### Stop (`pie.stop`)

**flags**: *undo*

Stop the running game and restore the scene.

### Raise (`terrain.tool.raise`)

**flags**: *undo*

Pick the raise sculpt tool.

### Lower (`terrain.tool.lower`)

**flags**: *undo*

Pick the lower sculpt tool.

### Flatten (`terrain.tool.flatten`)

**flags**: *undo*

Pick the flatten sculpt tool.

### Smooth (`terrain.tool.smooth`)

**flags**: *undo*

Pick the smooth sculpt tool.

### Noise (`terrain.tool.noise`)

**flags**: *undo*

Pick the noise sculpt tool.

### Generate (`terrain.tool.generate`)

**flags**: *undo*

Open the heightmap-generation panel.

### Generate Terrain (`terrain.generate`)

**flags**: *undo*

Generate a fresh heightmap for the selected terrain.

### Erode Terrain (`terrain.erode`)

**flags**: *undo*

Apply hydraulic erosion to the selected terrain.

### Cycle Array Layer (`asset.cycle_array_layer`)

**flags**: *undo*

Step the previewed array texture by one layer.

### Select Assets Folder (`asset.select_folder`)

**flags**: *undo*

Choose a different folder as the assets directory.

### New Material (`material.create`)

**flags**: *undo*

Create a fresh empty material.

### Rescan Materials (`material.rescan`)

**flags**: *undo*

Refresh the material browser from disk.

### Select Materials Folder (`material.select_folder`)

**flags**: *undo*

Choose a different folder as the materials directory.

### Browse Texture (`material.browse_texture_slot`)

**flags**: *undo*

Pick an image to assign to this material's texture slot.

### Clear Texture (`material.clear_texture_slot`)

**flags**: *undo*

Remove the image from this material's texture slot.

### Add Component (`component.add`)

**flags**: *undo*

Add a component to the selected entity.

### Remove Component (`component.remove`)

**flags**: *undo*

Remove a component from the selected entity.

### Revert To Prefab (`component.revert_baseline`)

**flags**: *undo*

Restore the component to the value it had in the source prefab.

### Enable Physics (`physics.enable`)

**flags**: *undo*

Make the selected entity participate in the physics simulation.

### Disable Physics (`physics.disable`)

**flags**: *undo*

Stop the selected entity from participating in the physics simulation.

### Toggle Keyframe (`animation.toggle_keyframe`)

**flags**: *undo*

Add or replace a keyframe for this property at the current timeline cursor.

### Focus Selected (`viewport.focus_selected`)

**flags**: *undo*

Center the camera on the selected entity.

### Save Camera Bookmark (`viewport.bookmark.save`)

**flags**: *undo*

Save the camera position to a numbered slot.

### Load Camera Bookmark (`viewport.bookmark.load`)

**flags**: *undo*

Restore the camera to a previously-saved slot.

### Document Operators (`operators.document`)

Writes all available operators into a document

