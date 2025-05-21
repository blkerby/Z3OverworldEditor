use hashbrown::HashMap;
use iced::{
    keyboard::{self, key},
    widget, window, Event, Point, Task,
};
use itertools::Itertools;
use log::{error, info, warn};

use crate::{
    import::Importer,
    message::{Message, SelectionSource},
    persist::{
        self, copy_area_theme, delete_area, delete_area_theme, delete_palette, load_area_list,
        remap_tiles, rename_area, rename_area_theme, save_area, save_area_png, scan_used_tiles,
    },
    state::{
        Area, AreaId, AreaPosition, ColorIdx, ColorRGB, Dialogue, EditorState, Flip, Focus,
        PaletteId, Screen, SidePanelView, Tile, TileBlock, TileIdx, Tool, MAX_PIXEL_SIZE,
        MIN_PIXEL_SIZE,
    },
    undo::{get_undo_action, UndoAction},
    view::{open_project, open_rom},
};
use anyhow::{bail, Context, Result};

fn select_tileset_tile(state: &mut EditorState, tile_idx: TileIdx) -> Result<()> {
    state.tile_idx = Some(tile_idx);
    state.start_coords = Some((tile_idx % 16, tile_idx / 16));
    state.end_coords = Some((tile_idx % 16, tile_idx / 16));
    state.selection_source = SelectionSource::Tileset;
    state.focus = Focus::TilesetTile;
    Ok(())
}

// Avoid processing the same messages multiple times (e.g. when brushing/pasting and
// dragging with the mouse). This helps limit memory usage in the undo stack and
// makes it behave more like how users would expect.
fn should_debounce(message: &Message, last_message: &Message) -> bool {
    match message {
        Message::BrushColor {
            palette_id,
            color_idx,
            color,
        } => match last_message {
            Message::BrushColor {
                palette_id: last_palette_id,
                color_idx: last_color_idx,
                color: last_color,
            } => {
                return palette_id == last_palette_id
                    && color_idx == last_color_idx
                    && color == last_color;
            }
            _ => false,
        },
        Message::BrushPixel {
            palette_id,
            tile_idx,
            coords,
            color_idx,
        } => match last_message {
            Message::BrushPixel {
                palette_id: last_palette_id,
                tile_idx: last_tile_idx,
                coords: last_coords,
                color_idx: last_color_idx,
            } => {
                palette_id == last_palette_id
                    && tile_idx == last_tile_idx
                    && coords == last_coords
                    && color_idx == last_color_idx
            }
            _ => false,
        },
        Message::TilesetBrush {
            palette_id,
            coords,
            selected_gfx,
            tile_block,
        } => match last_message {
            Message::TilesetBrush {
                palette_id: last_palette_id,
                coords: last_coords,
                selected_gfx: last_selected_gfx,
                tile_block: last_tile_block,
            } => {
                palette_id == last_palette_id
                    && coords == last_coords
                    && selected_gfx == last_selected_gfx
                    && tile_block == last_tile_block
            }
            _ => false,
        },
        Message::AreaBrush {
            position,
            area_id,
            coords,
            selection,
            palette_only,
        } => match last_message {
            Message::AreaBrush {
                position: last_position,
                area_id: last_area_id,
                coords: last_coords,
                selection: last_selection,
                palette_only: last_palette_only,
            } => {
                position == last_position
                    && area_id == last_area_id
                    && coords == last_coords
                    && selection == last_selection
                    && palette_only == last_palette_only
            }
            _ => false,
        },
        _ => false,
    }
}

