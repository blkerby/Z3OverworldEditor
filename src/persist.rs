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
use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::Serializer;

use crate::{
    helpers::scale_color,
    state::{
        ensure_areas_non_empty, ensure_palettes_non_empty, ensure_themes_non_empty,
        is_valid_layer_name, Area, AreaId, AreaPosition, Background, BackgroundLayering,
        DynamicTiles, EditorState, Flip, Layer, Palette, PaletteId, TileIdx, TilePlacement,
    },
    update::update_palette_order,
};

pub(crate) fn save_json<T: Serialize>(path: &Path, data: &T) -> Result<()> {
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

fn get_dynamic_tiles_path(state: &EditorState) -> Result<PathBuf> {
    Ok(get_project_dir(state)?
        .join("DynamicTiles")
        .join("replacements.json"))
}

fn load_dynamic_tiles(state: &mut EditorState) -> Result<()> {
    let path = get_dynamic_tiles_path(state)?;
    if !path.exists() {
        state.dynamic_tiles = DynamicTiles::default();
        return Ok(());
    }

    let mut dynamic_tiles: DynamicTiles = load_json(&path)?;
    let mut kinds = HashSet::new();
    for group in &dynamic_tiles.groups {
        if !kinds.insert(group.kind) {
            bail!("duplicate dynamic tile type: {}", group.kind);
        }
        let (width, height) = group.kind.size();
        for variant in &group.variants {
            if variant.before.tiles.len() != height
                || variant.before.tiles.iter().any(|row| row.len() != width)
            {
                bail!("invalid Before grid size for {}", group.kind);
            }
            if variant.after_frames.len() != group.kind.after_frame_count() {
                bail!("invalid After frame count for {}", group.kind);
            }
            for frame in &variant.after_frames {
                if frame.tiles.len() != height || frame.tiles.iter().any(|row| row.len() != width) {
                    bail!("invalid After grid size for {}", group.kind);
                }
            }
        }
    }
    dynamic_tiles.groups.sort_by_key(|group| group.kind);
    dynamic_tiles.modified = false;
    state.dynamic_tiles = dynamic_tiles;
    Ok(())
}

fn save_dynamic_tiles(state: &mut EditorState) -> Result<()> {
    if state.dynamic_tiles.modified {
        state.disable_watch_file_changes()?;
        state.dynamic_tiles.groups.sort_by_key(|group| group.kind);
        save_json(&get_dynamic_tiles_path(state)?, &state.dynamic_tiles)?;
        state.dynamic_tiles.modified = false;
        state.enable_watch_file_changes()?;
    }
    Ok(())
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
        for color in color_bytes.iter() {
            for _ in 0..pixel_size {
                data.extend(color);
            }
        }
    }

    let path = Path::new(png_path);
    let file = File::create(path).unwrap();
    let w = &mut BufWriter::new(file);
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
    let num_rows = tiles.len().div_ceil(num_cols);

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
    let w = &mut BufWriter::new(file);
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
        pal.validate_animated_tile_groups()
            .with_context(|| format!("invalid palette: {}", path.display()))?;
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

#[derive(Serialize, Deserialize)]
struct StoredArea {
    vanilla_map_id: Option<u8>,
    bg_color: [u8; 3],
    #[serde(default)]
    bg_layering: BackgroundLayering,
    #[serde(default = "default_bg_camera_follow")]
    bg_camera_follow_x: f32,
    #[serde(default)]
    bg_camera_drift_x: f32,
    #[serde(default = "default_bg_camera_follow")]
    bg_camera_follow_y: f32,
    #[serde(default)]
    bg_camera_drift_y: f32,
    size: (u8, u8),
    layers: Vec<StoredLayer>,
}

fn default_bg_camera_follow() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize)]
struct StoredLayer {
    name: String,
    background: Background,
    screens: Vec<StoredScreen>,
}

#[derive(Serialize, Deserialize)]
struct StoredScreen {
    position: (u16, u16),
    size: (u16, u16),
    palettes: Vec<Vec<Option<PaletteId>>>,
    tiles: Vec<Vec<Option<TileIdx>>>,
    flips: Vec<Vec<Option<Flip>>>,
}

