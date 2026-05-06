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
- Drag a `.jsn` to open it.
- Drop new files into `assets/` from your file manager.
  Bevy's `file_watcher` is on in the templates, so they show
  up without a manual refresh.

If you only need a texture and no PBR parameters, this is
the path. The "texture browser" that older docs and tutorials
mention is just this panel filtered to images.

## Material browser

A sibling panel for named PBR materials: bundles of textures
plus material parameters (metallic, roughness, normal
strength, parallax) registered in the `MaterialRegistry`
resource. Use this when one texture isn't enough, or when
you want to share material settings across many brushes.

### Auto-detection

If you drop a folder of textures named consistently (e.g.
`brick_albedo.png`, `brick_normal.png`,
`brick_roughness.png`), the material browser groups them
into one auto-detected entry. The regex driving detection
is `pbr_filename_regex` in `src/material_browser.rs`; it
recognises common suffixes (`_albedo`, `_diffuse`, `_normal`,
`_n`, `_roughness`, `_r`, `_metallic`, `_m`, `_ao`,
`_height`, `_displacement`).

You can edit the resulting material in the inspector.
Material values serialize into the scene's `JsnAssets`
table (or the project-wide catalog) keyed by
`bevy_pbr::StandardMaterial` and ride along with save.

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
  `.jsn`. References use `#Name`.
- Project-wide: it lives in `.jsn/catalog.jsn` (legacy
  fallback `assets/catalog.jsn`) and any scene in the
  project can reference it. References use `@Name`.

The browser shows both, with the source labelled.

## Common gotchas

- **Texture didn't show up after I dropped it in.** Bevy's
  watcher catches new files but only existing scenes reload
  their materials. Re-select the brush face to refresh.
- **The auto-detect groups two unrelated textures.**
  Filename heuristics are coarse. Rename the files or open
  the affected definition and split it manually.
- **Material disappears in the standalone build.** Standalone
  loads `assets/scene.jsn` plus `.jsn/catalog.jsn`. Scene-local
  materials still ship inline; project references resolve
  from the catalog at load time, so a missing catalog file
  causes `@Name` references to fall back to defaults.
