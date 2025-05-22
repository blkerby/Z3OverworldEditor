use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
    sync::{Arc, Mutex},
};

use anyhow::{bail, Context, Result};
use hashbrown::{HashMap, HashSet};
use json_pretty_compact::PrettyCompactFormatter;
use log::info;
use notify::{recommended_watcher, EventHandler};
use serde::{de::DeserializeOwned, Serialize};
use serde_json::Serializer;

use crate::{
    helpers::scale_color,
    state::{
        ensure_areas_non_empty, ensure_palettes_non_empty, ensure_themes_non_empty, Area, AreaId,
        AreaPosition, EditorState, Flip, Palette, PaletteId, TileIdx,
    },
    update::update_palette_order,
};

fn save_json<T: Serialize>(path: &Path, data: &T) -> Result<()> {
    info!("Saving {}", path.display());
    let formatter = PrettyCompactFormatter::new().with_max_line_length(200);
    let mut data_bytes = vec![];
    let mut ser = Serializer::with_formatter(&mut data_bytes, formatter);
    data.serialize(&mut ser).unwrap();
    fs::create_dir_all(path.parent().context("invalid parent directory")?)?;
    fs::write(path, &data_bytes)?;
    Ok(())
}

fn load_json<T: DeserializeOwned>(path: &Path) -> Result<T> {
    info!("Loading {}", path.display());
    let data_bytes = std::fs::read(path)?;
    let data: T = serde_json::from_slice(&data_bytes)?;
    Ok(data)
}

pub fn load_global_config(state: &mut EditorState) -> Result<()> {
    state.global_config = load_json(&state.global_config_path)?;
    Ok(())
}

pub fn save_global_config(state: &mut EditorState) -> Result<()> {
    if state.global_config.modified {
        state.disable_watch_file_changes()?;
        save_json(&state.global_config_path, &state.global_config)?;
        state.enable_watch_file_changes()?;
        state.global_config.modified = false;
    }
    Ok(())
}

fn get_project_dir(state: &EditorState) -> Result<PathBuf> {
    Ok(state
        .global_config
        .project_dir
        .as_ref()
        .context("Project directory not set.")?
        .to_owned())
}

fn get_palette_dir(state: &EditorState) -> Result<PathBuf> {
    Ok(get_project_dir(state)?.join("Palettes"))
}

fn save_palette_colors_png(png_path: &Path, palette: &Palette) -> Result<()> {
    let pixel_size = 32;
    let color_bytes: Vec<[u8; 3]> = palette
        .colors
        .iter()
        .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
        .collect();

    let mut data: Vec<u8> = vec![];
    for _y in 0..pixel_size {
        for c in 0..16 {
            for _ in 0..pixel_size {
                data.extend(color_bytes[c]);
            }
        }
    }

    let path = Path::new(png_path);
    let file = File::create(path).unwrap();
    let ref mut w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, 16 * pixel_size as u32, pixel_size as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();

    Ok(())
}

fn save_palette_tiles_png(png_path: &Path, palette: &Palette) -> Result<()> {
    let color_bytes: Vec<[u8; 3]> = palette
        .colors
        .iter()
        .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
        .collect();
    let pixel_size = 4;

    let tiles = &palette.tiles;
    let num_cols = 16;
    let num_rows = (tiles.len() + num_cols - 1) / num_cols;

    let mut data: Vec<u8> = vec![];
    data.reserve_exact(num_rows * num_cols * 64 * 3);
    for y in 0..num_rows * (8 * pixel_size) {
        for x in 0..num_cols * (8 * pixel_size) {
            let tile_x = x / (8 * pixel_size);
            let tile_y = y / (8 * pixel_size);
            let pixel_x = x / pixel_size % 8;
            let pixel_y = y / pixel_size % 8;
            let tile_idx = tile_y * num_cols + tile_x;
            if tile_idx >= tiles.len() {
                data.extend([0, 0, 0, 0]);
                continue;
            }
            let tile = &palette.tiles[tile_idx];
            let color_idx = tile.pixels[pixel_y][pixel_x];
            let color = color_bytes[color_idx as usize];
            data.extend(&color);
        }
    }

    let path = Path::new(png_path);
    let file = File::create(path).unwrap();
    let ref mut w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(
        w,
        num_cols as u32 * 8 * pixel_size as u32,
        num_rows as u32 * 8 * pixel_size as u32,
    );
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();

    Ok(())
}

pub fn clear_pngs(state: &EditorState) -> Result<()> {
    let project_dir = state
        .global_config
        .project_dir
        .as_ref()
        .unwrap()
        .to_str()
        .unwrap();
    for path in glob::glob(&format!("{}/**/*.png", project_dir))? {
        let path = path?;
        info!("Removing file {}", path.to_str().unwrap());
        std::fs::remove_file(path)?;
    }
    Ok(())
}

