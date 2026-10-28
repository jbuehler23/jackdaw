use std::collections::BTreeSet;
use std::fmt::Write as _;

use bevy::color::ColorToPacked as _;
use jackdaw_tilemap::{CellCoord, Region, TileAddress, TilesetSetup, emit_rows};

use crate::{TerrainCollision, Tileset, TilesetError};

impl Tileset {
    /// Sets `key` to `value` on each of `layers`, or removes it for an empty
    /// value, and says how many layers changed.
    pub fn paint_property(&mut self, layers: &[u32], key: &str, value: &str) -> usize {
        let mut changed = 0;
        for &layer in layers {
            let before = self
                .properties
                .get(&layer)
                .and_then(|p| p.get(key))
                .cloned();
            if value.is_empty() {
                if let Some(properties) = self.properties.get_mut(&layer) {
                    properties.remove(key);
                    if properties.is_empty() {
                        self.properties.remove(&layer);
                    }
                }
            } else {
                let properties = self.properties.entry(layer).or_default();
                properties.insert(key.to_string(), value.to_string());
            }
            let after = self
                .properties
                .get(&layer)
                .and_then(|p| p.get(key))
                .cloned();
            if before != after {
                changed += 1;
            }
        }
        changed
    }

    /// Plain-text summary of the tileset: terrains, sets with their coverage,
    /// flagged tiles, objects, properties, and a grid of each sheet.
    pub fn describe(&self) -> Result<String, TilesetError> {
        let setup = self.setup()?;
        let mut out = String::new();
        let (tile, grid) = (self.tile_size, self.grid);
        let _ = writeln!(
            out,
            "image \"{}\": tile {}x{}, {}x{} layers",
            self.image, tile.x, tile.y, grid.x, grid.y
        );
        let _ = writeln!(out, "terrains: {}", self.terrains.len());
        for t in &self.terrains {
            let [r, g, b, _] = t.swatch.to_u8_array();
            let background = if t.background { ", background" } else { "" };
            let _ = writeln!(
                out,
                "  terrain {} \"{}\": swatch #{r:02x}{g:02x}{b:02x}{background}, collision {}",
                t.id,
                t.name,
                collision_name(t.collision)
            );
        }
        let _ = writeln!(out, "terrain sets: {}", self.terrain_sets.len());
        for s in &self.terrain_sets {
            let locked = if s.locked_manual { ", locked" } else { "" };
            let _ = write!(
                out,
                "  set {} over {}: {:?}{locked}",
                s.inner, s.outer, s.mode
            );
            if let Ok(c) = setup.coverage(s.inner, s.outer) {
                let _ = write!(out, ", {}/{} shapes", c.covered, c.total);
                let missing: Vec<String> = (c.missing.iter())
                    .map(|t| t.peers.map(|p| p.to_string()).join(","))
                    .collect();
                if !missing.is_empty() {
                    let _ = write!(out, ", missing {}", missing.join("; "));
                }
            }
            out.push('\n');
        }
        let flagged: BTreeSet<TileAddress> = setup.flagged().into_iter().collect();
        let _ = writeln!(out, "flagged tiles: {}", flagged.len());
        for (index, sheet) in self.setup_sheets.iter().enumerate() {
            let tiles: Vec<String> = (flagged.iter())
                .filter(|a| a.sheet == index)
                .map(|a| a.tile.to_string())
                .collect();
            if !tiles.is_empty() {
                let _ = writeln!(out, "  sheet {sheet}: {}", tiles.join(" "));
            }
        }
        let _ = writeln!(out, "objects: {}", self.objects.len());
        for o in &self.objects {
            let by_user = if o.by_user { ", by hand" } else { "" };
            let _ = writeln!(
                out,
                "  object on sheet {} at {},{}: {}x{}, {} tiles, pivot {},{}{by_user}",
                o.sheet,
                o.origin.x,
                o.origin.y,
                o.size.x,
                o.size.y,
                o.cells.len(),
                o.pivot.x,
                o.pivot.y
            );
        }
        let _ = writeln!(out, "properties: {} layers", self.properties.len());
        for (layer, properties) in &self.properties {
            let pairs: Vec<String> = (properties.iter())
                .map(|(key, value)| format!("{key}={value}"))
                .collect();
            let _ = writeln!(out, "  layer {layer}: {}", pairs.join(", "));
        }
        let _ = writeln!(
            out,
            "grid legend: . nothing, # object, ? flagged, + transition, terrain id: pure tile"
        );
        for (index, sheet) in (0..).zip(&self.sheets) {
            let _ = writeln!(
                out,
                "sheet {index} \"{}\": {}x{} tiles from layer {}",
                sheet.source, sheet.columns, sheet.rows, sheet.first_layer
            );
            if let Some(rows) = self.sheet_rows(&setup, index, &flagged) {
                for row in rows.lines() {
                    let _ = writeln!(out, "  {row}");
                }
            }
        }
        Ok(out)
    }

    /// One symbol per tile of sheet `sheet`, row by row, as region text.
    fn sheet_rows(
        &self,
        setup: &TilesetSetup,
        sheet: u32,
        flagged: &BTreeSet<TileAddress>,
    ) -> Option<String> {
        let source = self.sheets.get(sheet as usize)?;
        let in_setup = self.setup_sheet(sheet).ok();
        let objects: BTreeSet<u32> = (self.objects.iter())
            .filter(|o| o.sheet == sheet)
            .flat_map(|o| o.cells.iter().map(|c| c.tile))
            .collect();
        let symbol = |tile: u32| {
            if objects.contains(&tile) {
                return '#';
            }
            let Some(address) = in_setup.map(|sheet| TileAddress { sheet, tile }) else {
                return '.';
            };
            if flagged.contains(&address) {
                return '?';
            }
            let Some(found) = setup.tiles.get(&address) else {
                return '.';
            };
            let peers = found.terrains.peers;
            if peers == [0; 9] {
                '.'
            } else if peers.iter().all(|&p| p == peers[0]) {
                char::from_digit(u32::from(peers[0]), 36).unwrap_or('*')
            } else {
                '+'
            }
        };
        let cells = (0..source.columns * source.rows).map(symbol).collect();
        let origin = CellCoord::new(0, 0);
        let region = Region::from_cells(origin, source.columns, source.rows, cells)?;
        Some(emit_rows(&region))
    }
}

fn collision_name(collision: TerrainCollision) -> &'static str {
    match collision {
        TerrainCollision::Solid => "solid",
        TerrainCollision::Trigger => "trigger",
        TerrainCollision::Passable => "passable",
    }
}
