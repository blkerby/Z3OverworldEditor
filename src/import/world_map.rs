use std::cmp::Reverse;

use hashbrown::HashSet;

use super::{strip_tile, Constants, PcAddr, Rom, SnesAddr};
use crate::{
    persist::{save_area_json, save_area_png},
    state::{
        Area, AreaPosition, Background, BackgroundLayering, EditorState, Layer, MapUse, MapWorld,
        Palette, Tile, TilePlacement, World,
    },
    update::update_palette_order,
};
use anyhow::{ensure, Context, Result};
use log::info;

// Two words cover all 256 possible Mode 7 tiles; a palette's color set fits in one.
fn find_palette_cover(
    remaining: [u128; 2],
    candidates: &[([u128; 2], u128)],
    choices: &[Vec<usize>],
    slots: usize,
    failed: &mut HashSet<([u128; 2], usize)>,
) -> Option<Vec<u128>> {
    if remaining == [0, 0] {
        return Some(vec![]);
    }
    if slots == 0 || failed.contains(&(remaining, slots)) {
        return None;
    }

    let mut max_covered = 0;
    for &(cover, _) in candidates {
        let covered =
            (cover[0] & remaining[0]).count_ones() + (cover[1] & remaining[1]).count_ones();
        max_covered = max_covered.max(covered);
    }
    if remaining[0].count_ones() + remaining[1].count_ones() > max_covered * slots as u32 {
        return None;
    }

    // Branch on the tile with the fewest possible palettes.
    let mut options: Option<&Vec<usize>> = None;
    for (i, tile_choices) in choices.iter().enumerate() {
        if remaining[i / 128] & (1 << (i % 128)) != 0
            && options.is_none_or(|previous| tile_choices.len() < previous.len())
        {
            options = Some(tile_choices);
        }
    }
    let mut options = options.unwrap().clone();
    options.sort_by_key(|&i| {
        let cover = candidates[i].0;
        Reverse((cover[0] & remaining[0]).count_ones() + (cover[1] & remaining[1]).count_ones())
    });
    let mut tried = HashSet::new();
    for i in options {
        let (cover, colors) = candidates[i];
        let next = [remaining[0] & !cover[0], remaining[1] & !cover[1]];
        if tried.insert(next) {
            if let Some(mut solution) =
                find_palette_cover(next, candidates, choices, slots - 1, failed)
            {
                solution.push(colors);
                return Some(solution);
            }
        }
    }
    failed.insert((remaining, slots));
    None
}

// shortcut: Exhaustive packing targets vanilla's small color sets; revisit for custom Mode 7 artwork.
fn pack_palette_colors(tile_colors: &[u128]) -> Result<Vec<u128>> {
    let mut distinct = tile_colors.to_vec();
    distinct.sort_unstable();
    distinct.dedup();
    let mut required = Vec::new();
    let mut all_colors = 0;
    for &colors in &distinct {
        all_colors |= colors;
        if !distinct
            .iter()
            .any(|&other| colors != other && colors & other == colors)
        {
            required.push(colors);
        }
    }

    // Enumerate feasible unions. Subset palettes cannot improve a minimum cover.
    let mut unions: HashSet<u128> = HashSet::from([0u128]);
    for &colors in &required {
        for previous in unions.clone() {
            let combined = previous | colors;
            if combined.count_ones() <= 15 {
                unions.insert(combined);
            }
        }
    }
    let mut unions: Vec<u128> = unions.into_iter().collect();
    unions.sort_unstable_by_key(|&colors| (Reverse(colors.count_ones()), colors));
    let mut candidates: Vec<([u128; 2], u128)> = Vec::new();
    let mut choices = vec![Vec::new(); required.len()];
    for colors in unions {
        if candidates
            .iter()
            .any(|&(_, other)| colors & other == colors)
        {
            continue;
        }
        let mut cover = [0, 0];
        for (i, &tile) in required.iter().enumerate() {
            if colors & tile == tile {
                cover[i / 128] |= 1 << (i % 128);
                choices[i].push(candidates.len());
            }
        }
        candidates.push((cover, colors));
    }
    let mut remaining = [0, 0];
    for i in 0..required.len() {
        remaining[i / 128] |= 1 << (i % 128);
    }
    let mut failed = HashSet::new();
    for count in all_colors.count_ones().div_ceil(15).max(1)..=8 {
        if let Some(colors) = find_palette_cover(
            remaining,
            &candidates,
            &choices,
            count as usize,
            &mut failed,
        ) {
            return Ok(colors);
        }
    }
    anyhow::bail!("Mode 7 map graphics require more than eight 4bpp palettes")
}