pub fn save_palettes(state: &mut EditorState) -> Result<()> {
    let pal_dir = get_palette_dir(state)?;
    state.disable_watch_file_changes()?;
    for pal in &mut state.palettes {
        if pal.modified {
            let pal_json_filename = format!("{}.json", pal.name);
            let pal_json_path = pal_dir.join(pal_json_filename);
            for i in 0..pal.tiles.len() {
                pal.tiles[i].id = Some(i as TileIdx);
            }
            save_json(&pal_json_path, pal)?;

            let pal_colors_png_filename = format!("{}-colors.png", pal.name);
            let pal_colors_png_path = pal_dir.join(pal_colors_png_filename);
            save_palette_colors_png(&pal_colors_png_path, pal)?;

            let pal_tiles_png_filename = format!("{}-tiles.png", pal.name);
            let pal_tiles_png_path = pal_dir.join(pal_tiles_png_filename);
            save_palette_tiles_png(&pal_tiles_png_path, pal)?;

            pal.modified = false;
        }
    }
    state.enable_watch_file_changes()?;
    Ok(())
}

fn load_palettes(state: &mut EditorState) -> Result<()> {
    let pal_dir = get_palette_dir(state)?;
    let pattern = format!("{}/*.json", pal_dir.display());
    state.palettes.clear();
    for entry in glob::glob(&pattern)? {
        let path = entry?;
        let name = path
            .file_stem()
            .context(format!("bad file name: {}", path.display()))?
            .to_str()
            .context("bad file stem")?;
        let mut pal: Palette = load_json(&path)?;
        pal.name = name.to_owned();
        state.palettes.push(pal);
    }
    ensure_palettes_non_empty(state);
    update_palette_order(state);
    state.palette_idx = 0;
    Ok(())
}

pub fn delete_palette(state: &mut EditorState, name: &str) -> Result<()> {
    let pal_dir = get_palette_dir(state)?;
    let path = pal_dir.join(format!("{}.json", name));
    info!("Deleting {}", path.display());
    state.disable_watch_file_changes()?;
    std::fs::remove_file(path)?;
    state.enable_watch_file_changes()?;
    Ok(())
}

fn get_area_dir(state: &EditorState) -> Result<PathBuf> {
    Ok(get_project_dir(state)?.join("Areas"))
}

pub fn load_area_list(state: &mut EditorState) -> Result<()> {
    let area_dir = get_area_dir(state)?;

    let pattern = format!("{}/*/*.json", area_dir.display());
    state.theme_names.clear();
    state.area_names.clear();
    for entry in glob::glob(&pattern)? {
        let path = entry?;
        let theme_name = path.file_stem().unwrap().to_str().unwrap();
        let parent_path = path.parent().unwrap();
        let area_name = parent_path.file_name().unwrap().to_owned();
        state.theme_names.push(theme_name.to_string());
        state.area_names.push(area_name.into_string().unwrap());
    }
    ensure_themes_non_empty(state);
    ensure_areas_non_empty(state)?;
    state.area_names.sort();
    state.area_names.dedup();
    state.theme_names.sort();
    state.theme_names.dedup();
    Ok(())
}

pub fn load_area(state: &EditorState, area_id: &AreaId) -> Result<Area> {
    let area_path = get_area_dir(state)?
        .join(area_id.area.clone())
        .join(format!("{}.json", area_id.theme));
    let mut area: Area = load_json(&area_path)?;
    area.name = area_id.area.to_owned();
    area.theme = area_id.theme.to_owned();
    Ok(area)
}

