# Materials and textures

Two panels handle this: the **Asset Browser** and the
**Material Browser**. Earlier builds had a separate texture
browser, but it was absorbed into the asset browser and only
the two remain.

## Asset browser

The bottom-left panel by default. Shows the project's
`assets/` directory as a tree on the left and a tile grid of
the current folder on the right. Image files (png, jpg, jpeg,
bmp, tga, webp, ktx2) render as thumbnails; everything else
shows a generic file tile.

What you do here:

- Click an image to preview it in the side panel (KTX2
  arrays show a layer slider).
- Drag an image tile onto a brush face in the viewport to
  apply it as the face's `texture_path`. This routes through
  the `ApplyTextureOp` operator, so it goes on the undo
  stack.
- Drag a `.glb` into the viewport to spawn a model entity.
- Drag a `.bsn` to open it.
- Drop new files into `assets/` from your file manager.
  The editor watches `assets/`, so they show up without a
  manual refresh.

If you only need a texture and no PBR parameters, this is
the path. The "texture browser" that older docs and tutorials
mention is just this panel filtered to images.

## Material browser

A sibling panel for PBR materials: bundles of textures plus
material parameters (metallic, roughness, normal strength,
parallax). Use this when one texture isn't enough, or when
you want to share material settings across many brushes.

### What it lists

Every material file the project holds, wherever it sits. The
editor indexes each `.bsn` under `assets/` by what the file
says it holds, so the panel is a view of that index rather
than of one folder; there is no materials directory to point
it at. Beside those sit the materials that have no file yet,
marked unsaved: the texture sets detected under `assets/`, one
you made with **New Material**, and one whose file has gone.

### Auto-detection

If you drop a folder of textures named consistently (e.g.
`brick_albedo.png`, `brick_normal.png`,
`brick_roughness.png`), the panel groups them into one
detected entry, wherever under `assets/` they sit. The regex
driving detection is `pbr_filename_regex` in the
`jackdaw_material` crate; it recognises common suffixes
(`_albedo`, `_diffuse`, `_normal`, `_n`, `_roughness`, `_r`,
`_metallic`, `_m`, `_ao`, `_height`, `_displacement`).

A detected entry is a material you can edit and apply at
once; **Save Material** writes it a file of its own, and from
then on the panel lists it from the index rather than from the
scan.

### Saving

**Save Material** writes the material back to its own file,
or, for one that has none yet, to `assets/materials/<name>.bsn`.
**Save Material As** opens a file dialog on that same default
so you can put it anywhere under `assets/`; what you name the
file is what the material is called. A material that is edited
in the panel and already has a file is written back as the
edit lands.

An edit to a material with no file stays in memory, and a
scene that uses it embeds it inline on save, so it keeps
rendering outside this editor run.

### Applying

Select a brush face, drop a material onto it. The face's
`material_name` field takes priority over its `texture_path`,
so a face with both falls back gracefully if the material is
missing.

### Preview

Each definition renders onto a sphere via a render-to-texture
pipeline (`src/material_preview.rs`). Previews use
`RenderLayers::layer(1)` so they don't clash with main-view
geometry.

## Project-wide vs scene-local materials

Two storage tiers:

- Scene-local: the material lives only inside the current
  `.bsn`. References use `#Name`.
- Project-wide: it lives in a `.bsn` file of its own, in
  whatever folder you keep it in; the editor finds it by
  reading what the file holds. A save with no folder in mind
  puts it under `assets/materials/`. Any scene in the project
  can reference it, and references spell its path, such as
  `materials/slate.bsn`. Scenes written before paths spell
  `@Name` instead; they still load, and the next save writes
  the path. `project.migrate_asset_references` writes every
  such name out as a path in one go.

The browser shows both, with the source labelled.

## Common gotchas

- **Texture didn't show up after I dropped it in.** Bevy's
  watcher catches new files but only existing scenes reload
  their materials. Re-select the brush face to refresh.
- **The auto-detect groups two unrelated textures.**
  Filename heuristics are coarse. Rename the files, or save
  the material and edit its slots in the panel.
- **Material disappears in the standalone build.** Standalone
  walks every `.bsn` under `assets/` and loads the ones holding
  a material. Scene-local materials still ship inline; a
  reference resolves to the file at that path, so a material
  file left out of the build falls back to a default.