pub fn try_update(state: &mut EditorState, message: &Message) -> Result<Option<Task<Message>>> {
    if state.global_config.project_dir.is_none() {
        let Message::ProjectOpened(_) = &message else {
            return Ok(None);
        };
    }
    match message {
        Message::Nothing => {}
        Message::Event(event) => match event {
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::Tab),
                modifiers,
                ..
            }) => {
                if modifiers.shift() {
                    return Ok(Some(widget::focus_previous()));
                } else {
                    return Ok(Some(widget::focus_next()));
                }
            }
            Event::Keyboard(keyboard::Event::ModifiersChanged(modifiers)) => {
                state.shift_brush = modifiers.shift();
                match state.focus {
                    Focus::None => {}
                    Focus::PickArea(_) => {}
                    Focus::PickTheme(_) => {}
                    Focus::Area(_) => {
                        state.identify_tile = modifiers.control();
                    }
                    Focus::PickPalette => {}
                    Focus::PaletteColor | Focus::GraphicsPixel => {
                        state.identify_color = modifiers.control();
                    }
                    Focus::TilesetTile => {
                        state.identify_tile = modifiers.control();
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::Escape),
                ..
            }) => {
                state.tool = Tool::Select;
                state.dialogue = None;
                state.color_idx = None;
                state.tile_idx = None;
                state.selected_gfx = vec![];
                state.start_coords = None;
                state.end_coords = None;
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::ArrowRight),
                ..
            }) => {
                match state.focus {
                    Focus::None => {}
                    Focus::PickArea(_) => {}
                    Focus::PickTheme(_) => {}
                    Focus::Area(_) => {
                        // TODO: Handle making selections with keyboard:
                    }
                    Focus::PickPalette => {}
                    Focus::PaletteColor => {
                        if let Some(idx) = state.color_idx {
                            if idx < 15 {
                                let new_idx = idx + 1;
                                state.color_idx = Some(new_idx);
                                state.selected_color =
                                    state.palettes[state.palette_idx].colors[new_idx as usize];
                            }
                        }
                    }
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.0 < 7 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    coords.0 + 1,
                                    coords.1,
                                ))));
                            }
                        }
                    }
                    Focus::TilesetTile => {
                        if let Some(idx) = state.tile_idx {
                            if (idx as usize) + 1 < state.palettes[state.palette_idx].tiles.len() {
                                let new_idx = idx + 1;
                                select_tileset_tile(state, new_idx)?;
                            }
                        }
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::ArrowLeft),
                ..
            }) => {
                match state.focus {
                    Focus::None => {}
                    Focus::PickArea(_) => {}
                    Focus::PickTheme(_) => {}
                    Focus::Area(_) => {
                        // TODO: Handle making selections with keyboard:
                    }
                    Focus::PickPalette => {}
                    Focus::PaletteColor => {
                        if let Some(idx) = state.color_idx {
                            if idx > 0 {
                                let new_idx = idx - 1;
                                state.color_idx = Some(new_idx);
                                state.selected_color =
                                    state.palettes[state.palette_idx].colors[new_idx as usize];
                            }
                        }
                    }
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.0 > 0 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    coords.0 - 1,
                                    coords.1,
                                ))));
                            }
                        }
                    }
                    Focus::TilesetTile => {
                        if let Some(idx) = state.tile_idx {
                            if idx > 0 {
                                let new_idx = idx - 1;
                                select_tileset_tile(state, new_idx)?;
                            }
                        }
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::ArrowDown),
                ..
            }) => {
                match state.focus {
                    Focus::None => {}
                    Focus::PickArea(position) => {
                        let area_id = state.area_id(position);
                        if let Some(area_idx) =
                            state.area_names.iter().position(|x| x == &area_id.area)
                        {
                            if area_idx + 1 < state.area_names.len() {
                                return Ok(Some(Task::done(Message::SelectArea(
                                    position,
                                    state.area_names[area_idx + 1].clone(),
                                ))));
                            }
                        } else {
                            bail!("Area not found: {}", area_id.area);
                        }
                    }
                    Focus::PickTheme(position) => {
                        let area_id = state.area_id(position);
                        if let Some(theme_idx) =
                            state.theme_names.iter().position(|x| x == &area_id.theme)
                        {
                            if theme_idx + 1 < state.theme_names.len() {
                                return Ok(Some(Task::done(Message::SelectTheme(
                                    position,
                                    state.theme_names[theme_idx + 1].clone(),
                                ))));
                            }
                        } else {
                            bail!("Area not found: {}", area_id.area);
                        }
                    }
                    Focus::Area(_) => {
                        // TODO: Handle making selections with keyboard:
                    }
                    Focus::PickPalette => {
                        if state.palette_idx + 1 < state.palettes.len() {
                            state.palette_idx += 1;
                            state.color_idx = None;
                            state.tile_idx = None;
                        }
                    }
                    Focus::PaletteColor => {}
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.1 < 7 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    coords.0,
                                    coords.1 + 1,
                                ))));
                            }
                        }
                    }
                    Focus::TilesetTile => {
                        if let Some(idx) = state.tile_idx {
                            if (idx as usize) + 16 < state.palettes[state.palette_idx].tiles.len() {
                                let new_idx = idx + 16;
                                select_tileset_tile(state, new_idx)?;
                            }
                        }
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                key: keyboard::Key::Named(key::Named::ArrowUp),
                ..
            }) => {
                match state.focus {
                    Focus::None => {}
                    Focus::PickArea(position) => {
                        let area_id = state.area_id(position);
                        if let Some(area_idx) =
                            state.area_names.iter().position(|x| x == &area_id.area)
                        {
                            if area_idx > 0 {
                                return Ok(Some(Task::done(Message::SelectArea(
                                    position,
                                    state.area_names[area_idx - 1].clone(),
                                ))));
                            }
                        } else {
                            bail!("Area not found: {}", area_id.area);
                        }
                    }
                    Focus::PickTheme(position) => {
                        let area_id = state.area_id(position);
                        if let Some(theme_idx) =
                            state.theme_names.iter().position(|x| x == &area_id.theme)
                        {
                            if theme_idx > 0 {
                                return Ok(Some(Task::done(Message::SelectTheme(
                                    position,
                                    state.theme_names[theme_idx - 1].clone(),
                                ))));
                            }
                        } else {
                            bail!("Area not found: {}", area_id.area);
                        }
                    }
                    Focus::Area(_) => {
                        // TODO: Handle making selections with keyboard:
                    }
                    Focus::PickPalette => {
                        if state.palette_idx > 0 {
                            state.palette_idx -= 1;
                            state.color_idx = None;
                            state.tile_idx = None;
                        }
                    }
                    Focus::PaletteColor => {}
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.1 > 0 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    coords.0,
                                    coords.1 - 1,
                                ))));
                            }
                        }
                    }
                    Focus::TilesetTile => {
                        if let Some(idx) = state.tile_idx {
                            if (idx as usize) >= 16 {
                                let new_idx = idx - 16;
                                select_tileset_tile(state, new_idx)?;
                            }
                        }
                    }
                }
            }
            Event::Keyboard(keyboard::Event::KeyPressed {
                modified_key: keyboard::Key::Character(c),
                modifiers,
                ..
            }) => {
                if modifiers.control() {
                    match c.as_str() {
                        "r" => {
                            return Ok(Some(Task::done(Message::RebuildProjectDialogue)));
                        }
                        _ => {}
                    }
                } else {
                    match c.as_str() {
                        "b" => {
                            state.tool = Tool::Brush;
                        }
                        "s" => {
                            state.tool = Tool::Select;
                        }
                        "m" => {
                            state.tool = Tool::Move;
                        }
                        "g" => {
                            state.show_grid_16 = !state.show_grid_16;
                        }
                        "p" => {
                            state.snap_grid_16 = !state.snap_grid_16;
                        }
                        "t" => {
                            state.side_panel_view = SidePanelView::Tileset;
                        }
                        "a" => {
                            state.side_panel_view = SidePanelView::Area;
                        }
                        "h" => {
                            for i in 0..state.selected_tile_block.size.1 as usize {
                                state.selected_tile_block.palettes[i].reverse();
                                state.selected_tile_block.tiles[i].reverse();
                                state.selected_tile_block.flips[i].reverse();
                                state.selected_gfx[i].reverse();
                                for j in 0..state.selected_tile_block.size.0 as usize {
                                    state.selected_tile_block.flips[i][j] =
                                        state.selected_tile_block.flips[i][j].flip_horizontally();
                                    state.selected_gfx[i][j] =
                                        Flip::Horizontal.apply_to_tile(state.selected_gfx[i][j]);
                                }
                            }
                        }
                        "v" => {
                            state.selected_tile_block.palettes.reverse();
                            state.selected_tile_block.tiles.reverse();
                            state.selected_tile_block.flips.reverse();
                            state.selected_gfx.reverse();
                            for i in 0..state.selected_tile_block.size.1 as usize {
                                for j in 0..state.selected_tile_block.size.0 as usize {
                                    state.selected_tile_block.flips[i][j] =
                                        state.selected_tile_block.flips[i][j].flip_vertically();
                                    state.selected_gfx[i][j] =
                                        Flip::Vertical.apply_to_tile(state.selected_gfx[i][j]);
                                }
                            }
                        }
                        "-" => {
                            state.global_config.pixel_size =
                                (state.global_config.pixel_size - 1.0).max(MIN_PIXEL_SIZE);
                        }
                        "=" => {
                            state.global_config.pixel_size =
                                (state.global_config.pixel_size + 1.0).min(MAX_PIXEL_SIZE);
                        }
                        _ => {}
                    }
                }
            }
            _ => {}
        },
        &Message::Focus(focus) => {
            state.focus = focus;
        }
        Message::SaveProject => {
            if *state.files_modified_notification.lock().unwrap() {
                *state.files_modified_notification.lock().unwrap() = false;
                state.dialogue = Some(Dialogue::ModifiedReload);
            } else {
                persist::save_project(state)?;
            }
        }
        Message::OpenProject => {
            return Ok(Some(Task::perform(open_project(), Message::ProjectOpened)));
        }
        Message::ModifiedReload => {
            persist::load_project(state)?;
            state.dialogue = None;
        }
        Message::RebuildProjectDialogue => {
            state.dialogue = Some(Dialogue::RebuildProject);
            return Ok(Some(Task::done(Message::RebuildProject)));
        }
        Message::RebuildProject => {
            // Save all area PNGs (which could be out-of-date, e.g. if a palette were updated or a new theme created)
            state.disable_watch_file_changes()?;
            for theme in &state.theme_names.clone() {
                for area_name in &state.area_names.clone() {
                    let area_id = AreaId {
                        theme: theme.clone(),
                        area: area_name.clone(),
                    };
                    if state.areas.contains_key(&area_id) {
                        save_area_png(state, &area_id)?;
                    } else {
                        state.load_area(&area_id)?;
                        save_area_png(state, &area_id)?;
                        state.areas.remove(&area_id);
                    }
                }
            }
            state.enable_watch_file_changes()?;
            state.dialogue = None;
        }
        &Message::WindowClose(id) => {
            persist::save_project(state)?;
            return Ok(Some(window::close(id)));
        }
        Message::ProjectOpened(path) => {
            match path {
                Some(p) => {
                    info!("Opening project at {}", p.display());
                    // Ensure that the old project has been persisted before loading the new:
                    if state.global_config.project_dir.is_some() {
                        persist::save_project(state)?;
                    }

                    // Update the global config to be set to the new project:
                    state.global_config.project_dir = Some(p.clone());
                    state.global_config.modified = true;
                    persist::save_global_config(state)?;
                    persist::load_project(state)?;
                    state.dialogue = None;
                }
                None => {
                    if state.global_config.project_dir.is_none() {
                        info!("Project path not selected, exiting.");
                        std::process::exit(1);
                    }
                }
            }
        }
        Message::SettingsDialogue => {
            state.dialogue = Some(Dialogue::Settings);
        }
        Message::HelpDialogue => {
            state.dialogue = Some(Dialogue::Help);
        }
        &Message::SetPixelSize(pixel_size) => {
            state.global_config.pixel_size = pixel_size;
            state.global_config.modified = true;
        }
        &Message::SetGridAlpha(grid_alpha) => {
            state.global_config.grid_alpha = grid_alpha;
            state.global_config.modified = true;
        }
        Message::CloseDialogue => {
            state.dialogue = None;
        }
        Message::ImportDialogue => {
            return Ok(Some(Task::perform(open_rom(), Message::ImportConfirm)));
        }
        Message::ImportConfirm(path) => {
            if path.is_some() {
                state.rom_path = path.clone();
                state.dialogue = Some(Dialogue::ImportROMConfirm);
            } else {
                state.dialogue = Some(Dialogue::Settings);
            }
        }
        Message::ImportROMProgress => {
            state.dialogue = Some(Dialogue::ImportROMProgress);
            return Ok(Some(Task::done(Message::ImportROM)));
        }
        Message::ImportROM => {
            let path = state.rom_path.as_ref().context("internal error")?;
            Importer::import(state, &path.clone())?;
            state.dialogue = None;
        }
        Message::SelectPalette(name) => {
            for i in 0..state.palettes.len() {
                if name == &format!("{}: {}", state.palettes[i].id, state.palettes[i].name) {
                    state.palette_idx = i;
                    state.color_idx = None;
                    state.tile_idx = None;
                    break;
                }
            }
        }
        Message::AddPaletteDialogue => {
            let id = state.palettes.iter().map(|x| x.id).max().unwrap() + 1;
            state.dialogue = Some(Dialogue::AddPalette {
                name: "".to_string(),
                id,
            });
            return Ok(Some(iced::widget::text_input::focus("AddPalette")));
        }
        Message::SetAddPaletteName(new_name) => match &mut state.dialogue {
            Some(Dialogue::AddPalette { name, .. }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        &Message::SetAddPaletteID(new_id) => match &mut state.dialogue {
            Some(Dialogue::AddPalette { id, .. }) => {
                *id = new_id;
            }
            _ => {}
        },
        Message::AddPalette { name, id } => {
            if name.len() == 0 {
                warn!("Empty palette name is invalid.");
                return Ok(None);
            }
            for p in state.palettes.iter() {
                if &p.name == name {
                    // Don't add non-unique palette name.
                    warn!("Palette name {} already exists.", name);
                    return Ok(None);
                }
                if p.id == *id {
                    // Don't add non-unique palette ID.
                    warn!("Palette ID {} already exists.", id);
                    return Ok(None);
                }
            }
            let mut pal = state.palettes[state.palette_idx].clone();
            pal.name = name.clone();
            pal.id = *id;
            pal.modified = true;
            state.palettes.push(pal);
            state.palette_idx = state.palettes.len() - 1;
            update_palette_order(state);
            state.dialogue = None;
        }
        Message::RenamePaletteDialogue => {
            state.dialogue = Some(Dialogue::RenamePalette {
                name: "".to_string(),
            });
            return Ok(Some(iced::widget::text_input::focus("RenamePalette")));
        }
        Message::SetRenamePaletteName(new_name) => match &mut state.dialogue {
            Some(Dialogue::RenamePalette { name }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        Message::RenamePalette { id: _, name } => {
            if name.len() == 0 {
                warn!("Empty palette name is invalid.");
                return Ok(None);
            }
            for p in state.palettes.iter() {
                if &p.name == name {
                    // Don't add non-unique palette name.
                    warn!("Palette name {} already exists.", name);
                    return Ok(None);
                }
            }

            let name = name.clone();
            let old_name = state.palettes[state.palette_idx].name.clone();
            state.palettes[state.palette_idx].name = name.clone();
            state.palettes[state.palette_idx].modified = true;
            persist::save_project(state)?;
            delete_palette(state, &old_name)?;
            update_palette_order(state);
            state.dialogue = None;
        }
        Message::DeletePaletteDialogue => {
            state.dialogue = Some(Dialogue::DeletePalette);
        }
        &Message::DeletePalette(id) => {
            if state.palettes.len() == 1 {
                warn!("Not allowed to delete the last palette.");
                return Ok(None);
            }

            let palette_idx = *state
                .palettes_id_idx_map
                .get(&id)
                .context("palette not found")?;
            let name = state.palettes[palette_idx].name.clone();
            persist::delete_palette(state, &name)?;
            if state.palette_idx < state.palettes.len() {
                state.palettes.remove(palette_idx);
                if state.palette_idx == state.palettes.len() {
                    state.palette_idx -= 1;
                }
            }
            update_palette_order(state);
            state.dialogue = None;
        }
        Message::RestorePalette(palette) => {
            let mut pal = palette.clone();
            pal.modified = true;
            state.palettes.push(pal);
            state.palette_idx = state.palettes.len() - 1;
            update_palette_order(state);
        }
        Message::HideModal => {
            state.dialogue = None;
        }
        &Message::SelectColor(pal_idx, color_idx) => {
            if pal_idx != state.palette_idx {
                state.tile_idx = None;
                state.start_coords = None;
                state.end_coords = None;
            }
            state.palette_idx = pal_idx;
            state.color_idx = Some(color_idx);
            state.selected_color = state.palettes[pal_idx as usize].colors[color_idx as usize];
            state.focus = Focus::PaletteColor;
        }
        &Message::BrushColor {
            palette_id,
            color_idx,
            color,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            state.palettes[pal_idx].colors[color_idx as usize] = color;
            state.palettes[pal_idx].modified = true;
        }
        &Message::ChangeRed(c) => {
            if let Some(color_idx) = state.color_idx {
                let pal_idx = state.palette_idx;
                let palette_id = state.palettes[pal_idx].id;
                state.selected_color[0] = c;
                return Ok(Some(Task::done(Message::BrushColor {
                    palette_id,
                    color_idx: color_idx,
                    color: state.selected_color,
                })));
            }
        }
        &Message::ChangeGreen(c) => {
            if let Some(color_idx) = state.color_idx {
                let pal_idx = state.palette_idx;
                let palette_id = state.palettes[pal_idx].id;
                state.selected_color[1] = c;
                return Ok(Some(Task::done(Message::BrushColor {
                    palette_id,
                    color_idx: color_idx,
                    color: state.selected_color,
                })));
            }
        }
        &Message::ChangeBlue(c) => {
            if let Some(color_idx) = state.color_idx {
                let pal_idx = state.palette_idx;
                let palette_id = state.palettes[pal_idx].id;
                state.selected_color[2] = c;
                return Ok(Some(Task::done(Message::BrushColor {
                    palette_id,
                    color_idx: color_idx,
                    color: state.selected_color,
                })));
            }
        }
        Message::AddTileRow(palette_id) => {
            let idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            state.palettes[idx].tiles.extend(vec![Tile::default(); 16]);
            state.palettes[idx].modified = true;
        }
        Message::DeleteTileRow(palette_id) => {
            let idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            if state.palettes[idx].tiles.len() <= 16 {
                warn!("Not allowed to delete the last row of tiles.");
                return Ok(None);
            }
            let new_size = state.palettes[state.palette_idx].tiles.len() - 16;
            state.palettes[state.palette_idx]
                .tiles
                .resize(new_size, Tile::default());
            if let Some(idx) = state.tile_idx {
                if idx >= new_size as TileIdx {
                    state.tile_idx = Some(new_size as TileIdx - 1);
                }
            }
            state.palettes[state.palette_idx].modified = true;
        }
        Message::RestoreTileRow(palette_id, tiles) => {
            let idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            state.palettes[idx].tiles.extend(tiles);
            state.palettes[idx].modified = true;
        }
        &Message::SetTilePriority {
            palette_id,
            tile_idx,
            priority,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            state.palettes[pal_idx].tiles[tile_idx as usize].priority = priority;
            state.palettes[pal_idx].modified = true;
        }
        &Message::SetTileCollision {
            palette_id,
            tile_idx,
            collision,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            state.palettes[pal_idx].tiles[tile_idx as usize].collision = collision;
            state.palettes[pal_idx].modified = true;
        }
        &Message::SetTileHFlippable {
            palette_id,
            tile_idx,
            h_flippable,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            state.palettes[pal_idx].tiles[tile_idx as usize].h_flippable = h_flippable;
            state.palettes[pal_idx].modified = true;
        }
        &Message::SetTileVFlippable {
            palette_id,
            tile_idx,
            v_flippable,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            state.palettes[pal_idx].tiles[tile_idx as usize].v_flippable = v_flippable;
            state.palettes[pal_idx].modified = true;
        }
        &Message::TilesetBrush {
            palette_id,
            coords: Point { x: x0, y: y0 },
            selected_gfx: ref s,
            ref tile_block,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            let mut color_map: HashMap<ColorRGB, ColorIdx> = HashMap::new();
            if tile_block.is_some() {
                for (i, &c) in state.palettes[pal_idx].colors.iter().enumerate().skip(1) {
                    color_map.insert(c, i as ColorIdx);
                }
            }
            for y in 0..s.len() {
                for x in 0..s[0].len() {
                    let y1 = y + y0 as usize;
                    let x1 = x + x0 as usize;
                    let i = y1 * 16 + x1;
                    if x1 < 16 && i < state.palettes[pal_idx].tiles.len() {
                        let mut tile = s[y as usize][x as usize];
                        if let Some(t) = tile_block {
                            let src_pal_id = t.palettes[y][x];
                            if src_pal_id == palette_id {
                                continue;
                            }
                            let src_pal_idx = state.palettes_id_idx_map[&src_pal_id];
                            let src_pal = &state.palettes[src_pal_idx];
                            for py in 0..8 {
                                for px in 0..8 {
                                    let src_color_idx = tile.pixels[py][px] as usize;
                                    if src_color_idx == 0 {
                                        continue;
                                    }
                                    let color = src_pal.colors[src_color_idx];
                                    if let Some(&color_idx) = color_map.get(&color) {
                                        tile.pixels[py][px] = color_idx;
                                    }
                                }
                            }
                        }
                        state.palettes[pal_idx].tiles[i] = tile;
                    }
                }
            }
            state.palettes[pal_idx].modified = true;
        }
        &Message::SelectPixel(x, y) => {
            state.pixel_coords = Some((x, y));
            if let Some(tile_idx) = state.tile_idx {
                let pal = &mut state.palettes[state.palette_idx];
                let color_idx = pal.tiles[tile_idx as usize].pixels[y as usize][x as usize];
                state.color_idx = Some(color_idx);
                state.selected_color = pal.colors[color_idx as usize];
                state.focus = Focus::GraphicsPixel;
            }
        }
        &Message::BrushPixel {
            palette_id,
            tile_idx,
            coords,
            color_idx,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            let pal = &mut state.palettes[pal_idx];
            pal.tiles[tile_idx as usize].pixels[coords.y as usize][coords.x as usize] = color_idx;
            pal.modified = true;
        }
        &Message::SelectArea(position, ref name) => {
            let area_id = &state.main_area_id;
            state.switch_area(
                position,
                &AreaId {
                    area: name.clone(),
                    theme: area_id.theme.clone(),
                },
            )?;
            if let SelectionSource::Area(p) = state.selection_source {
                if p == position {
                    state.start_coords = None;
                    state.end_coords = None;
                }
            }
        }
        Message::AddAreaDialogue => {
            state.dialogue = Some(Dialogue::AddArea {
                name: "".to_string(),
                size: (2, 2),
            });
            return Ok(Some(iced::widget::text_input::focus("AddArea")));
        }
        Message::SetAddAreaName(new_name) => match &mut state.dialogue {
            Some(Dialogue::AddArea { name, .. }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        &Message::SetAddAreaSizeX(new_x) => match &mut state.dialogue {
            Some(Dialogue::AddArea { size, .. }) => {
                size.0 = new_x;
            }
            _ => {}
        },
        &Message::SetAddAreaSizeY(new_y) => match &mut state.dialogue {
            Some(Dialogue::AddArea { size, .. }) => {
                size.1 = new_y;
            }
            _ => {}
        },
        Message::AddArea { name, size } => {
            if name.len() == 0 {
                warn!("Empty area name is invalid.");
                return Ok(None);
            }
            for s in &state.area_names {
                if s == name {
                    // Don't add a non-unique area name.
                    warn!("Area name {} already exists.", name);
                    return Ok(None);
                }
            }
            for theme in state.theme_names.clone() {
                state.set_area(
                    AreaPosition::Main,
                    Area {
                        modified: true,
                        name: name.clone(),
                        theme,
                        size: *size,
                        vanilla_map_id: state.areas[&state.main_area_id].vanilla_map_id,
                        bg_color: state.areas[&state.main_area_id].bg_color,
                        screens: (0..size.0)
                            .cartesian_product(0..size.1)
                            .map(|(x, y)| Screen {
                                position: (x, y),
                                palettes: [[0; 32]; 32],
                                tiles: [[0; 32]; 32],
                                flips: [[Flip::None; 32]; 32],
                            })
                            .collect(),
                    },
                )?;
                save_area(state, &state.main_area_id.clone())?;
            }
            state.dialogue = None;
            state.area_names.push(name.clone());
            state.area_names.sort();
        }
        Message::EditAreaDialogue => {
            state.dialogue = Some(Dialogue::EditArea {
                name: state.main_area_id.area.clone(),
            });
            return Ok(Some(iced::widget::text_input::focus("EditArea")));
        }
        Message::SetEditAreaName(new_name) => match &mut state.dialogue {
            Some(Dialogue::EditArea { name }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        Message::EditArea { old_name, new_name } => {
            if new_name.len() == 0 {
                warn!("Empty area name is invalid.");
                return Ok(None);
            }
            for s in &state.area_names {
                if s == new_name && s != old_name {
                    // Don't add a non-unique area name.
                    warn!("Area name {} already exists.", new_name);
                    return Ok(None);
                }
            }
            let area_id = state.main_area_id.clone();
            if new_name != old_name {
                rename_area(state, old_name, new_name)?;
                load_area_list(state)?;
                if &state.main_area_id.area == old_name {
                    state.switch_area(
                        AreaPosition::Main,
                        &AreaId {
                            area: new_name.clone(),
                            theme: area_id.theme.clone(),
                        },
                    )?;
                }
                if &state.side_area_id.area == old_name {
                    state.switch_area(
                        AreaPosition::Side,
                        &AreaId {
                            area: new_name.clone(),
                            theme: area_id.theme.clone(),
                        },
                    )?;
                }
            }
            state.dialogue = None;
        }
        &Message::EditAreaBGRed(c) => {
            let mut color = state.main_area().bg_color;
            color[0] = c;
            return Ok(Some(Task::done(Message::EditAreaBGColor {
                area_id: state.area_id(AreaPosition::Main).clone(),
                color,
            })));
        }
        &Message::EditAreaBGGreen(c) => {
            let mut color = state.main_area().bg_color;
            color[1] = c;
            return Ok(Some(Task::done(Message::EditAreaBGColor {
                area_id: state.area_id(AreaPosition::Main).clone(),
                color,
            })));
        }
        &Message::EditAreaBGBlue(c) => {
            let mut color = state.main_area().bg_color;
            color[2] = c;
            return Ok(Some(Task::done(Message::EditAreaBGColor {
                area_id: state.area_id(AreaPosition::Main).clone(),
                color,
            })));
        }
        &Message::EditAreaBGColor { ref area_id, color } => {
            state.switch_area(AreaPosition::Main, area_id)?;
            state.main_area_mut().bg_color = color;
        }
        Message::DeleteAreaDialogue => {
            state.dialogue = Some(Dialogue::DeleteArea);
        }
        Message::DeleteArea(name) => {
            if state.area_names.len() == 1 {
                warn!("Not allowed to delete the last remaining area.");
                return Ok(None);
            }
            let theme = state.main_area().theme.clone();
            delete_area(state, name)?;
            load_area_list(state)?;
            if &state.main_area_id.area == name {
                state.switch_area(
                    AreaPosition::Main,
                    &AreaId {
                        area: state.area_names[0].clone(),
                        theme: theme.clone(),
                    },
                )?;
            }
            if &state.side_area_id.area == name {
                state.switch_area(
                    AreaPosition::Side,
                    &AreaId {
                        area: state.area_names[0].clone(),
                        theme: theme.clone(),
                    },
                )?;
            }
            state.dialogue = None;
        }
        &Message::SelectTheme(position, ref theme) => {
            state.switch_area(
                position,
                &AreaId {
                    area: state.area(position).name.clone(),
                    theme: theme.clone(),
                },
            )?;
        }
        Message::AddThemeDialogue => {
            state.dialogue = Some(Dialogue::AddTheme {
                name: "".to_string(),
            });
            return Ok(Some(iced::widget::text_input::focus("AddTheme")));
        }
        Message::SetAddThemeName(new_name) => match &mut state.dialogue {
            Some(Dialogue::AddTheme { name }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        Message::AddTheme(theme_name) => {
            if theme_name.len() == 0 {
                warn!("Empty theme name is invalid.");
                return Ok(None);
            }
            for t in &state.theme_names {
                if t == theme_name {
                    // Don't add a non-unique theme name.
                    warn!("Theme name {} already exists.", theme_name);
                    return Ok(None);
                }
            }
            let old_theme = state.main_area().theme.clone();
            for area_name in &state.area_names.clone() {
                copy_area_theme(state, area_name, &old_theme, &theme_name)?;
            }
            state.switch_area(
                AreaPosition::Main,
                &AreaId {
                    area: state.main_area().name.clone(),
                    theme: theme_name.clone(),
                },
            )?;
            state.theme_names.push(theme_name.clone());
            state.theme_names.sort();
            state.dialogue = None;
        }
        Message::RenameThemeDialogue => {
            state.dialogue = Some(Dialogue::RenameTheme {
                name: state.main_area().theme.clone(),
            });
            return Ok(Some(iced::widget::text_input::focus("RenameTheme")));
        }
        Message::SetRenameThemeName(new_name) => match &mut state.dialogue {
            Some(Dialogue::RenameTheme { name }) => {
                *name = new_name.clone();
            }
            _ => {}
        },
        Message::RenameTheme { old_name, new_name } => {
            if new_name.len() == 0 {
                warn!("Empty theme name is invalid.");
                return Ok(None);
            }
            for t in &state.theme_names {
                if t == new_name {
                    // Don't add a non-unique theme name.
                    warn!("Theme name {} already exists.", new_name);
                    return Ok(None);
                }
            }
            for area_name in &state.area_names.clone() {
                rename_area_theme(state, area_name, old_name, new_name)?;
            }
            load_area_list(state)?;
            if &state.main_area_id.theme == old_name {
                state.switch_area(
                    AreaPosition::Main,
                    &AreaId {
                        area: state.main_area().name.clone(),
                        theme: new_name.clone(),
                    },
                )?;
            }
            if &state.side_area_id.theme == old_name {
                state.switch_area(
                    AreaPosition::Side,
                    &AreaId {
                        area: state.main_area().name.clone(),
                        theme: new_name.clone(),
                    },
                )?;
            }
            state.dialogue = None;
        }
        Message::DeleteThemeDialogue => {
            state.dialogue = Some(Dialogue::DeleteTheme);
        }
        Message::DeleteTheme(theme_name) => {
            if state.theme_names.len() == 1 {
                warn!("Not allowed to delete the last remaining theme.");
                return Ok(None);
            }
            let area = state.main_area().name.clone();
            for area_name in &state.area_names.clone() {
                delete_area_theme(state, area_name, theme_name)?;
            }
            load_area_list(state)?;
            if &state.main_area_id.theme == theme_name {
                state.switch_area(
                    AreaPosition::Main,
                    &AreaId {
                        area: area.clone(),
                        theme: state.theme_names[0].clone(),
                    },
                )?;
            }
            if &state.side_area_id.theme == theme_name {
                state.switch_area(
                    AreaPosition::Side,
                    &AreaId {
                        area: area.clone(),
                        theme: state.theme_names[0].clone(),
                    },
                )?;
            }
            state.dialogue = None;
        }
        &Message::StartTileSelection(p, source) => {
            state.selection_source = source;
            state.start_coords = Some((p.x, p.y));
            state.end_coords = Some((p.x, p.y));
        }
        Message::ProgressTileSelection(p) => {
            state.end_coords = Some((p.x, p.y));
        }
        Message::EndTileSelection(p1) => {
            let p1 = (p1.x, p1.y);
            let Some(p0) = state.start_coords else {
                return Ok(None);
            };

            let left = p0.0.min(p1.0);
            let mut right = p0.0.max(p1.0);
            let top = p0.1.min(p1.1);
            let mut bottom = p0.1.max(p1.1);

            if state.snap_grid_16 {
                right += 1;
                bottom += 1;
            }

            match state.selection_source {
                SelectionSource::Area(position) => {
                    state.focus = Focus::Area(position);
                }
                SelectionSource::Tileset => {
                    state.focus = Focus::TilesetTile;
                }
            }

            let mut palettes: Vec<Vec<PaletteId>> = vec![];
            let mut tiles: Vec<Vec<TileIdx>> = vec![];
            let mut flips: Vec<Vec<Flip>> = vec![];
            for y in top..=bottom {
                let mut pal_row: Vec<PaletteId> = vec![];
                let mut tile_row: Vec<TileIdx> = vec![];
                let mut flip_row: Vec<Flip> = vec![];
                for x in left..=right {
                    match state.selection_source {
                        SelectionSource::Area(position) => {
                            pal_row.push(state.area(position).get_palette(x, y)?);
                            tile_row.push(state.area(position).get_tile(x, y)?);
                            flip_row.push(state.area(position).get_flip(x, y)?);
                        }
                        SelectionSource::Tileset => {
                            pal_row.push(state.palettes[state.palette_idx].id);
                            tile_row.push(y * 16 + x);
                            flip_row.push(Flip::None)
                        }
                    }
                }
                palettes.push(pal_row);
                tiles.push(tile_row);
                flips.push(flip_row);
            }
            state.selected_tile_block = TileBlock {
                size: (right - left + 1, bottom - top + 1),
                palettes,
                tiles,
                flips,
            };
            let s = &state.selected_tile_block;

            state.selected_gfx = get_selected_gfx(state, &state.selected_tile_block);
            state.start_coords = None;
            state.end_coords = None;
            if left == right && top == bottom {
                return Ok(Some(Task::done(Message::OpenTile {
                    palette_id: s.palettes[0][0],
                    tile_idx: s.tiles[0][0],
                })));
            } else {
                state.tile_idx = None;
            }
        }
        &Message::AreaBrush {
            position,
            ref area_id,
            coords,
            ref selection,
            palette_only,
        } => {
            state.switch_area(position, area_id)?;
            let s = selection;
            let p = coords;
            let area = state.area_mut(position);
            for y in 0..s.size.1 {
                for x in 0..s.size.0 {
                    let _ = area.set_palette(p.x + x, p.y + y, s.palettes[y as usize][x as usize]);
                    if !palette_only {
                        let _ = area.set_tile(p.x + x, p.y + y, s.tiles[y as usize][x as usize]);
                        let _ = area.set_flip(p.x + x, p.y + y, s.flips[y as usize][x as usize]);
                    }
                }
            }
            area.modified = true;
        }
        &Message::OpenTile {
            palette_id,
            tile_idx,
        } => {
            if let Some(&palette_idx) = state.palettes_id_idx_map.get(&palette_id) {
                state.palette_idx = palette_idx;
                state.tile_idx = Some(tile_idx);
            }
        }
        Message::MovingTilesProgress {
            src_selection,
            dst_selection,
            check_reversible,
        } => {
            state.dialogue = Some(Dialogue::MovingTilesProgress);
            return Ok(Some(Task::done(Message::MoveTiles {
                src_selection: src_selection.clone(),
                dst_selection: dst_selection.clone(),
                check_reversible: *check_reversible,
            })));
        }
        Message::MoveTiles {
            src_selection,
            dst_selection,
            check_reversible,
        } => {
            assert!(src_selection.size == dst_selection.size);
            let mut mapping: HashMap<(PaletteId, TileIdx), (PaletteId, TileIdx, Flip)> =
                HashMap::new();

            // Validate that the selected tiles are unique, and create the mapping:
            for y in 0..src_selection.size.1 {
                for x in 0..src_selection.size.0 {
                    let src_palette_id = src_selection.palettes[y as usize][x as usize];
                    let src_tile_idx = src_selection.tiles[y as usize][x as usize];
                    let src_flip = src_selection.flips[y as usize][x as usize];
                    let dst_palette_id = dst_selection.palettes[y as usize][x as usize];
                    let dst_tile_idx = dst_selection.tiles[y as usize][x as usize];
                    let dst_flip = dst_selection.flips[y as usize][x as usize];
                    if mapping.contains_key(&(src_palette_id, src_tile_idx)) {
                        warn!("Not moving tiles: palette {} tile number {} (${:x}) occurs twice in selection",
                                src_palette_id, src_tile_idx, src_tile_idx);
                        return Ok(None);
                    }
                    mapping.insert(
                        (src_palette_id, src_tile_idx),
                        (
                            dst_palette_id,
                            dst_tile_idx,
                            dst_flip.apply_to_flip(src_flip),
                        ),
                    );
                }
            }

            // Validate that the source and destination tiles are disjoint:
            for y in 0..dst_selection.size.1 {
                for x in 0..dst_selection.size.0 {
                    let dst_palette_id = dst_selection.palettes[y as usize][x as usize];
                    let dst_tile_idx = dst_selection.tiles[y as usize][x as usize];
                    if mapping.contains_key(&(dst_palette_id, dst_tile_idx)) {
                        warn!("Not moving tiles: palette {} tile number {} (${:x}) occurs in both the source and destination",
                                dst_palette_id, dst_tile_idx, dst_tile_idx);
                        return Ok(None);
                    }
                }
            }

            if *check_reversible {
                // Ensure that the destination tiles are unused, so that
                // the operation will be reversible:
                let used_tiles = scan_used_tiles(state)?;
                for y in 0..dst_selection.size.1 {
                    for x in 0..dst_selection.size.0 {
                        let dst_palette_id = dst_selection.palettes[y as usize][x as usize];
                        let dst_tile_idx = dst_selection.tiles[y as usize][x as usize];
                        if used_tiles.contains(&(dst_palette_id, dst_tile_idx)) {
                            return Ok(Some(Task::done(Message::MoveTilesConfirmDialogue {
                                src_selection: src_selection.clone(),
                                dst_selection: dst_selection.clone(),
                            })));
                        }
                    }
                }
            }

            // Update the references to the tiles:
            remap_tiles(state, &mapping)?;

            state.dialogue = None;
        }
        Message::MoveTilesConfirmDialogue {
            src_selection,
            dst_selection,
        } => {
            state.dialogue = Some(Dialogue::MoveTiles {
                src_selection: src_selection.clone(),
                dst_selection: dst_selection.clone(),
            });
        }
    }
    Ok(Some(Task::none()))
}

pub fn update(state: &mut EditorState, mut message: Message) -> Task<Message> {
    // Handle undo/redo controls:
    let mut undo = false;
    match &message {
        Message::Event(Event::Keyboard(keyboard::Event::KeyPressed {
            key: keyboard::Key::Character(c),
            modifiers,
            ..
        })) if modifiers.control() && c == "z" => {
            if modifiers.shift() {
                // Redo:
                if let Some((msg, rev_msg)) = state.redo_stack.pop() {
                    state.undo_stack.push((msg.clone(), rev_msg));
                    message = msg;
                    undo = true;
                }
            } else {
                // Undo:
                if let Some((msg, rev_msg)) = state.undo_stack.pop() {
                    state.redo_stack.push((msg, rev_msg.clone()));
                    message = rev_msg;
                    undo = true;
                }
            }
        }
        _ => {}
    }

    if let Some((last_message, _)) = state.undo_stack.last() {
        if !undo && should_debounce(&message, last_message) {
            return Task::none();
        }
    }

    let undo_action = if undo {
        // Don't try to undo an undo/redo
        UndoAction::None
    } else {
        match get_undo_action(state, &message) {
            Ok(action) => action,
            Err(e) => {
                error!("Error creating undo action: {}\n{}", e, e.backtrace());
                return Task::none();
            }
        }
    };

    match try_update(state, &message) {
        Ok(Some(t)) => {
            // The update was successful, so update the undo stack if applicable:
            match undo_action {
                UndoAction::None => {}
                UndoAction::Irreversible => {
                    state.undo_stack.clear();
                    state.redo_stack.clear();
                }
                UndoAction::Ok(reverse_message) => {
                    state.undo_stack.push((message, reverse_message));
                    state.redo_stack.clear();
                }
            }
            t
        }
        Ok(None) => {
            // The update did not process (for some normal reason), so
            // skip pushing onto the undo stack.
            Task::none()
        }
        Err(e) => {
            // The update failed for an abnormal reason, so skip pushing
            // onto the undo stack, and log the error and backtrace:
            error!("Error processing {:?}: {}\n{}", message, e, e.backtrace());

            // Make sure file watcher is re-enabled, since an error could easily
            // have occurred between disabling and re-enabling:
            if let Err(e) = state.enable_watch_file_changes() {
                error!("Error re-enabling watcher: {}\n{}", e, e.backtrace());
            }
            return Task::none();
        }
    }
}

pub fn update_palette_order(state: &mut EditorState) {
    let id = state.palettes[state.palette_idx].id;
    state.palettes.sort_by(|x, y| x.id.cmp(&y.id));
    state.palettes_id_idx_map.clear();
    for i in 0..state.palettes.len() {
        state.palettes_id_idx_map.insert(state.palettes[i].id, i);
        if state.palettes[i].id == id {
            state.palette_idx = i;
        }
    }
}

pub fn get_selected_gfx(state: &EditorState, s: &TileBlock) -> Vec<Vec<Tile>> {
    let mut gfx = vec![];
    for y in 0..s.size.1 {
        let mut gfx_row: Vec<Tile> = vec![];
        for x in 0..s.size.0 {
            let palette_id = s.palettes[y as usize][x as usize];
            let tile_idx = s.tiles[y as usize][x as usize];
            let flip = s.flips[y as usize][x as usize];
            let tile = if let Some(&idx) = state.palettes_id_idx_map.get(&palette_id) {
                if (tile_idx as usize) < state.palettes[idx as usize].tiles.len() {
                    flip.apply_to_tile(state.palettes[idx as usize].tiles[tile_idx as usize])
                } else {
                    Tile::default()
                }
            } else {
                Tile::default()
            };
            gfx_row.push(tile);
        }
        gfx.push(gfx_row);
    }
    gfx
}