pub fn save_area_png(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    let mut color_bytes: Vec<Vec<[u8; 3]>> = vec![];
    let area = &state.areas[area_id];
    for i in 0..state.palettes.len() {
        let mut colors = state.palettes[i].colors.clone();
        colors[0] = area.bg_color;
        let cb = colors
            .iter()
            .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
            .collect();
        color_bytes.push(cb);
    }

    let num_cols = area.size.1 as usize * 256;
    let num_rows = area.size.0 as usize * 256;
    let mut data: Vec<u8> = vec![0; num_rows * num_cols * 3];
    let col_stride = 3;
    let row_stride = num_cols * col_stride;
    for sy in 0..area.size.1 as usize {
        for sx in 0..area.size.0 as usize {
            let screen = &area.screens[sy * area.size.0 as usize + sx];
            let screen_addr = sy * 256 * row_stride + sx * 256 * col_stride;
            for ty in 0..32 {
                for tx in 0..32 {
                    let palette_id = screen.palettes[ty][tx];
                    let Some(&palette_idx) = state.palettes_id_idx_map.get(&palette_id) else {
                        // TODO: draw some indicator of the broken tile (due to invalid palette reference)
                        continue;
                    };
                    let tile_idx = screen.tiles[ty][tx];
                    if tile_idx as usize >= state.palettes[palette_idx].tiles.len() {
                        // TODO: draw some indicator of the broken tile (due to invalid palette reference)
                        continue;
                    }
                    let flip = screen.flips[ty][tx];
                    let tile = state.palettes[palette_idx].tiles[tile_idx as usize];
                    let tile = flip.apply_to_tile(tile);
                    let cb = &color_bytes[palette_idx];
                    let mut tile_addr = screen_addr + ty * 8 * row_stride + tx * 8 * col_stride;

                    for py in 0..8 {
                        let mut addr = tile_addr;
                        for px in 0..8 {
                            let color_idx = tile.pixels[py][px];
                            let color = cb[color_idx as usize];
                            data[addr..(addr + 3)].copy_from_slice(&color);
                            addr += 3;
                        }
                        tile_addr += row_stride;
                    }
                }
            }
        }
    }

    let area_dir = get_area_dir(state)?;
    let area_png_filename = format!("{}.png", area.theme);
    let area_png_path = area_dir.join(&area.name).join(area_png_filename);
    let file = File::create(&area_png_path).unwrap();
    let ref mut w = BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, num_cols as u32, num_rows as u32);
    encoder.set_color(png::ColorType::Rgb);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();

    Ok(())
}

pub fn save_area_json(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    let area_dir = get_area_dir(state)?;
    let area_json_filename = format!("{}.json", area_id.theme);
    let area_json_path = area_dir.join(&area_id.area).join(area_json_filename);
    save_json(&area_json_path, &state.areas[area_id])?;
    Ok(())
}

pub fn save_area(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    if state.areas[area_id].modified {
        state.disable_watch_file_changes()?;
        save_area_json(state, area_id)?;
        save_area_png(state, area_id)?;
        state.enable_watch_file_changes()?;
        state.areas.get_mut(area_id).unwrap().modified = false;
    }
    Ok(())
}

pub fn copy_area_theme(
    state: &mut EditorState,
    name: &str,
    old_theme: &str,
    new_theme: &str,
) -> Result<()> {
    let area_dir = get_area_dir(state)?.join(name);
    let old_area_path = area_dir.join(format!("{}.json", old_theme));
    let new_area_path = area_dir.join(format!("{}.json", new_theme));
    info!(
        "Copying {} to {}",
        old_area_path.display(),
        new_area_path.display()
    );
    state.disable_watch_file_changes()?;
    std::fs::copy(old_area_path, new_area_path)?;
    state.enable_watch_file_changes()?;
    Ok(())
}

pub fn rename_area(state: &mut EditorState, old_name: &str, new_name: &str) -> Result<()> {
    let old_area_path = get_area_dir(state)?.join(old_name);
    let new_area_path = get_area_dir(state)?.join(new_name);
    info!(
        "Renaming {} to {} (directory)",
        old_area_path.display(),
        new_area_path.display()
    );
    state.disable_watch_file_changes()?;
    std::fs::rename(old_area_path, new_area_path)?;
    state.enable_watch_file_changes()?;
    let keys: Vec<AreaId> = state
        .areas
        .keys()
        .filter(|x| x.area == old_name)
        .cloned()
        .collect();
    for k in keys {
        state.areas.remove(&k);
    }
    Ok(())
}

pub fn rename_area_theme(
    state: &mut EditorState,
    area_name: &str,
    old_theme: &str,
    new_theme: &str,
) -> Result<()> {
    let area_dir = get_area_dir(state)?.join(area_name);
    let old_area_path = area_dir.join(format!("{}.json", old_theme));
    let new_area_path = area_dir.join(format!("{}.json", new_theme));
    info!(
        "Renaming {} to {}",
        old_area_path.display(),
        new_area_path.display()
    );
    state.disable_watch_file_changes()?;
    std::fs::rename(old_area_path, new_area_path)?;
    state.enable_watch_file_changes()?;
    Ok(())
}

pub fn delete_area(state: &mut EditorState, name: &str) -> Result<()> {
    let area_path = get_area_dir(state)?.join(name);
    info!("Deleting {}", area_path.display());
    state.disable_watch_file_changes()?;
    std::fs::remove_dir_all(area_path)?;
    state.enable_watch_file_changes()?;
    let keys: Vec<AreaId> = state
        .areas
        .keys()
        .filter(|x| x.area == name)
        .cloned()
        .collect();
    for k in keys {
        state.areas.remove(&k);
    }
    Ok(())
}