pub fn load_area(state: &EditorState, area_id: &AreaId) -> Result<Area> {
    let area_path = get_area_dir(state)?
        .join(area_id.area.clone())
        .join(format!("{}.json", area_id.theme));
    let stored: StoredArea = load_json(&area_path)?;
    let width = stored.size.0 as usize * 32;
    let height = stored.size.1 as usize * 32;
    if width == 0 || height == 0 {
        bail!("area size must be nonzero");
    }
    for follow in [stored.bg_camera_follow_x, stored.bg_camera_follow_y] {
        if ![0.0, 0.25, 0.5, 1.0, 1.5].contains(&follow) {
            bail!("invalid background camera follow: {}", follow);
        }
    }
    for drift in [stored.bg_camera_drift_x, stored.bg_camera_drift_y] {
        if !(-512.0..=512.0).contains(&drift) {
            bail!("invalid background camera drift: {}", drift);
        }
    }
    let mut names = HashSet::new();
    let mut has_bg2 = false;
    let mut layers = Vec::with_capacity(stored.layers.len());

    for stored_layer in stored.layers {
        if !is_valid_layer_name(&stored_layer.name) {
            bail!("invalid layer name: {}", stored_layer.name);
        }
        if !names.insert(stored_layer.name.clone()) {
            bail!("duplicate layer name: {}", stored_layer.name);
        }
        if stored_layer.background == Background::Bg2 {
            has_bg2 = true;
        }

        let mut tiles = vec![vec![None; width]; height];
        let mut covered = vec![vec![false; width]; height];
        for screen in stored_layer.screens {
            let screen_width = screen.size.0 as usize;
            let screen_height = screen.size.1 as usize;
            if screen_width == 0
                || screen_height == 0
                || screen.palettes.len() != screen_height
                || screen.tiles.len() != screen_height
                || screen.flips.len() != screen_height
            {
                bail!("invalid screen size in layer {}", stored_layer.name);
            }
            let end_x = screen.position.0 as usize + screen_width;
            let end_y = screen.position.1 as usize + screen_height;
            if end_x > width || end_y > height {
                bail!("screen outside area in layer {}", stored_layer.name);
            }
            for y in 0..screen_height {
                if screen.palettes[y].len() != screen_width
                    || screen.tiles[y].len() != screen_width
                    || screen.flips[y].len() != screen_width
                {
                    bail!("invalid screen row size in layer {}", stored_layer.name);
                }
                for x in 0..screen_width {
                    let area_x = screen.position.0 as usize + x;
                    let area_y = screen.position.1 as usize + y;
                    if covered[area_y][area_x] {
                        bail!("overlapping screens in layer {}", stored_layer.name);
                    }
                    covered[area_y][area_x] = true;
                    tiles[area_y][area_x] = match (
                        screen.palettes[y][x],
                        screen.tiles[y][x],
                        screen.flips[y][x],
                    ) {
                        (Some(palette), Some(tile), Some(flip)) => Some(TilePlacement {
                            palette,
                            tile,
                            flip,
                        }),
                        (None, None, None) => None,
                        _ => bail!("mismatched transparent cell in layer {}", stored_layer.name),
                    };
                }
            }
        }
        layers.push(Layer {
            modified: false,
            name: stored_layer.name,
            background: stored_layer.background,
            tiles,
        });
    }
    if layers.is_empty() {
        bail!("area has no layers");
    }
    if !has_bg2 {
        bail!("area has no BG2 layer");
    }

    Ok(Area {
        modified: false,
        name: area_id.area.clone(),
        theme: area_id.theme.clone(),
        vanilla_map_id: stored.vanilla_map_id,
        bg_color: stored.bg_color,
        bg_layering: stored.bg_layering,
        bg_camera_follow_x: stored.bg_camera_follow_x,
        bg_camera_drift_x: stored.bg_camera_drift_x,
        bg_camera_follow_y: stored.bg_camera_follow_y,
        bg_camera_drift_y: stored.bg_camera_drift_y,
        size: stored.size,
        layers,
    })
}

