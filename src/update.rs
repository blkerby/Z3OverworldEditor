use hashbrown::HashMap;
use iced::{
    keyboard::{self, key},
    widget, window, Event, Point, Task,
};
use log::{error, info, warn};

use crate::{
    import::Importer,
    message::{Message, SelectionSource},
    persist::{
        self, clear_pngs, copy_area_theme, delete_area, delete_area_theme, delete_layer_png,
        delete_palette, load_area_list, remap_tiles, rename_area, rename_area_theme, save_area,
        save_area_png, save_palettes, scan_used_tiles,
    },
    state::{
        is_valid_layer_name, Area, AreaId, AreaPosition, Background, BackgroundLayering, ColorIdx,
        ColorRGB, Dialogue, DynamicTileGrid, DynamicTileGroup, DynamicTileTarget, DynamicTileType,
        DynamicTileVariant, EditorState, Flip, Focus, Layer, PaletteId, PixelTarget, SidePanelView,
        Tile, TileBlock, TileIdx, TilePlacement, Tool, MAX_PIXEL_SIZE, MIN_PIXEL_SIZE,
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

fn target_pixel(
    state: &EditorState,
    palette_idx: usize,
    target: PixelTarget,
    x: u8,
    y: u8,
) -> Result<ColorIdx> {
    let palette = &state.palettes[palette_idx];
    Ok(match target {
        PixelTarget::Regular(tile) => palette.tiles[tile as usize].pixels[y as usize][x as usize],
        PixelTarget::Animated { tile_idx, frame } => {
            let base_tile = tile_idx / 16 * 16;
            let tile = (tile_idx % 16) as usize;
            palette
                .animated_tile_groups
                .iter()
                .find(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?
                .frames[frame][tile][y as usize][x as usize]
        }
    })
}

fn set_target_pixel(
    state: &mut EditorState,
    palette_idx: usize,
    target: PixelTarget,
    x: u8,
    y: u8,
    color: ColorIdx,
) -> Result<()> {
    let palette = &mut state.palettes[palette_idx];
    match target {
        PixelTarget::Regular(tile) => {
            palette.tiles[tile as usize].pixels[y as usize][x as usize] = color
        }
        PixelTarget::Animated { tile_idx, frame } => {
            let base_tile = tile_idx / 16 * 16;
            let tile = (tile_idx % 16) as usize;
            palette
                .animated_tile_groups
                .iter_mut()
                .find(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?
                .frames[frame][tile][y as usize][x as usize] = color
        }
    }
    palette.modified = true;
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
                palette_id == last_palette_id && color_idx == last_color_idx && color == last_color
            }
            _ => false,
        },
        Message::BrushPixel {
            palette_id,
            target,
            coords,
            color_idx,
        } => match last_message {
            Message::BrushPixel {
                palette_id: last_palette_id,
                target: last_target,
                coords: last_coords,
                color_idx: last_color_idx,
            } => {
                palette_id == last_palette_id
                    && target == last_target
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
            layer_idx,
            coords,
            selection,
            palette_only,
        } => match last_message {
            Message::AreaBrush {
                position: last_position,
                area_id: last_area_id,
                layer_idx: last_layer_idx,
                coords: last_coords,
                selection: last_selection,
                palette_only: last_palette_only,
            } => {
                position == last_position
                    && area_id == last_area_id
                    && layer_idx == last_layer_idx
                    && coords == last_coords
                    && selection == last_selection
                    && palette_only == last_palette_only
            }
            _ => false,
        },
        Message::AreaErase {
            position,
            area_id,
            layer_idx,
            coords,
            size,
        } => matches!(
            last_message,
            Message::AreaErase {
                position: last_position,
                area_id: last_area_id,
                layer_idx: last_layer_idx,
                coords: last_coords,
                size: last_size,
            } if position == last_position
                && area_id == last_area_id
                && layer_idx == last_layer_idx
                && coords == last_coords
                && size == last_size
        ),
        Message::DynamicTileBrush {
            kind,
            variant,
            target,
            coords,
            selection,
        } => match last_message {
            Message::DynamicTileBrush {
                kind: last_kind,
                variant: last_variant,
                target: last_target,
                coords: last_coords,
                selection: last_selection,
            } => {
                kind == last_kind
                    && variant == last_variant
                    && target == last_target
                    && coords == last_coords
                    && selection == last_selection
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
                    Focus::PickDynamicTileType => {}
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
                if state.dynamic_tiles_open && state.dialogue.is_none() {
                    state.dynamic_tiles_open = false;
                    return Ok(None);
                }
                state.tool = Tool::Select;
                state.dialogue = None;
                state.color_idx = None;
                state.tile_idx = None;
                state.pixel_target = None;
                state.pixel_coords = None;
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
                    Focus::PickDynamicTileType => {}
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
                                    state.pixel_target.context("pixel target not selected")?,
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
                    Focus::PickDynamicTileType => {}
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
                                    state.pixel_target.context("pixel target not selected")?,
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
                            state.pixel_target = None;
                            state.pixel_coords = None;
                        }
                    }
                    Focus::PickDynamicTileType => {
                        if let Some(kind_idx) = DynamicTileType::ALL
                            .iter()
                            .position(|kind| kind == &state.dynamic_tile_type)
                        {
                            if kind_idx + 1 < DynamicTileType::ALL.len() {
                                return Ok(Some(Task::done(Message::SelectDynamicTileType(
                                    DynamicTileType::ALL[kind_idx + 1],
                                ))));
                            }
                        }
                    }
                    Focus::PaletteColor => {}
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.1 < 7 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    state.pixel_target.context("pixel target not selected")?,
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
                            state.pixel_target = None;
                            state.pixel_coords = None;
                        }
                    }
                    Focus::PickDynamicTileType => {
                        if let Some(kind_idx) = DynamicTileType::ALL
                            .iter()
                            .position(|kind| kind == &state.dynamic_tile_type)
                        {
                            if kind_idx > 0 {
                                return Ok(Some(Task::done(Message::SelectDynamicTileType(
                                    DynamicTileType::ALL[kind_idx - 1],
                                ))));
                            }
                        }
                    }
                    Focus::PaletteColor => {}
                    Focus::GraphicsPixel => {
                        if let Some(coords) = state.pixel_coords {
                            if coords.1 > 0 {
                                return Ok(Some(Task::done(Message::SelectPixel(
                                    state.pixel_target.context("pixel target not selected")?,
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
                    if c.as_str() == "r" {
                        return Ok(Some(Task::done(Message::RebuildProjectDialogue)));
                    }
                } else {
                    match c.as_str() {
                        "b" => {
                            state.tool = Tool::Brush;
                        }
                        "e" => {
                            state.tool = Tool::Erase;
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
                                state.selected_tile_block.placements[i].reverse();
                                state.selected_gfx[i].reverse();
                                for j in 0..state.selected_tile_block.size.0 as usize {
                                    if let Some(placement) =
                                        &mut state.selected_tile_block.placements[i][j]
                                    {
                                        placement.flip = placement.flip.flip_horizontally();
                                    }
                                    if let Some(tile) = &mut state.selected_gfx[i][j] {
                                        *tile = Flip::Horizontal.apply_to_tile(*tile);
                                    }
                                }
                            }
                        }
                        "v" => {
                            state.selected_tile_block.placements.reverse();
                            state.selected_gfx.reverse();
                            for i in 0..state.selected_tile_block.size.1 as usize {
                                for j in 0..state.selected_tile_block.size.0 as usize {
                                    if let Some(placement) =
                                        &mut state.selected_tile_block.placements[i][j]
                                    {
                                        placement.flip = placement.flip.flip_vertically();
                                    }
                                    if let Some(tile) = &mut state.selected_gfx[i][j] {
                                        *tile = Flip::Vertical.apply_to_tile(*tile);
                                    }
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
            // Also save all palettes.
            state.disable_watch_file_changes()?;
            clear_pngs(state)?;
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
            for pal in &mut state.palettes {
                pal.modified = true;
            }
            save_palettes(state)?;
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
        Message::OpenDynamicTiles => {
            state.dialogue = None;
            state.dynamic_tiles_open = true;
            let count = state
                .dynamic_tiles
                .groups
                .iter()
                .find(|group| group.kind == state.dynamic_tile_type)
                .map_or(0, |group| group.variants.len());
            state.dynamic_tile_frames = vec![0; count];
        }
        Message::CloseDynamicTiles => {
            state.dynamic_tiles_open = false;
        }
        Message::SelectDynamicTileType(kind) => {
            state.dynamic_tile_type = *kind;
            let count = state
                .dynamic_tiles
                .groups
                .iter()
                .find(|group| group.kind == *kind)
                .map_or(0, |group| group.variants.len());
            state.dynamic_tile_frames = vec![0; count];
        }
        Message::SelectDynamicTileFrame { variant, frame } => {
            state.dynamic_tile_frames.resize(*variant + 1, 0);
            state.dynamic_tile_frames[*variant] = *frame;
        }
        Message::AddDynamicTileVariant => {
            let kind = state.dynamic_tile_type;
            let group_idx = if let Some(idx) = state
                .dynamic_tiles
                .groups
                .iter()
                .position(|group| group.kind == kind)
            {
                idx
            } else {
                state.dynamic_tiles.groups.push(DynamicTileGroup {
                    kind,
                    variants: vec![],
                });
                state.dynamic_tiles.groups.len() - 1
            };
            let (width, height) = kind.size();
            let empty_grid = DynamicTileGrid {
                tiles: vec![vec![None; width]; height],
            };
            let variant = DynamicTileVariant {
                before: empty_grid.clone(),
                after_frames: vec![empty_grid; kind.after_frame_count()],
            };
            state.dynamic_tiles.groups[group_idx].variants.push(variant);
            state
                .dynamic_tile_frames
                .resize(state.dynamic_tiles.groups[group_idx].variants.len(), 0);
            state.dynamic_tiles.modified = true;
        }
        Message::DeleteDynamicTileVariant(variant) => {
            let kind = state.dynamic_tile_type;
            if let Some(group_idx) = state
                .dynamic_tiles
                .groups
                .iter()
                .position(|group| group.kind == kind)
            {
                let variants = &mut state.dynamic_tiles.groups[group_idx].variants;
                if *variant < variants.len() {
                    variants.remove(*variant);
                    if variants.is_empty() {
                        state.dynamic_tiles.groups.remove(group_idx);
                    }
                    if *variant < state.dynamic_tile_frames.len() {
                        state.dynamic_tile_frames.remove(*variant);
                    }
                    state.start_coords = None;
                    state.end_coords = None;
                    state.dynamic_tiles.modified = true;
                }
            }
        }
        Message::SetDynamicTileVariants { kind, variants } => {
            if let Some(group_idx) = state
                .dynamic_tiles
                .groups
                .iter()
                .position(|group| group.kind == *kind)
            {
                if variants.is_empty() {
                    state.dynamic_tiles.groups.remove(group_idx);
                } else {
                    state.dynamic_tiles.groups[group_idx].variants = variants.clone();
                }
            } else if !variants.is_empty() {
                state.dynamic_tiles.groups.push(DynamicTileGroup {
                    kind: *kind,
                    variants: variants.clone(),
                });
            }
            state.dynamic_tiles.groups.sort_by_key(|group| group.kind);
            if *kind == state.dynamic_tile_type {
                state.dynamic_tile_frames = vec![0; variants.len()];
            }
            state.dynamic_tiles.modified = true;
        }
        Message::DynamicTileBrush {
            kind,
            variant,
            target,
            coords,
            selection,
        } => {
            if let Some(group) = state
                .dynamic_tiles
                .groups
                .iter_mut()
                .find(|group| group.kind == *kind)
            {
                if let Some(variant) = group.variants.get_mut(*variant) {
                    let grid = match target {
                        DynamicTileTarget::Before => &mut variant.before,
                        DynamicTileTarget::After(frame) => &mut variant.after_frames[*frame],
                    };
                    for y in 0..selection.size.1 as usize {
                        let grid_y = coords.y as usize + y;
                        if grid_y >= grid.tiles.len() {
                            break;
                        }
                        for x in 0..selection.size.0 as usize {
                            let grid_x = coords.x as usize + x;
                            if grid_x >= grid.tiles[grid_y].len() {
                                break;
                            }
                            grid.tiles[grid_y][grid_x] = selection.placements[y][x];
                        }
                    }
                    state.dynamic_tiles.modified = true;
                }
            }
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
            state.pixel_target = state.tile_idx.map(PixelTarget::Regular);
            state.pixel_coords = None;
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
                    state.pixel_target = None;
                    state.pixel_coords = None;
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
        Message::SetAddPaletteName(new_name) => {
            if let Some(Dialogue::AddPalette { name, .. }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        &Message::SetAddPaletteID(new_id) => {
            if let Some(Dialogue::AddPalette { id, .. }) = &mut state.dialogue {
                *id = new_id;
            }
        }
        Message::AddPalette { name, id } => {
            if name.is_empty() {
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
        Message::SetRenamePaletteName(new_name) => {
            if let Some(Dialogue::RenamePalette { name }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        Message::RenamePalette { id: _, name } => {
            if name.is_empty() {
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
            state.tile_idx = None;
            state.pixel_target = None;
            state.pixel_coords = None;
            state.color_idx = None;
            state.dialogue = None;
        }
        Message::RestorePalette(palette) => {
            let mut pal = palette.clone();
            pal.modified = true;
            state.palettes.push(pal);
            state.palette_idx = state.palettes.len() - 1;
            state.tile_idx = None;
            state.pixel_target = None;
            state.pixel_coords = None;
            state.color_idx = None;
            update_palette_order(state);
        }
        Message::AnimatedTilesDialogue => {
            let palette = &state.palettes[state.palette_idx];
            let selected_row = state.tile_idx.map(|tile| tile / 16 * 16);
            let base_tile = selected_row
                .filter(|base| {
                    palette
                        .animated_tile_groups
                        .iter()
                        .any(|group| group.base_tile == *base)
                })
                .or_else(|| {
                    palette
                        .animated_tile_groups
                        .first()
                        .map(|group| group.base_tile)
                });
            state.dialogue = Some(Dialogue::AnimatedTiles {
                base_tile,
                frame: 0,
                tile: 0,
            });
        }
        &Message::SelectAnimatedGroup(base_tile) => {
            if let Some(Dialogue::AnimatedTiles {
                base_tile: selected,
                frame,
                tile,
            }) = &mut state.dialogue
            {
                *selected = Some(base_tile);
                *frame = 0;
                *tile = 0;
                state.pixel_coords = None;
                state.pixel_target = None;
                state.focus = Focus::None;
            }
        }
        &Message::SelectAnimatedTile {
            base_tile,
            frame: new_frame,
            tile: new_tile,
        } => {
            if let Some(Dialogue::AnimatedTiles {
                base_tile: selected,
                frame,
                tile,
            }) = &mut state.dialogue
            {
                *selected = Some(base_tile);
                *frame = new_frame;
                *tile = new_tile;
                state.pixel_coords = None;
                state.pixel_target = None;
                state.focus = Focus::None;
            }
        }
        Message::AddAnimatedTileGroup { palette_id, group } => {
            let palette_idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            let palette = &mut state.palettes[palette_idx];
            if palette
                .animated_tile_groups
                .iter()
                .any(|existing| existing.base_tile == group.base_tile)
            {
                warn!("Animated tile group already exists for this row.");
                return Ok(None);
            }
            palette.animated_tile_groups.push(group.clone());
            palette
                .animated_tile_groups
                .sort_by_key(|group| group.base_tile);
            palette.modified = true;
            state.dialogue = Some(Dialogue::AnimatedTiles {
                base_tile: Some(group.base_tile),
                frame: 0,
                tile: 0,
            });
        }
        &Message::DeleteAnimatedTileGroup {
            palette_id,
            base_tile,
        } => {
            let palette_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("palette not found")?;
            let palette = &mut state.palettes[palette_idx];
            let group_idx = palette
                .animated_tile_groups
                .iter()
                .position(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?;
            palette.animated_tile_groups.remove(group_idx);
            palette.modified = true;
            let next = palette
                .animated_tile_groups
                .get(group_idx)
                .or_else(|| palette.animated_tile_groups.last())
                .map(|group| group.base_tile);
            state.dialogue = Some(Dialogue::AnimatedTiles {
                base_tile: next,
                frame: 0,
                tile: 0,
            });
        }
        Message::SetAnimatedTileGroup {
            palette_id,
            base_tile,
            group,
        } => {
            let palette_idx = *state
                .palettes_id_idx_map
                .get(palette_id)
                .context("palette not found")?;
            let palette = &mut state.palettes[palette_idx];
            let old = palette
                .animated_tile_groups
                .iter_mut()
                .find(|candidate| candidate.base_tile == *base_tile)
                .context("animated tile group not found")?;
            *old = group.clone();
            palette.modified = true;
            if let Some(Dialogue::AnimatedTiles { frame, .. }) = &mut state.dialogue {
                *frame = (*frame).min(group.frames.len());
            }
        }
        &Message::SetAnimatedFrameCount {
            palette_id,
            base_tile,
            frame_count,
        } => {
            let palette_idx = state.palettes_id_idx_map[&palette_id];
            let palette = &mut state.palettes[palette_idx];
            let group = palette
                .animated_tile_groups
                .iter_mut()
                .find(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?;
            let last = *group
                .frames
                .last()
                .context("animated tile group has no frames")?;
            group.frames.resize(frame_count as usize - 1, last);
            palette.modified = true;
            if let Some(Dialogue::AnimatedTiles { frame, .. }) = &mut state.dialogue {
                *frame = (*frame).min(frame_count as usize - 1);
            }
        }
        &Message::SetAnimatedFrameHold {
            palette_id,
            base_tile,
            frame_hold,
        } => {
            let palette_idx = state.palettes_id_idx_map[&palette_id];
            let palette = &mut state.palettes[palette_idx];
            palette
                .animated_tile_groups
                .iter_mut()
                .find(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?
                .frame_hold = frame_hold;
            palette.modified = true;
        }
        &Message::SetAnimatedPhaseOffset {
            palette_id,
            base_tile,
            phase_offset,
        } => {
            let palette_idx = state.palettes_id_idx_map[&palette_id];
            let palette = &mut state.palettes[palette_idx];
            palette
                .animated_tile_groups
                .iter_mut()
                .find(|group| group.base_tile == base_tile)
                .context("animated tile group not found")?
                .phase_offset = phase_offset;
            palette.modified = true;
        }
        Message::HideModal => {
            state.dialogue = None;
            state.pixel_target = state.tile_idx.map(PixelTarget::Regular);
            state.pixel_coords = None;
        }
        &Message::SelectColor(pal_idx, color_idx) => {
            if pal_idx != state.palette_idx {
                state.tile_idx = None;
                state.pixel_target = None;
                state.pixel_coords = None;
                state.start_coords = None;
                state.end_coords = None;
            }
            state.palette_idx = pal_idx;
            state.color_idx = Some(color_idx);
            state.selected_color = state.palettes[pal_idx].colors[color_idx as usize];
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
                    color_idx,
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
                    color_idx,
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
                    color_idx,
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
            let new_size = state.palettes[idx].tiles.len() - 16;
            if state.palettes[idx]
                .animated_tile_groups
                .iter()
                .any(|group| group.base_tile as usize == new_size)
            {
                warn!("Not allowed to delete an animated tile group's base row.");
                return Ok(None);
            }
            state.palettes[idx].tiles.resize(new_size, Tile::default());
            if let Some(idx) = state.tile_idx {
                if idx >= new_size as TileIdx {
                    state.tile_idx = Some(new_size as TileIdx - 1);
                }
            }
            state.palettes[idx].modified = true;
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
            for (y, row) in s.iter().enumerate() {
                for (x, &source_tile) in row.iter().enumerate() {
                    let Some(source_tile) = source_tile else {
                        continue;
                    };
                    let y1 = y + y0 as usize;
                    let x1 = x + x0 as usize;
                    let i = y1 * 16 + x1;
                    if x1 < 16 && i < state.palettes[pal_idx].tiles.len() {
                        let mut tile = source_tile;
                        if let Some(t) = tile_block {
                            let Some(placement) = t.placements[y][x] else {
                                continue;
                            };
                            let src_pal_id = placement.palette;
                            if src_pal_id != palette_id {
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
                                        } else {
                                            tile.pixels[py][px] = 15;
                                        }
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
        &Message::SelectPixel(target, x, y) => {
            state.pixel_coords = Some((x, y));
            state.pixel_target = Some(target);
            let color_idx = target_pixel(state, state.palette_idx, target, x, y)?;
            state.color_idx = Some(color_idx);
            state.selected_color = state.palettes[state.palette_idx].colors[color_idx as usize];
            state.focus = Focus::GraphicsPixel;
        }
        &Message::BrushPixel {
            palette_id,
            target,
            coords,
            color_idx,
        } => {
            let pal_idx = *state
                .palettes_id_idx_map
                .get(&palette_id)
                .context("undefined palette")?;
            set_target_pixel(state, pal_idx, target, coords.x, coords.y, color_idx)?;
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
        Message::SetAddAreaName(new_name) => {
            if let Some(Dialogue::AddArea { name, .. }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        &Message::SetAddAreaSizeX(new_x) => {
            if let Some(Dialogue::AddArea { size, .. }) = &mut state.dialogue {
                size.0 = new_x;
            }
        }
        &Message::SetAddAreaSizeY(new_y) => {
            if let Some(Dialogue::AddArea { size, .. }) = &mut state.dialogue {
                size.1 = new_y;
            }
        }
        Message::AddArea { name, size } => {
            if name.is_empty() {
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
                        vanilla_map_id: None,
                        bg_color: state.areas[&state.main_area_id].bg_color,
                        bg_layering: BackgroundLayering::None,
                        bg_camera_follow_x: 1.0,
                        bg_camera_drift_x: 0.0,
                        bg_camera_follow_y: 1.0,
                        bg_camera_drift_y: 0.0,
                        layers: vec![Layer {
                            modified: true,
                            name: "Main".to_string(),
                            background: Background::Bg2,
                            tiles: vec![
                                vec![Some(TilePlacement::default()); size.0 as usize * 32];
                                size.1 as usize * 32
                            ],
                        }],
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
        Message::SetEditAreaName(new_name) => {
            if let Some(Dialogue::EditArea { name }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        Message::EditArea { old_name, new_name } => {
            if new_name.is_empty() {
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
            state.main_area_mut().modified = true;
        }
        Message::EditAreaBGLayering { area_id, value } => {
            let area = state.areas.get_mut(area_id).context("area not loaded")?;
            area.bg_layering = *value;
            area.modified = true;
        }
        Message::EditAreaBGCameraFollowX { area_id, value } => {
            let area = state.areas.get_mut(area_id).context("area not loaded")?;
            area.bg_camera_follow_x = *value;
            area.modified = true;
        }
        &Message::EditAreaBGCameraDriftX(value) => {
            return Ok(Some(Task::done(Message::SetAreaBGCameraDriftX {
                area_id: state.main_area_id.clone(),
                value,
            })));
        }
        Message::SetAreaBGCameraDriftX { area_id, value } => {
            let area = state.areas.get_mut(area_id).context("area not loaded")?;
            area.bg_camera_drift_x = *value;
            area.modified = true;
        }
        Message::EditAreaBGCameraFollowY { area_id, value } => {
            let area = state.areas.get_mut(area_id).context("area not loaded")?;
            area.bg_camera_follow_y = *value;
            area.modified = true;
        }
        &Message::EditAreaBGCameraDriftY(value) => {
            return Ok(Some(Task::done(Message::SetAreaBGCameraDriftY {
                area_id: state.main_area_id.clone(),
                value,
            })));
        }
        Message::SetAreaBGCameraDriftY { area_id, value } => {
            let area = state.areas.get_mut(area_id).context("area not loaded")?;
            area.bg_camera_drift_y = *value;
            area.modified = true;
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
        Message::ToggleLayerDrawer => state.layer_drawer_open = !state.layer_drawer_open,
        &Message::SelectLayer(position, layer_idx) => {
            if layer_idx >= state.area(position).layers.len() {
                return Ok(None);
            }
            match position {
                AreaPosition::Main => {
                    state.main_layer_idx = layer_idx;
                    state.visible_layers[layer_idx] = true;
                }
                AreaPosition::Side => state.side_layer_idx = layer_idx,
            }
        }
        &Message::ToggleLayerVisibility(layer_idx) => {
            let Some(visible) = state.visible_layers.get_mut(layer_idx) else {
                return Ok(None);
            };
            *visible = !*visible;
        }
        &Message::AddLayer(position) => {
            let selected = state.selected_layer_idx(position);
            let same_side =
                position == AreaPosition::Main && state.side_area_id == state.main_area_id;
            let mut suffix = 1;
            let name = loop {
                let name = if suffix == 1 {
                    "Layer".to_string()
                } else {
                    format!("Layer {suffix}")
                };
                if !state
                    .area(position)
                    .layers
                    .iter()
                    .any(|layer| layer.name == name)
                {
                    break name;
                }
                suffix += 1;
            };
            let layer_idx = selected + 1;
            let area = state.area_mut(position);
            area.layers.insert(
                layer_idx,
                Layer {
                    modified: true,
                    name,
                    background: Background::Bg2,
                    tiles: vec![vec![None; area.size.0 as usize * 32]; area.size.1 as usize * 32],
                },
            );
            area.modified = true;
            match position {
                AreaPosition::Main => {
                    state.main_layer_idx = layer_idx;
                    state.visible_layers.insert(layer_idx, true);
                    if same_side && state.side_layer_idx >= layer_idx {
                        state.side_layer_idx += 1;
                    }
                }
                AreaPosition::Side => state.side_layer_idx = layer_idx,
            }
        }
        Message::DeleteLayer {
            position,
            area_id,
            layer_idx,
        } => {
            state.switch_area(*position, area_id)?;
            let same_side = *position == AreaPosition::Main && state.side_area_id == *area_id;
            let area = state.area(*position);
            let Some(layer) = area.layers.get(*layer_idx) else {
                return Ok(None);
            };
            if area.layers.len() == 1
                || (layer.background == Background::Bg2
                    && area
                        .layers
                        .iter()
                        .filter(|layer| layer.background == Background::Bg2)
                        .count()
                        == 1)
            {
                warn!("Not allowed to delete the final layer or final BG2 layer.");
                return Ok(None);
            }
            let name = layer.name.clone();
            delete_layer_png(state, area_id, &name)?;
            let area = state.area_mut(*position);
            area.layers.remove(*layer_idx);
            area.modified = true;
            let selected = (*layer_idx).min(area.layers.len() - 1);
            match position {
                AreaPosition::Main => {
                    state.visible_layers.remove(*layer_idx);
                    state.main_layer_idx = selected;
                    state.visible_layers[selected] = true;
                    if same_side {
                        if state.side_layer_idx > *layer_idx {
                            state.side_layer_idx -= 1;
                        } else if state.side_layer_idx == *layer_idx {
                            state.side_layer_idx = selected;
                        }
                    }
                }
                AreaPosition::Side => state.side_layer_idx = selected,
            }
        }
        Message::RestoreLayer {
            position,
            area_id,
            layer_idx,
            layer,
        } => {
            state.switch_area(*position, area_id)?;
            let same_side = *position == AreaPosition::Main && state.side_area_id == *area_id;
            let mut layer = layer.clone();
            layer.modified = true;
            let area = state.area_mut(*position);
            let layer_idx = (*layer_idx).min(area.layers.len());
            area.layers.insert(layer_idx, layer);
            area.modified = true;
            match position {
                AreaPosition::Main => {
                    state.visible_layers.insert(layer_idx, true);
                    state.main_layer_idx = layer_idx;
                    if same_side && state.side_layer_idx >= layer_idx {
                        state.side_layer_idx += 1;
                    }
                }
                AreaPosition::Side => state.side_layer_idx = layer_idx,
            }
        }
        Message::MoveLayer {
            position,
            area_id,
            layer_idx,
            new_idx,
        } => {
            state.switch_area(*position, area_id)?;
            let same_side = *position == AreaPosition::Main && state.side_area_id == *area_id;
            let area = state.area_mut(*position);
            if *layer_idx >= area.layers.len() || *new_idx >= area.layers.len() {
                return Ok(None);
            }
            let layer = area.layers.remove(*layer_idx);
            area.layers.insert(*new_idx, layer);
            area.modified = true;
            match position {
                AreaPosition::Main => {
                    let visible = state.visible_layers.remove(*layer_idx);
                    state.visible_layers.insert(*new_idx, visible);
                    state.main_layer_idx = *new_idx;
                    if same_side {
                        if state.side_layer_idx == *layer_idx {
                            state.side_layer_idx = *new_idx;
                        } else if layer_idx < new_idx
                            && state.side_layer_idx > *layer_idx
                            && state.side_layer_idx <= *new_idx
                        {
                            state.side_layer_idx -= 1;
                        } else if new_idx < layer_idx
                            && state.side_layer_idx >= *new_idx
                            && state.side_layer_idx < *layer_idx
                        {
                            state.side_layer_idx += 1;
                        }
                    }
                }
                AreaPosition::Side => state.side_layer_idx = *new_idx,
            }
        }
        Message::RenameLayer {
            position,
            area_id,
            layer_idx,
            name,
        } => {
            state.switch_area(*position, area_id)?;
            let area = state.area(*position);
            if !is_valid_layer_name(name)
                || area
                    .layers
                    .iter()
                    .enumerate()
                    .any(|(idx, layer)| idx != *layer_idx && layer.name == *name)
            {
                warn!("Invalid or duplicate layer name: {name}");
                return Ok(None);
            }
            let old_name = area.layers[*layer_idx].name.clone();
            if old_name == *name {
                return Ok(None);
            }
            delete_layer_png(state, area_id, &old_name)?;
            let area = state.area_mut(*position);
            area.layers[*layer_idx].name = name.clone();
            area.layers[*layer_idx].modified = true;
            area.modified = true;
        }
        Message::SetLayerBackground {
            position,
            area_id,
            layer_idx,
            background,
        } => {
            state.switch_area(*position, area_id)?;
            let area = state.area_mut(*position);
            if area.layers[*layer_idx].background == Background::Bg2
                && *background == Background::Bg1
                && area
                    .layers
                    .iter()
                    .filter(|layer| layer.background == Background::Bg2)
                    .count()
                    == 1
            {
                warn!("Not allowed to change the final BG2 layer to BG1.");
                return Ok(None);
            }
            if area.layers[*layer_idx].background == *background {
                return Ok(None);
            }
            area.layers[*layer_idx].background = *background;
            area.modified = true;
        }
        Message::AddThemeDialogue => {
            state.dialogue = Some(Dialogue::AddTheme {
                name: "".to_string(),
            });
            return Ok(Some(iced::widget::text_input::focus("AddTheme")));
        }
        Message::SetAddThemeName(new_name) => {
            if let Some(Dialogue::AddTheme { name }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        Message::AddTheme(theme_name) => {
            if theme_name.is_empty() {
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
                copy_area_theme(state, area_name, &old_theme, theme_name)?;
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
        Message::SetRenameThemeName(new_name) => {
            if let Some(Dialogue::RenameTheme { name }) = &mut state.dialogue {
                *name = new_name.clone();
            }
        }
        Message::RenameTheme { old_name, new_name } => {
            if new_name.is_empty() {
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
        Message::HoverArea(p) => {
            state.hover_coords = Some((p.x, p.y));
        }
        Message::HoverAreaEnd => {
            state.hover_coords = None;
        }
        &Message::StartTileSelection(p, source) => {
            state.selection_source = source;
            state.start_coords = Some((p.x, p.y));
            state.end_coords = Some((p.x, p.y));
            state.hover_coords = None;
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

            if state.snap_grid_16
                && !matches!(state.selection_source, SelectionSource::DynamicTiles { .. })
            {
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
                SelectionSource::DynamicTiles { .. } => {
                    state.focus = Focus::None;
                }
            }

            let mut placements = vec![];
            for y in top..=bottom {
                let mut row = vec![];
                for x in left..=right {
                    let placement = match state.selection_source {
                        SelectionSource::Area(position) => state
                            .area(position)
                            .get_layer_placement(state.selected_layer_idx(position), x, y)?,
                        SelectionSource::Tileset => Some(TilePlacement {
                            palette: state.palettes[state.palette_idx].id,
                            tile: y * 16 + x,
                            flip: Flip::None,
                        }),
                        SelectionSource::DynamicTiles {
                            kind,
                            variant,
                            target,
                        } => {
                            let Some(group) = state
                                .dynamic_tiles
                                .groups
                                .iter()
                                .find(|group| group.kind == kind)
                            else {
                                return Ok(None);
                            };
                            let Some(variant) = group.variants.get(variant) else {
                                return Ok(None);
                            };
                            let grid = match target {
                                DynamicTileTarget::Before => &variant.before,
                                DynamicTileTarget::After(frame) => {
                                    let Some(frame) = variant.after_frames.get(frame) else {
                                        return Ok(None);
                                    };
                                    frame
                                }
                            };
                            grid.tiles
                                .get(y as usize)
                                .and_then(|row| row.get(x as usize))
                                .copied()
                                .flatten()
                        }
                    };
                    row.push(placement);
                }
                placements.push(row);
            }
            state.selected_tile_block = TileBlock {
                size: (right - left + 1, bottom - top + 1),
                placements,
            };
            let s = &state.selected_tile_block;

            state.selected_gfx = get_selected_gfx(state, &state.selected_tile_block);
            state.start_coords = None;
            state.end_coords = None;
            if left == right && top == bottom {
                let Some(placement) = s.placements[0][0] else {
                    state.tile_idx = None;
                    state.pixel_target = None;
                    state.pixel_coords = None;
                    return Ok(Some(Task::none()));
                };
                return Ok(Some(Task::done(Message::OpenTile {
                    palette_id: placement.palette,
                    tile_idx: placement.tile,
                })));
            } else {
                state.tile_idx = None;
                state.pixel_target = None;
                state.pixel_coords = None;
            }
        }
        &Message::AreaBrush {
            position,
            ref area_id,
            layer_idx,
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
                    let source = s.placements[y as usize][x as usize];
                    if palette_only {
                        if let (Some(source), Ok(Some(mut destination))) = (
                            source,
                            area.get_layer_placement(layer_idx, p.x + x, p.y + y),
                        ) {
                            destination.palette = source.palette;
                            let _ = area.set_layer_placement(
                                layer_idx,
                                p.x + x,
                                p.y + y,
                                Some(destination),
                            );
                        }
                    } else {
                        let _ = area.set_layer_placement(layer_idx, p.x + x, p.y + y, source);
                    }
                }
            }
        }
        &Message::AreaErase {
            position,
            ref area_id,
            layer_idx,
            coords,
            size,
        } => {
            state.switch_area(position, area_id)?;
            let area = state.area_mut(position);
            for y in 0..size {
                for x in 0..size {
                    let _ = area.set_layer_placement(layer_idx, coords.x + x, coords.y + y, None);
                }
            }
        }
        &Message::OpenTile {
            palette_id,
            tile_idx,
        } => {
            if let Some(&palette_idx) = state.palettes_id_idx_map.get(&palette_id) {
                if palette_idx != state.palette_idx {
                    state.color_idx = None;
                }
                state.palette_idx = palette_idx;
                state.tile_idx = Some(tile_idx);
                state.pixel_target = Some(PixelTarget::Regular(tile_idx));
                state.pixel_coords = None;
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
                    let src = src_selection.placements[y as usize][x as usize]
                        .context("transparent source tile")?;
                    let dst = dst_selection.placements[y as usize][x as usize]
                        .context("transparent destination tile")?;
                    let src_palette_id = src.palette;
                    let src_tile_idx = src.tile;
                    let src_flip = src.flip;
                    let dst_palette_id = dst.palette;
                    let dst_tile_idx = dst.tile;
                    let dst_flip = dst.flip;
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
                    let dst = dst_selection.placements[y as usize][x as usize]
                        .context("transparent destination tile")?;
                    let dst_palette_id = dst.palette;
                    let dst_tile_idx = dst.tile;
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
                        let dst = dst_selection.placements[y as usize][x as usize]
                            .context("transparent destination tile")?;
                        let dst_palette_id = dst.palette;
                        let dst_tile_idx = dst.tile;
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
            Task::none()
        }
    }
}

pub fn update_palette_order(state: &mut EditorState) {
    let id = state.palettes[state.palette_idx].id;
    state.palettes.sort_by_key(|x| x.id);
    state.palettes_id_idx_map.clear();
    for i in 0..state.palettes.len() {
        state.palettes_id_idx_map.insert(state.palettes[i].id, i);
        if state.palettes[i].id == id {
            state.palette_idx = i;
        }
    }
}

pub fn get_selected_gfx(state: &EditorState, s: &TileBlock) -> Vec<Vec<Option<Tile>>> {
    let mut gfx = vec![];
    for y in 0..s.size.1 {
        let mut gfx_row: Vec<Option<Tile>> = vec![];
        for x in 0..s.size.0 {
            let tile = if let Some(placement) = s.placements[y as usize][x as usize] {
                if let Some(&idx) = state.palettes_id_idx_map.get(&placement.palette) {
                    if (placement.tile as usize) < state.palettes[idx].tiles.len() {
                        Some(
                            placement
                                .flip
                                .apply_to_tile(state.palettes[idx].tiles[placement.tile as usize]),
                        )
                    } else {
                        Some(Tile::default())
                    }
                } else {
                    Some(Tile::default())
                }
            } else {
                None
            };
            gfx_row.push(tile);
        }
        gfx.push(gfx_row);
    }
    gfx
}