pub fn delete_area_theme(state: &mut EditorState, area_name: &str, theme: &str) -> Result<()> {
    let area_dir = get_area_dir(state)?.join(area_name);
    let area_path = area_dir.join(format!("{}.json", theme));
    info!("Deleting {}", area_path.display());
    state.disable_watch_file_changes()?;
    std::fs::remove_file(area_path)?;
    state.enable_watch_file_changes()?;
    state.areas.remove(&AreaId {
        area: area_name.to_string(),
        theme: theme.to_string(),
    });
    Ok(())
}

pub fn scan_used_tiles(state: &mut EditorState) -> Result<HashSet<(PaletteId, TileIdx)>> {
    let area_names = state.area_names.clone();
    let theme_names = state.theme_names.clone();
    let mut out = HashSet::new();
    for area_name in &area_names {
        for theme_name in &theme_names {
            let area_id = AreaId {
                area: area_name.clone(),
                theme: theme_name.clone(),
            };
            let area =
                load_area(state, &area_id).context(format!("Error loading {:?}", area_id))?;
            for y in 0..area.size.1 as u16 * 32 {
                for x in 0..area.size.0 as u16 * 32 {
                    let pal = area.get_palette(x, y).unwrap();
                    let tile_idx = area.get_tile(x, y).unwrap();
                    out.insert((pal, tile_idx));
                }
            }
        }
    }
    Ok(out)
}

pub fn remap_tiles(
    state: &mut EditorState,
    map: &HashMap<(PaletteId, TileIdx), (PaletteId, TileIdx, Flip)>,
) -> Result<()> {
    let area_names = state.area_names.clone();
    let theme_names = state.theme_names.clone();
    for area_name in &area_names {
        for theme_name in &theme_names {
            let area_id = AreaId {
                area: area_name.clone(),
                theme: theme_name.clone(),
            };
            let mut area = load_area(state, &area_id)?;
            for y in 0..area.size.1 as u16 * 32 {
                for x in 0..area.size.0 as u16 * 32 {
                    let pal = area.get_palette(x, y).unwrap();
                    let tile_idx = area.get_tile(x, y).unwrap();
                    if let Some(&(p, t, f)) = map.get(&(pal, tile_idx)) {
                        let flip = area.get_flip(x, y)?;
                        area.set_palette(x, y, p)?;
                        area.set_tile(x, y, t)?;
                        area.set_flip(x, y, f.apply_to_flip(flip))?;
                        area.modified = true;
                    }
                }
            }
            state.areas.insert(area_id.clone(), area);
            save_area(state, &area_id)?;
            state.cleanup_areas()?;
        }
    }
    Ok(())
}

pub fn save_project(state: &mut EditorState) -> Result<()> {
    if state.global_config.project_dir.is_none() {
        return Ok(());
    }
    save_global_config(state)?;
    save_palettes(state)?;
    save_area(state, &state.main_area_id.clone())?;
    save_area(state, &state.side_area_id.clone())?;
    Ok(())
}

struct FileModificationHandler {
    modified: Arc<Mutex<bool>>,
}

impl FileModificationHandler {
    fn new(modified: Arc<Mutex<bool>>) -> Self {
        FileModificationHandler { modified }
    }
}

impl EventHandler for FileModificationHandler {
    fn handle_event(&mut self, event: notify::Result<notify::Event>) {
        let Ok(e) = event else {
            return;
        };
        match e.kind {
            notify::EventKind::Modify(_) => {
                let mut data = self.modified.lock().unwrap();
                *data = true;
            }
            _ => {}
        }
    }
}

pub fn load_project(state: &mut EditorState) -> Result<()> {
    if state.global_config.project_dir.is_none() {
        bail!("Project directory not set");
    }
    if !state.global_config.project_dir.as_ref().unwrap().exists() {
        bail!(
            "Project directory does not exist: {}",
            state.global_config.project_dir.as_ref().unwrap().display()
        );
    }

    // Set up watcher on the project directories:
    let watch_locations = ["Areas", "Palettes"];
    state.watch_paths.clear();
    for loc in watch_locations {
        state
            .watch_paths
            .push(state.global_config.project_dir.as_ref().unwrap().join(loc));
    }
    state.watcher = Some(recommended_watcher(FileModificationHandler::new(
        state.files_modified_notification.clone(),
    ))?);
    state.watch_enabled = false;
    state.enable_watch_file_changes()?;

    load_palettes(state)?;
    load_area_list(state)?;
    let area_id = AreaId {
        area: state.area_names[0].clone(),
        theme: state.theme_names[0].clone(),
    };
    state.load_area(&area_id)?;
    state.switch_area(AreaPosition::Main, &area_id)?;
    state.switch_area(AreaPosition::Side, &area_id)?;
    state.palette_idx = 0;
    state.color_idx = None;
    state.tile_idx = None;
    state.undo_stack.clear();
    state.redo_stack.clear();
    Ok(())
}