fn save_layer_png(state: &EditorState, area_id: &AreaId, layer_idx: usize) -> Result<()> {
    let mut color_bytes: Vec<Vec<[u8; 3]>> = vec![];
    let area = &state.areas[area_id];
    let layer = &area.layers[layer_idx];
    for i in 0..state.palettes.len() {
        let cb = state.palettes[i]
            .colors
            .iter()
            .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
            .collect();
        color_bytes.push(cb);
    }

    let num_cols = area.size.0 as usize * 256;
    let num_rows = area.size.1 as usize * 256;
    let [r, g, b] = area.bg_color.map(scale_color);
    let mut data = [r, g, b, 255].repeat(num_rows * num_cols);
    let col_stride = 4;
    let row_stride = num_cols * col_stride;
    for (ty, row) in layer.tiles.iter().enumerate() {
        for (tx, placement) in row.iter().enumerate() {
            let Some(placement) = placement else {
                continue;
            };
            let Some(&palette_idx) = state.palettes_id_idx_map.get(&placement.palette) else {
                continue;
            };
            if placement.tile as usize >= state.palettes[palette_idx].tiles.len() {
                continue;
            }
            let tile = placement
                .flip
                .apply_to_tile(state.palettes[palette_idx].tiles[placement.tile as usize]);
            let cb = &color_bytes[palette_idx];
            let mut tile_addr = ty * 8 * row_stride + tx * 8 * col_stride;
            for py in 0..8 {
                let mut addr = tile_addr;
                for px in 0..8 {
                    let color_idx = tile.pixels[py][px];
                    if color_idx != 0 {
                        data[addr..addr + 3].copy_from_slice(&cb[color_idx as usize]);
                        data[addr + 3] = 255;
                    }
                    addr += 4;
                }
                tile_addr += row_stride;
            }
        }
    }

    let area_png_path = get_area_dir(state)?
        .join(&area.name)
        .join(&area.theme)
        .join(format!("{}.png", layer.name));
    fs::create_dir_all(area_png_path.parent().context("invalid PNG path")?)?;
    let file = File::create(&area_png_path).unwrap();
    let w = &mut BufWriter::new(file);
    let mut encoder = png::Encoder::new(w, num_cols as u32, num_rows as u32);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&data).unwrap();

    Ok(())
}

pub fn delete_layer_png(state: &mut EditorState, area_id: &AreaId, name: &str) -> Result<()> {
    let path = get_area_dir(state)?
        .join(&area_id.area)
        .join(&area_id.theme)
        .join(format!("{name}.png"));
    state.disable_watch_file_changes()?;
    let result = fs::remove_file(path);
    state.enable_watch_file_changes()?;
    match result {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => return Err(error.into()),
    }
    Ok(())
}

pub fn save_area_png(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    for layer_idx in 0..state.areas[area_id].layers.len() {
        save_layer_png(state, area_id, layer_idx)?;
    }
    for layer in &mut state.areas.get_mut(area_id).unwrap().layers {
        layer.modified = false;
    }
    Ok(())
}

pub fn save_area_json(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    let area_dir = get_area_dir(state)?;
    let area_json_filename = format!("{}.json", area_id.theme);
    let area_json_path = area_dir.join(&area_id.area).join(area_json_filename);
    let area = &state.areas[area_id];
    let mut stored_layers = Vec::with_capacity(area.layers.len());
    for layer in &area.layers {
        let mut screens = Vec::new();
        let height = layer.tiles.len();
        let width = layer.tiles.first().map_or(0, Vec::len);
        for base_y in (0..height).step_by(32) {
            for base_x in (0..width).step_by(32) {
                let end_y = (base_y + 32).min(height);
                let end_x = (base_x + 32).min(width);
                let mut left = end_x;
                let mut right = base_x;
                let mut top = end_y;
                let mut bottom = base_y;
                for y in base_y..end_y {
                    for x in base_x..end_x {
                        if layer.tiles[y][x].is_some() {
                            left = left.min(x);
                            right = right.max(x);
                            top = top.min(y);
                            bottom = bottom.max(y);
                        }
                    }
                }
                if left == end_x {
                    continue;
                }

                let mut palettes = Vec::new();
                let mut tiles = Vec::new();
                let mut flips = Vec::new();
                for y in top..=bottom {
                    let mut palette_row = Vec::new();
                    let mut tile_row = Vec::new();
                    let mut flip_row = Vec::new();
                    for x in left..=right {
                        let placement = layer.tiles[y][x];
                        palette_row.push(placement.map(|placement| placement.palette));
                        tile_row.push(placement.map(|placement| placement.tile));
                        flip_row.push(placement.map(|placement| placement.flip));
                    }
                    palettes.push(palette_row);
                    tiles.push(tile_row);
                    flips.push(flip_row);
                }
                screens.push(StoredScreen {
                    position: (left as u16, top as u16),
                    size: ((right - left + 1) as u16, (bottom - top + 1) as u16),
                    palettes,
                    tiles,
                    flips,
                });
            }
        }
        stored_layers.push(StoredLayer {
            name: layer.name.clone(),
            background: layer.background,
            screens,
        });
    }
    let stored = StoredArea {
        vanilla_map_id: area.vanilla_map_id,
        bg_color: area.bg_color,
        bg_layering: area.bg_layering,
        bg_camera_follow_x: area.bg_camera_follow_x,
        bg_camera_drift_x: area.bg_camera_drift_x,
        bg_camera_follow_y: area.bg_camera_follow_y,
        bg_camera_drift_y: area.bg_camera_drift_y,
        size: area.size,
        layers: stored_layers,
    };
    save_json(&area_json_path, &stored)?;
    Ok(())
}

