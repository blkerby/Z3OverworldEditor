use crate::{
    message::Message,
    state::{EditorState, Flip, PaletteId, Tile, TileBlock, TileCoord, TileIdx},
};

use anyhow::{Context, Result};
use iced::Point;

#[derive(Debug)]
pub enum UndoAction {
    None,
    Irreversible,
    Ok(Message),
}

pub fn get_undo_action(state: &EditorState, message: &Message) -> Result<UndoAction> {
    // We only implement undo functionality for messages that produce
    // changes to the project data (i.e. palettes and areas), not to
    // transient editor state.
    let action = match message {
        Message::Nothing => UndoAction::None,
        Message::Event(_) => UndoAction::None,
        Message::Focus(_) => UndoAction::None,
        Message::WindowClose(_) => UndoAction::None,
        Message::SaveProject => UndoAction::None,
        Message::OpenProject => UndoAction::None,
        Message::ModifiedReload => UndoAction::None,
        Message::RebuildProjectDialogue => UndoAction::None,
        Message::RebuildProject => UndoAction::None,
        Message::ProjectOpened(_) => UndoAction::Irreversible,
        Message::SettingsDialogue => UndoAction::None,
        Message::HelpDialogue => UndoAction::None,
        Message::SetPixelSize(_) => UndoAction::None,
        Message::SetGridAlpha(_) => UndoAction::None,
        Message::CloseDialogue => UndoAction::None,
        Message::ImportDialogue => UndoAction::None,
        Message::ImportConfirm(_) => UndoAction::None,
        Message::ImportROMProgress => UndoAction::None,
        Message::ImportROM => UndoAction::Irreversible,
        Message::SelectPalette(_) => UndoAction::None,
        Message::AddPaletteDialogue => UndoAction::None,
        Message::SetAddPaletteName(_) => UndoAction::None,
        Message::SetAddPaletteID(_) => UndoAction::None,
        Message::AddPalette { id, .. } => UndoAction::Ok(Message::DeletePalette(*id)),
        Message::DeletePaletteDialogue => UndoAction::None,
        Message::DeletePalette(id) => {
            if let Some(&palette_idx) = state.palettes_id_idx_map.get(id) {
                let pal = state.palettes[palette_idx].clone();
                UndoAction::Ok(Message::RestorePalette(pal))
            } else {
                UndoAction::None
            }
        }
        Message::RestorePalette(pal) => UndoAction::Ok(Message::DeletePalette(pal.id)),
        Message::RenamePaletteDialogue => UndoAction::None,
        Message::SetRenamePaletteName(_) => UndoAction::None,
        Message::RenamePalette { id, name: _ } => {
            let idx = *state
                .palettes_id_idx_map
                .get(id)
                .context("palette not found")?;
            UndoAction::Ok(Message::RenamePalette {
                id: *id,
                name: state.palettes[idx].name.clone(),
            })
        }
        Message::HideModal => UndoAction::None,
        Message::SelectColor(_, _) => UndoAction::None,
        &Message::BrushColor {
            palette_id,
            color_idx,
            color: _,
        } => {
            let idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            UndoAction::Ok(Message::BrushColor {
                palette_id,
                color_idx,
                color: state.palettes[idx].colors[color_idx as usize],
            })
        }
        Message::ChangeRed(_) => UndoAction::None,
        Message::ChangeGreen(_) => UndoAction::None,
        Message::ChangeBlue(_) => UndoAction::None,
        &Message::AddTileRow(palette_id) => UndoAction::Ok(Message::DeleteTileRow(palette_id)),
        Message::DeleteTileRow(palette_id) => {
            let idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            let pal = &state.palettes[idx];
            let row = pal.tiles[pal.tiles.len() - 16..].to_vec();
            UndoAction::Ok(Message::RestoreTileRow(*palette_id, row))
        }
        &Message::RestoreTileRow(palette_id, _) => {
            UndoAction::Ok(Message::DeleteTileRow(palette_id))
        }
        &Message::SetTilePriority {
            palette_id,
            tile_idx,
            priority: _,
        } => {
            let idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            UndoAction::Ok(Message::SetTilePriority {
                palette_id,
                tile_idx,
                priority: state.palettes[idx].tiles[tile_idx as usize].priority,
            })
        }
        &Message::SetTileCollision {
            palette_id,
            tile_idx,
            collision: _,
        } => {
            let idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            UndoAction::Ok(Message::SetTileCollision {
                palette_id,
                tile_idx,
                collision: state.palettes[idx].tiles[tile_idx as usize].collision,
            })
        }
        &Message::SetTileHFlippable {
            palette_id,
            tile_idx,
            h_flippable: _,
        } => {
            let idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            UndoAction::Ok(Message::SetTileHFlippable {
                palette_id,
                tile_idx,
                h_flippable: state.palettes[idx].tiles[tile_idx as usize].h_flippable,
            })
        }
        &Message::SetTileVFlippable {
            palette_id,
            tile_idx,
            v_flippable: _,
        } => {
            let idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            UndoAction::Ok(Message::SetTileVFlippable {
                palette_id,
                tile_idx,
                v_flippable: state.palettes[idx].tiles[tile_idx as usize].v_flippable,
            })
        }
        &Message::TilesetBrush {
            palette_id,
            coords: Point { x: x0, y: y0 },
            ref selected_gfx,
            ..
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            let mut s: Vec<Vec<Tile>> = vec![];
            for y in 0..selected_gfx.len() {
                let mut row: Vec<Tile> = vec![];
                for x in 0..selected_gfx[0].len() {
                    let y1 = y + y0 as usize;
                    let x1 = x + x0 as usize;
                    let i = y1 * 16 + x1;
                    if x1 < 16 && i < state.palettes[pal_idx].tiles.len() {
                        row.push(state.palettes[pal_idx].tiles[i]);
                    }
                }
                s.push(row);
            }
            let msg = UndoAction::Ok(Message::TilesetBrush {
                palette_id,
                coords: Point { x: x0, y: y0 },
                selected_gfx: s,
                tile_block: None,
            });
            msg
        }
        Message::SelectPixel(_, _) => UndoAction::None,
        &Message::BrushPixel {
            palette_id,
            tile_idx,
            coords,
            color_idx: _,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            let pal = &state.palettes[pal_idx];
            let c = pal.tiles[tile_idx as usize].pixels[coords.y as usize][coords.x as usize];
            UndoAction::Ok(Message::BrushPixel {
                palette_id,
                tile_idx,
                coords,
                color_idx: c,
            })
        }
        Message::SelectArea(_, _) => UndoAction::None,
        Message::AddAreaDialogue => UndoAction::None,
        Message::SetAddAreaName(_) => UndoAction::None,
        Message::SetAddAreaSizeX(_) => UndoAction::None,
        Message::SetAddAreaSizeY(_) => UndoAction::None,
        Message::AddArea { name, size: _ } => UndoAction::Ok(Message::DeleteArea(name.clone())),
        Message::EditAreaDialogue => UndoAction::None,
        Message::SetEditAreaName(_) => UndoAction::None,
        Message::EditArea { old_name, new_name } => UndoAction::Ok(Message::EditArea {
            old_name: new_name.clone(),
            new_name: old_name.clone(),
        }),
        Message::EditAreaBGRed(_) => UndoAction::None,
        Message::EditAreaBGGreen(_) => UndoAction::None,
        Message::EditAreaBGBlue(_) => UndoAction::None,
        &Message::EditAreaBGColor {
            ref area_id,
            color: _,
        } => UndoAction::Ok(Message::EditAreaBGColor {
            area_id: area_id.clone(),
            color: state.areas[area_id].bg_color,
        }),
        Message::DeleteAreaDialogue => UndoAction::None,
        Message::DeleteArea(_) => UndoAction::Irreversible,
        Message::SelectTheme(_, _) => UndoAction::None,
        Message::AddThemeDialogue => UndoAction::None,
        Message::SetAddThemeName(_) => UndoAction::None,
        Message::AddTheme(theme_name) => UndoAction::Ok(Message::DeleteTheme(theme_name.clone())),
        Message::RenameThemeDialogue => UndoAction::None,
        Message::SetRenameThemeName(_) => UndoAction::None,
        Message::RenameTheme { old_name, new_name } => UndoAction::Ok(Message::RenameTheme {
            old_name: new_name.clone(),
            new_name: old_name.clone(),
        }),
        Message::DeleteThemeDialogue => UndoAction::None,
        Message::DeleteTheme(_) => UndoAction::Irreversible,
        Message::HoverArea(_) => UndoAction::None,
        Message::StartTileSelection(_, _) => UndoAction::None,
        Message::ProgressTileSelection(_) => UndoAction::None,
        Message::EndTileSelection(_) => UndoAction::None,
        Message::AreaBrush {
            position,
            area_id,
            coords,
            selection,
            palette_only,
        } => {
            let mut palettes: Vec<Vec<PaletteId>> = vec![];
            let mut tiles: Vec<Vec<TileIdx>> = vec![];
            let mut flips: Vec<Vec<Flip>> = vec![];
            for y in 0..selection.size.1 {
                let mut palette_row: Vec<PaletteId> = vec![];
                let mut tile_row: Vec<TileIdx> = vec![];
                let mut flip_row: Vec<Flip> = vec![];
                for x in 0..selection.size.0 {
                    if let Ok(p) = state.areas[area_id].get_palette(coords.x + x, coords.y + y) {
                        palette_row.push(p);
                    }
                    if let Ok(t) = state.areas[area_id].get_tile(coords.x + x, coords.y + y) {
                        tile_row.push(t);
                    }
                    if let Ok(f) = state.areas[area_id].get_flip(coords.x + x, coords.y + y) {
                        flip_row.push(f);
                    }
                }
                if palette_row.is_empty() {
                    break;
                }
                palettes.push(palette_row);
                tiles.push(tile_row);
                flips.push(flip_row);
            }
            let new_selection = TileBlock {
                size: (palettes[0].len() as TileCoord, palettes.len() as TileCoord),
                palettes,
                tiles,
                flips,
            };
            UndoAction::Ok(Message::AreaBrush {
                position: *position,
                area_id: area_id.clone(),
                coords: *coords,
                selection: new_selection,
                palette_only: *palette_only,
            })
        }
        Message::OpenTile { .. } => UndoAction::None,
        Message::MovingTilesProgress { .. } => UndoAction::None,
        &Message::MoveTiles {
            ref src_selection,
            ref dst_selection,
            check_reversible,
        } => {
            if !check_reversible {
                return Ok(UndoAction::Irreversible);
            }
            UndoAction::Ok(Message::MoveTiles {
                src_selection: dst_selection.clone(),
                dst_selection: src_selection.clone(),
                check_reversible: false,
            })
        }
        Message::MoveTilesConfirmDialogue { .. } => UndoAction::None,
    };
    Ok(action)
}