pub(super) fn import_world_maps(
    rom: &Rom,
    constants: &Constants,
    state: &mut EditorState,
    theme: &str,
) -> Result<()> {
    let project_dir = state
        .global_config
        .project_dir
        .as_ref()
        .context("Project directory not set")?
        .clone();
    let palettes = &mut state.palettes;
    let gfx = rom.read_n(SnesAddr(0x18C000).into(), 0x4000)?;
    let map_addr: PcAddr = constants.world_map_addr.into();
    let light_map = rom.read_n(map_addr, 0x1000)?;
    let dark_map = rom.read_n(map_addr + 0x1000, 0x400)?;
    let mut imported = Vec::new();
    let mut areas = Vec::new();
    let mut next_id = palettes.iter().map(|p| p.id).max().unwrap_or(0) as u32 + 1;

    for (world, name, palette_offset) in [
        (MapWorld::Light, "Light", 0),
        (MapWorld::Dark, "Dark", 0x100),
    ] {
        let mut used = [false; 256];
        let mut map_tiles = [[0u8; 64]; 64];
        used[0xEF] = true; // Vanilla fills the rest of the 128×128 map with this tile.
        for y in 0..64 {
            for x in 0..64 {
                // Dark World replaces only the central 32×32 of the Light World map.
                let tile =
                    if world == MapWorld::Dark && (16..48).contains(&x) && (16..48).contains(&y) {
                        dark_map[(y - 16) * 32 + x - 16]
                    } else {
                        light_map[(y / 32 * 2 + x / 32) * 1024 + y % 32 * 32 + x % 32]
                    };
                used[tile as usize] = true;
                map_tiles[y][x] = tile;
            }
        }
        let mut source_colors = [0u16; 128];
        for (i, color) in source_colors.iter_mut().enumerate() {
            *color = rom.read_u16(map_addr + 0x1400 + palette_offset + i as u32 * 2)? & 0x7FFF;
        }
        let mut colors = source_colors.to_vec();
        colors.sort_unstable();
        colors.dedup();
        let mut color_bits = [0u128; 128];
        for i in 1..128 {
            color_bits[i] = 1 << colors.binary_search(&source_colors[i]).unwrap();
        }
        let mut source_tiles = Vec::new();
        let mut source_ids = Vec::new();
        let mut tile_colors = Vec::new();
        for (id, &is_used) in used.iter().enumerate() {
            if !is_used {
                continue;
            }
            let pixels = &gfx[id * 64..id * 64 + 64];
            let mut mask = 0u128;
            for &pixel in pixels {
                ensure!(
                    pixel < 128,
                    "{name} World map tile {id:02X} uses sprite palette colors"
                );
                mask |= color_bits[pixel as usize];
            }
            ensure!(
                mask.count_ones() <= 15,
                "{name} World map tile {id:02X} needs more than fifteen opaque colors"
            );
            source_tiles.push(pixels);
            source_ids.push(id);
            tile_colors.push(mask);
        }
        let packed =
            pack_palette_colors(&tile_colors).with_context(|| format!("{name} World map"))?;
        let mut world_palettes = Vec::new();
        let mut remaps = Vec::new();
        let mut new_count = 0;
        for &mask in &packed {
            let mut palette_colors = [[0; 3]; 16];
            let mut color_indices = [0u8; 128];
            let mut index = 1;
            for (i, &color) in colors.iter().enumerate() {
                if mask & (1 << i) != 0 {
                    palette_colors[index] = [
                        (color & 31) as u8,
                        ((color >> 5) & 31) as u8,
                        (color >> 10) as u8,
                    ];
                    color_indices[i] = index as u8;
                    index += 1;
                }
            }
            let mut remap = [0; 128];
            for i in 1..128 {
                remap[i] = color_indices[colors.binary_search(&source_colors[i]).unwrap()];
            }
            remaps.push(remap);
            if let Some(existing) = palettes
                .iter()
                .find(|p| p.map_world == Some(world) && p.colors[1..] == palette_colors[1..])
            {
                world_palettes.push(existing.clone());
            } else {
                let mut number = world_palettes.len() + 1;
                let mut palette_name = format!("{name} World Map {number}");
                while palettes
                    .iter()
                    .chain(&world_palettes)
                    .any(|p| p.name == palette_name)
                {
                    number += 1;
                    palette_name = format!("{name} World Map {number}");
                }
                world_palettes.push(Palette {
                    id: next_id.try_into().context("No unused palette IDs remain")?,
                    name: palette_name,
                    modified: true,
                    map_world: Some(world),
                    colors: palette_colors,
                    ..Palette::default()
                });
                next_id += 1;
                new_count += 1;
            }
        }
        let existing_count = palettes
            .iter()
            .filter(|p| p.map_world == Some(world))
            .count();
        ensure!(
            existing_count + new_count <= 8,
            "Import would exceed eight {name} World map palettes; existing artwork was preserved"
        );
        let mut added_tiles = 0;
        let mut placements = [TilePlacement::default(); 256];
        for (i, (pixels, mask)) in source_tiles.iter().zip(&tile_colors).enumerate() {
            let palette_idx = packed
                .iter()
                .position(|colors| colors & mask == *mask)
                .unwrap();
            let palette = &mut world_palettes[palette_idx];
            let mut tile = Tile {
                map_use: Some(MapUse::Terrain),
                ..Tile::default()
            };
            for (i, &pixel) in pixels.iter().enumerate() {
                tile.pixels[i / 8][i % 8] = remaps[palette_idx][pixel as usize];
            }
            let tile_idx = if let Some(index) = palette
                .tiles
                .iter()
                .position(|&existing| strip_tile(existing) == tile)
            {
                index
            } else {
                palette.tiles.push(tile);
                palette.modified = true;
                added_tiles += 1;
                palette.tiles.len() - 1
            };
            placements[source_ids[i]] = TilePlacement {
                palette: palette.id,
                tile: tile_idx as u16,
                ..TilePlacement::default()
            };
        }
        let mut terrain_count = added_tiles;
        for palette in palettes.iter() {
            if palette.map_world == Some(world) {
                terrain_count += palette
                    .tiles
                    .iter()
                    .filter(|tile| tile.map_use == Some(MapUse::Terrain))
                    .count();
            }
        }
        ensure!(
            terrain_count <= 1024,
            "Import would exceed 1024 {name} World terrain tiles; existing artwork was preserved"
        );
        info!("{name} World map: {} source tiles in {} palettes; added {new_count} palettes and {added_tiles} tiles", source_tiles.len(), packed.len());
        imported.extend(world_palettes);
        let mut tiles = vec![vec![None; 64]; 64];
        for y in 0..64 {
            for x in 0..64 {
                tiles[y][x] = Some(placements[map_tiles[y][x] as usize]);
            }
        }
        let area_name = format!("{name} World Map");
        for area_theme in &state.theme_names {
            // New areas need every theme; later imports replace only the selected theme.
            if area_theme != theme
                && project_dir
                    .join("Areas")
                    .join(&area_name)
                    .join(format!("{area_theme}.json"))
                    .exists()
            {
                continue;
            }
            let color = source_colors[0];
            areas.push(Area {
                modified: true,
                name: area_name.clone(),
                theme: area_theme.clone(),
                vanilla_map_id: None,
                world: if world == MapWorld::Light {
                    World::Light
                } else {
                    World::Dark
                },
                other_world_area: Some(
                    if world == MapWorld::Light {
                        "Dark World Map"
                    } else {
                        "Light World Map"
                    }
                    .to_owned(),
                ),
                bg_color: [
                    (color & 31) as u8,
                    ((color >> 5) & 31) as u8,
                    (color >> 10) as u8,
                ],
                bg_layering: BackgroundLayering::None,
                bg_camera_follow_x: 1.0,
                bg_camera_drift_x: 0.0,
                bg_camera_follow_y: 1.0,
                bg_camera_drift_y: 0.0,
                size: (2, 2),
                layers: vec![Layer {
                    modified: true,
                    name: "Main".to_owned(),
                    condition: String::new(),
                    background: Background::Bg2,
                    tiles: tiles.clone(),
                    map: None,
                }],
            });
        }
    }
    // Commit only after both worlds convert successfully.
    for palette in imported {
        if let Some(existing) = palettes.iter_mut().find(|p| p.id == palette.id) {
            *existing = palette;
        } else {
            palettes.push(palette);
        }
    }
    update_palette_order(state);
    for area in areas {
        let id = area.id();
        if !state.area_names.contains(&area.name) {
            state.area_names.push(area.name.clone());
        }
        state.set_area(AreaPosition::Main, area)?;
        save_area_json(state, &id)?;
        save_area_png(state, &id)?;
        state.areas.get_mut(&id).unwrap().modified = false;
    }
    state.area_names.sort();
    Ok(())
}