pub fn save_area(state: &mut EditorState, area_id: &AreaId) -> Result<()> {
    let json_modified = state.areas[area_id].modified;
    let mut modified_layers = Vec::new();
    for (idx, layer) in state.areas[area_id].layers.iter().enumerate() {
        if layer.modified {
            modified_layers.push(idx);
        }
    }
    if json_modified || !modified_layers.is_empty() {
        state.disable_watch_file_changes()?;
        if json_modified {
            save_area_json(state, area_id)?;
        }
        for layer_idx in modified_layers.iter().copied() {
            save_layer_png(state, area_id, layer_idx)?;
        }
        state.enable_watch_file_changes()?;
        let area = state.areas.get_mut(area_id).unwrap();
        area.modified = false;
        for layer_idx in modified_layers {
            area.layers[layer_idx].modified = false;
        }
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
            for layer in &area.layers {
                for row in &layer.tiles {
                    for placement in row.iter().flatten() {
                        out.insert((placement.palette, placement.tile));
                    }
                }
            }
        }
    }
    for group in &state.dynamic_tiles.groups {
        for variant in &group.variants {
            for row in &variant.before.tiles {
                for placement in row.iter().flatten() {
                    out.insert((placement.palette, placement.tile));
                }
            }
            for frame in &variant.after_frames {
                for row in &frame.tiles {
                    for placement in row.iter().flatten() {
                        out.insert((placement.palette, placement.tile));
                    }
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
            for layer in &mut area.layers {
                for row in &mut layer.tiles {
                    for placement in row.iter_mut().flatten() {
                        if let Some(&(palette, tile, flip)) =
                            map.get(&(placement.palette, placement.tile))
                        {
                            placement.palette = palette;
                            placement.tile = tile;
                            placement.flip = flip.apply_to_flip(placement.flip);
                            layer.modified = true;
                            area.modified = true;
                        }
                    }
                }
            }
            state.areas.insert(area_id.clone(), area);
            save_area(state, &area_id)?;
            state.cleanup_areas()?;
        }
    }

    let mut dynamic_tiles_modified = false;
    for group in &mut state.dynamic_tiles.groups {
        for variant in &mut group.variants {
            for row in &mut variant.before.tiles {
                for placement in row.iter_mut().flatten() {
                    if let Some(&(palette, tile, flip)) =
                        map.get(&(placement.palette, placement.tile))
                    {
                        placement.palette = palette;
                        placement.tile = tile;
                        placement.flip = flip.apply_to_flip(placement.flip);
                        dynamic_tiles_modified = true;
                    }
                }
            }
            for frame in &mut variant.after_frames {
                for row in &mut frame.tiles {
                    for placement in row.iter_mut().flatten() {
                        if let Some(&(palette, tile, flip)) =
                            map.get(&(placement.palette, placement.tile))
                        {
                            placement.palette = palette;
                            placement.tile = tile;
                            placement.flip = flip.apply_to_flip(placement.flip);
                            dynamic_tiles_modified = true;
                        }
                    }
                }
            }
        }
    }
    state.dynamic_tiles.modified |= dynamic_tiles_modified;
    Ok(())
}

pub fn save_project(state: &mut EditorState) -> Result<()> {
    if state.global_config.project_dir.is_none() {
        return Ok(());
    }
    save_global_config(state)?;
    save_palettes(state)?;
    save_dynamic_tiles(state)?;
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
        if let notify::EventKind::Modify(_) = e.kind {
            let mut data = self.modified.lock().unwrap();
            *data = true;
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
    state.watch_paths.clear();
    state
        .watch_paths
        .push(state.global_config.project_dir.as_ref().unwrap().clone());
    state.watcher = Some(recommended_watcher(FileModificationHandler::new(
        state.files_modified_notification.clone(),
    ))?);
    state.watch_enabled = false;
    state.enable_watch_file_changes()?;

    load_palettes(state)?;
    load_dynamic_tiles(state)?;
    load_area_list(state)?;
    let area_id = AreaId {
        area: state.area_names[0].clone(),
        theme: state.theme_names[0].clone(),
    };
    state.load_area(&area_id)?;
    state.switch_area(AreaPosition::Main, &area_id)?;
    state.switch_area(AreaPosition::Side, &area_id)?;
    state.reset_layer_state(AreaPosition::Main);
    state.reset_layer_state(AreaPosition::Side);
    state.palette_idx = 0;
    state.color_idx = None;
    state.tile_idx = None;
    state.undo_stack.clear();
    state.redo_stack.clear();
    state.dynamic_tiles_open = false;
    state.dynamic_tile_frames.clear();
    Ok(())
}
