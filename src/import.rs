use anyhow::{bail, ensure, Result};
use hashbrown::{hash_map::Entry, HashMap};
use itertools::Itertools;
use log::{info, warn};
use std::{
    fmt::Display,
    ops::{Add, AddAssign},
    path::Path,
};

use crate::{
    persist::{load_area, load_project, save_area_json, save_area_png, save_project},
    state::{
        AnimatedTileGroup, Area, AreaId, AreaName, Background, BackgroundLayering, ColorRGB,
        ColorValue, DynamicTileGrid, DynamicTileGroup, DynamicTileType, DynamicTileVariant,
        DynamicTiles, EditorState, Flip, Layer, Palette, PaletteId, Tile, TileIdx, TilePixels,
        TilePlacement,
    },
    update::update_palette_order,
};

const ANIMATED_TILE_START: u16 = 0x1C0;
const ANIMATED_TILE_COUNT: u16 = 32;
const AREA_MAP_COUNT: u32 = 0x90;
const MAP_COUNT: u32 = 0xA0;

fn animated_tile_slot(gfx_char: u16) -> Option<u16> {
    gfx_char
        .checked_sub(ANIMATED_TILE_START)
        .filter(|&slot| slot < ANIMATED_TILE_COUNT)
}

fn add_vanilla_animated_tile_groups(palette: &mut Palette) -> TileIdx {
    palette
        .tiles
        .resize(palette.tiles.len().div_ceil(16) * 16, Tile::default());
    let base_tile = palette.tiles.len() as TileIdx;
    palette.tiles.resize(
        palette.tiles.len() + ANIMATED_TILE_COUNT as usize,
        Tile::default(),
    );

    for row in 0..2 {
        palette.animated_tile_groups.push(AnimatedTileGroup {
            base_tile: base_tile + row as TileIdx * 16,
            frames: vec![[TilePixels::default(); 16]; 2],
            frame_hold: 9,
            phase_offset: 0,
        });
    }

    base_tile
}

fn set_vanilla_animated_tile(
    palette: &mut Palette,
    tiles8: &[TilePixels],
    tile_types: &[u8],
    base_sheet: u16,
    base_tile: TileIdx,
    slot: u16,
    priority: bool,
) {
    let slot = slot as usize;
    palette.tiles[base_tile as usize + slot] = Tile {
        priority,
        collision: tile_types[ANIMATED_TILE_START as usize + slot],
        pixels: tiles8[base_sheet as usize * 64 + slot],
        ..Tile::default()
    };

    let group_base = base_tile + (slot / 16 * 16) as TileIdx;
    let group = palette
        .animated_tile_groups
        .iter_mut()
        .find(|group| group.base_tile == group_base)
        .unwrap();
    let column = slot % 16;
    let frame_start = (base_sheet - 1) as usize * 64 + slot;
    group.frames[0][column] = tiles8[frame_start];
    group.frames[1][column] = tiles8[frame_start + 32];
}

fn strip_tile(mut tile: Tile) -> Tile {
    // These fields vary as matching tiles are reused while importing.
    tile.id = None;
    tile.h_flippable = false;
    tile.v_flippable = false;
    tile
}

// From past experience, it's a very common mistake to mix up SNES addresses
// with "PC" addresses (byte index into the ROM file). So we use type-safe wrappers
// to make these harder to mess up:
#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct PcAddr(u32);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord)]
struct SnesAddr(u32);

macro_rules! impl_add {
    ($target_type:ident, $other_type:ident) => {
        impl Add<$other_type> for $target_type {
            type Output = $target_type;

            fn add(self, other: $other_type) -> Self {
                $target_type((self.0 as $other_type + other) as u32)
            }
        }
    };
}

impl_add!(PcAddr, u32);
impl_add!(SnesAddr, u32);

macro_rules! impl_add_assign {
    ($target_type:ident, $other_type:ident) => {
        impl AddAssign<$other_type> for $target_type {
            fn add_assign(&mut self, other: $other_type) {
                self.0 = (self.0 as $other_type + other) as u32;
            }
        }
    };
}

impl_add_assign!(PcAddr, u32);
impl_add_assign!(SnesAddr, u32);

impl Display for PcAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "0x{:X}", self.0)?;
        Ok(())
    }
}

impl Display for SnesAddr {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${:X}", self.0)?;
        Ok(())
    }
}

impl SnesAddr {
    fn from_bytes(bank: u8, high: u8, low: u8) -> Self {
        Self((bank as u32) << 16 | (high as u32) << 8 | low as u32)
    }

    fn from_bank_offset(bank: u8, offset: u16) -> Self {
        Self((bank as u32) << 16 | offset as u32)
    }
}

impl From<SnesAddr> for PcAddr {
    fn from(addr: SnesAddr) -> Self {
        // It would be a mistake if we ever reference the lower half of a bank
        // (which would often map to RAM, registers, etc. rather than to ROM).
        assert!(addr.0 & 0x8000 == 0x8000);

        PcAddr(addr.0 >> 1 & 0x3F8000 | addr.0 & 0x7FFF)
    }
}

// Addresses of where certain data is located in the ROM. Many of these vary
// between JP and US versions.
struct Constants {
    hud_palettes_addr: SnesAddr,
    main_palettes_addr: SnesAddr,
    aux_palettes_addr: SnesAddr,
    animated_palettes_addr: SnesAddr,
    gfx_bank_addr: SnesAddr,
    gfx_high_addr: SnesAddr,
    gfx_low_addr: SnesAddr,
    tiles16_addr: SnesAddr,
    tiles16_cnt: u32,
    tiles32_tl_addr: SnesAddr,
    tiles32_tr_addr: SnesAddr,
    tiles32_bl_addr: SnesAddr,
    tiles32_br_addr: SnesAddr,
    tiles32_cnt: u32,
    map_high_addr: SnesAddr,
    map_low_addr: SnesAddr,
    custom_map_main_pal_set_addr: Option<SnesAddr>,
    map_aux_pal_set_addr: SnesAddr,
    special_map_pal_set_addr: SnesAddr,
    pal_set_addr: SnesAddr,
    global_gfx_set_addr: SnesAddr,
    local_gfx_set_addr: SnesAddr,
    map_gfx_set_addr: SnesAddr,
    custom_gfx_set_addr: Option<SnesAddr>,
    special_gfx_set_addr: SnesAddr,
    tile_types: SnesAddr,
    custom_bg_colors_addr: Option<SnesAddr>,
    us_map16: bool,
    grave_tilemap_addr: SnesAddr,
    grave_reveal_addr: SnesAddr,
}

impl Constants {
    fn jp() -> Self {
        Constants {
            hud_palettes_addr: SnesAddr(0x1BD660),
            main_palettes_addr: SnesAddr(0x1BE6C8),
            aux_palettes_addr: SnesAddr(0x1BE86C),
            animated_palettes_addr: SnesAddr(0x1BE604),
            gfx_bank_addr: SnesAddr(0x00E7D0),
            gfx_high_addr: SnesAddr(0x00E7D5),
            gfx_low_addr: SnesAddr(0x00E7DA),
            tiles16_addr: SnesAddr(0x0F8000),
            tiles16_cnt: 3742,
            tiles32_tl_addr: SnesAddr(0x038000),
            tiles32_tr_addr: SnesAddr(0x03B3C0),
            tiles32_bl_addr: SnesAddr(0x048000),
            tiles32_br_addr: SnesAddr(0x04B3C0),
            tiles32_cnt: 8828,
            map_high_addr: SnesAddr(0x02F6B1),
            map_low_addr: SnesAddr(0x02F891),
            custom_map_main_pal_set_addr: None,
            map_aux_pal_set_addr: SnesAddr(0x00FD1C),
            special_map_pal_set_addr: SnesAddr(0x02E595),
            pal_set_addr: SnesAddr(0x0CFE74),
            global_gfx_set_addr: SnesAddr(0x00E0B3),
            local_gfx_set_addr: SnesAddr(0x00DDD7),
            map_gfx_set_addr: SnesAddr(0x00FC9C),
            custom_gfx_set_addr: None,
            special_gfx_set_addr: SnesAddr(0x02E585), // appears incorrect in ZS?
            tile_types: SnesAddr(0x0FFD94),
            custom_bg_colors_addr: None,
            us_map16: false,
            grave_tilemap_addr: SnesAddr(0x099990),
            grave_reveal_addr: SnesAddr(0x0999AE),
        }
    }

    fn us() -> Self {
        Constants {
            hud_palettes_addr: SnesAddr(0x1BD660),
            main_palettes_addr: SnesAddr(0x1BE6C8),
            aux_palettes_addr: SnesAddr(0x1BE86C),
            animated_palettes_addr: SnesAddr(0x1BE604),
            gfx_bank_addr: SnesAddr(0x00E790),
            gfx_high_addr: SnesAddr(0x00E795),
            gfx_low_addr: SnesAddr(0x00E79A),
            tiles16_addr: SnesAddr(0x0F8000),
            tiles16_cnt: 3742,
            tiles32_tl_addr: SnesAddr(0x038000),
            tiles32_tr_addr: SnesAddr(0x03B400),
            tiles32_bl_addr: SnesAddr(0x048000),
            tiles32_br_addr: SnesAddr(0x04B400),
            tiles32_cnt: 8864,
            map_high_addr: SnesAddr(0x02F94D),
            map_low_addr: SnesAddr(0x02FB2D),
            custom_map_main_pal_set_addr: None,
            map_aux_pal_set_addr: SnesAddr(0x00FD1C),
            special_map_pal_set_addr: SnesAddr(0x02E831),
            pal_set_addr: SnesAddr(0x0ED504),
            global_gfx_set_addr: SnesAddr(0x00E073),
            local_gfx_set_addr: SnesAddr(0x0DD97),
            map_gfx_set_addr: SnesAddr(0x00FC9C),
            custom_gfx_set_addr: None,
            special_gfx_set_addr: SnesAddr(0x02E821),
            tile_types: SnesAddr(0x0E9459),
            custom_bg_colors_addr: None,
            us_map16: true,
            grave_tilemap_addr: SnesAddr(0x0999A4),
            grave_reveal_addr: SnesAddr(0x0999C2),
        }
    }

    fn auto(rom: &Rom) -> Result<Self> {
        if rom.read_u24(SnesAddr(0x008865).into())? == 0xBD8000 {
            info!("ZScream ROM format detected.");
            let mut constants = Constants::us();
            constants.tiles16_addr = SnesAddr(0xBD8000);
            constants.tiles16_cnt = 4096;
            constants.tiles32_tr_addr = SnesAddr(0x048000);
            constants.tiles32_bl_addr = SnesAddr(0x3E8000);
            constants.tiles32_br_addr = SnesAddr(0x3F8000);
            constants.tiles32_cnt = 17728;

            if rom.read_u8(SnesAddr(0x288148).into())? != 0 {
                info!("Using custom GFX table.");
                constants.custom_gfx_set_addr = Some(SnesAddr(0x288480));
            }
            if rom.read_u8(SnesAddr(0x288141).into())? != 0 {
                info!("Using custom main palette table.");
                constants.custom_map_main_pal_set_addr = Some(SnesAddr(0x288160));
            }
            if rom.read_u8(SnesAddr(0x288140).into())? != 0 {
                info!("Using custom BG colors.");
                constants.custom_bg_colors_addr = Some(SnesAddr(0x288000));
            }
            Ok(constants)
        } else if rom.read_u16(SnesAddr(0x00E7D2).into())? == 0xCA85 {
            info!("JP ROM format detected.");
            Ok(Constants::jp())
        } else if rom.read_u16(SnesAddr(0x00E792).into())? == 0xCA85 {
            info!("US ROM format detected.");
            Ok(Constants::us())
        } else {
            bail!("Unknown ROM format.");
        }
        // TODO: check for expanded 32x32 tiles from ZScream
    }

    fn convert_map16(&self, idx: u16) -> u16 {
        if !self.us_map16 {
            return idx;
        }
        // The US version inserted six Map16 entries at different points in the table.
        idx + match idx {
            ..=0x023E => 0,
            ..=0x0391 => 1,
            ..=0x0395 => 2,
            ..=0x0881 => 3,
            ..=0x088D => 4,
            0x088E => 5,
            _ => 6,
        }
    }
}

#[derive(Clone)]
struct Rom {
    pub data: Vec<u8>,
}

#[derive(Copy, Clone, Debug)]
struct Tile8 {
    gfx_char: u16, // Index into area-loaded graphics tiles (0-1023)
    pal_idx: u8,   // Index into area-loaded palettes (0-7)
    priority: bool,
    flip: Flip,
}

impl Tile8 {
    pub fn from_vram_tilemap_word(w: u16) -> Self {
        Self {
            gfx_char: w & 0x3FF,
            pal_idx: ((w >> 10) & 7) as u8,
            priority: (w >> 13) & 1 == 1,
            flip: match w >> 14 {
                0 => Flip::None,
                1 => Flip::Horizontal,
                2 => Flip::Vertical,
                3 => Flip::Both,
                _ => panic!("error decoding flip"),
            },
        }
    }
}

type Tile16 = [Tile8; 4];

// Index into Importer::tiles16
type Tile16Idx = u16;

type Tile32 = [Tile16Idx; 4];

// Index into Importer::tiles32
type Tile32Idx = u16;

type MapIdx = u16;

#[derive(Debug)]
struct MapPalettes {
    main: u8,
    aux1: u8,
    aux2: u8,
    animated: u8,
}

pub struct Importer<'a> {
    state: &'a mut EditorState,
    constants: Constants,
    rom: Rom,
    theme: String,
    area_name_by_map_id: HashMap<u8, AreaName>,
    hud_palette_ids: Vec<[PaletteId; 2]>,
    main_palette_ids: Vec<[PaletteId; 5]>,
    aux_palette_ids: Vec<[PaletteId; 3]>,
    animated_palette_ids: Vec<PaletteId>,
    tiles8: Vec<[[u8; 8]; 8]>, // 3bpp tile color indices (0-7)
    tiles16: Vec<Tile16>,
    tiles32: Vec<Tile32>,
    map_tiles: Vec<[[Tile32Idx; 16]; 16]>,
    map_parents: Vec<MapIdx>,
    map_palettes: Vec<MapPalettes>,
    map_gfx: Vec<[u8; 8]>,
    tile_types: Vec<u8>,
    pal_bg_color: HashMap<PaletteId, ColorRGB>,
}

impl Rom {
    pub fn new(data: Vec<u8>) -> Self {
        Rom { data }
    }

    pub fn read_u8(&self, addr: PcAddr) -> Result<u8> {
        ensure!(
            (addr.0 as usize) < self.data.len(),
            "read_u8 address out of bounds"
        );
        Ok(self.data[addr.0 as usize])
    }

    pub fn read_u16(&self, addr: PcAddr) -> Result<u16> {
        ensure!(
            addr.0 as usize + 1 < self.data.len(),
            "read_u16 address out of bounds"
        );
        let b0 = self.data[addr.0 as usize] as u16;
        let b1 = self.data[addr.0 as usize + 1] as u16;
        Ok(b0 | b1 << 8)
    }

    pub fn read_u24(&self, addr: PcAddr) -> Result<u32> {
        ensure!(
            addr.0 as usize + 2 < self.data.len(),
            "read_u24 address out of bounds"
        );
        let b0 = self.data[addr.0 as usize] as u32;
        let b1 = self.data[addr.0 as usize + 1] as u32;
        let b2 = self.data[addr.0 as usize + 2] as u32;
        Ok(b0 | b1 << 8 | b2 << 16)
    }

    pub fn read_n(&self, addr: PcAddr, n: usize) -> Result<&[u8]> {
        ensure!(
            addr.0 as usize + n <= self.data.len(),
            "read_n address out of bounds"
        );
        Ok(&self.data[addr.0 as usize..(addr.0 as usize + n)])
    }
}

impl<'a> Importer<'a> {
    pub fn import(state: &'a mut EditorState, path: &Path) -> Result<()> {
        info!("Importing from ROM at {}", path.display());
        let mut importer = Self::new(state, path)?;
        importer.import_all()?;
        Ok(())
    }

    fn new(state: &'a mut EditorState, path: &Path) -> Result<Self> {
        let rom_bytes = std::fs::read(path)?;
        let rom = Rom::new(rom_bytes);
        let theme = state.main_area().theme.clone();
        Ok(Self {
            state,
            constants: Constants::auto(&rom)?,
            rom,
            theme,
            area_name_by_map_id: HashMap::new(),
            hud_palette_ids: vec![],
            main_palette_ids: vec![],
            aux_palette_ids: vec![],
            animated_palette_ids: vec![],
            tiles8: vec![],
            tiles16: vec![],
            tiles32: vec![],
            map_tiles: vec![],
            map_parents: vec![],
            map_palettes: vec![],
            map_gfx: vec![],
            tile_types: vec![],
            pal_bg_color: HashMap::new(),
        })
    }

    fn import_all(&mut self) -> Result<()> {
        let starting_palette_id = self.state.palettes.iter().map(|x| x.id).max().unwrap() + 1;
        self.load_area_names()?;
        self.load_tile_types()?;
        self.import_all_palettes(starting_palette_id)?;
        self.load_graphics()?;
        self.load_16x16_tiles()?;
        self.load_32x32_tiles()?;
        self.load_map_tiles()?;
        self.load_map_parents()?;
        self.load_map_palettes()?;
        self.load_map_gfx()?;
        self.load_areas()?;
        self.load_dynamic_tiles()?;
        self.ensure_palette_full_rows()?;
        self.assign_bg_colors()?;
        save_project(self.state)?;
        load_project(self.state)?;
        for area_name in &self.state.area_names.clone() {
            let area_id = AreaId {
                theme: self.theme.clone(),
                area: area_name.clone(),
            };
            self.state.load_area(&area_id)?;
            save_area_png(self.state, &area_id)?;
            self.state.areas.remove(&area_id);
        }
        load_project(self.state)?; // Load again to open the first room.
        Ok(())
    }

    fn load_area_names(&mut self) -> Result<()> {
        for area_name in &self.state.area_names {
            let area_id = AreaId {
                area: area_name.clone(),
                theme: self.theme.clone(),
            };
            let area = load_area(self.state, &area_id)?;
            if let Some(id) = area.vanilla_map_id {
                self.area_name_by_map_id.insert(id, area_name.clone());
            }
        }
        Ok(())
    }

    fn load_palette(&mut self, addr: PcAddr, size: usize) -> Result<[ColorRGB; 16]> {
        let mut colors = [[0, 0, 0]; 16];
        for i in 0..size {
            let c = self.rom.read_u16(addr + i as u32 * 2)?;
            let r = c & 31;
            let g = (c >> 5) & 31;
            let b = (c >> 10) & 31;
            colors[i + 1] = [r as ColorValue, g as ColorValue, b as ColorValue];
        }
        Ok(colors)
    }

    fn import_all_palettes(&mut self, mut next_id: PaletteId) -> Result<()> {
        let palette_groups = [
            ("HUD", self.constants.hud_palettes_addr, 1, 2, 15),
            ("Main", self.constants.main_palettes_addr, 6, 5, 7),
            ("Aux", self.constants.aux_palettes_addr, 20, 3, 7),
            ("Animated", self.constants.animated_palettes_addr, 14, 1, 7),
        ];

        let mut pal_by_colors: HashMap<[ColorRGB; 16], PaletteId> = HashMap::new();
        for pal in &self.state.palettes {
            let mut colors = pal.colors;
            colors[0] = [0, 0, 0];
            let _ = pal_by_colors.try_insert(colors, pal.id);
        }

        for (group_name, base_addr, cnt_pal, cnt_rows, size) in palette_groups {
            let base_addr: PcAddr = base_addr.into();
            for i in 0..cnt_pal {
                let mut palette_ids: Vec<PaletteId> = vec![];
                for j in 0..cnt_rows {
                    let addr = base_addr + ((i * cnt_rows + j) * size) * 2;
                    let colors = self.load_palette(addr, size as usize)?;

                    if let Some(&id) = pal_by_colors.get(&colors) {
                        palette_ids.push(id);
                    } else {
                        let mut name = format!("{} {:x}-{}", group_name, i, j);
                        if self.theme != "Base" {
                            name += &format!(" ({})", self.theme);
                        }
                        self.state.palettes.push(Palette {
                            modified: true,
                            name,
                            id: next_id,
                            colors,
                            tiles: vec![],
                            animated_tile_groups: vec![],
                        });
                        pal_by_colors.insert(colors, next_id);
                        palette_ids.push(next_id);
                        next_id += 1;
                    }
                }
                match group_name {
                    "HUD" => {
                        self.hud_palette_ids.push(palette_ids.try_into().unwrap());
                    }
                    "Main" => {
                        self.main_palette_ids.push(palette_ids.try_into().unwrap());
                    }
                    "Aux" => {
                        self.aux_palette_ids.push(palette_ids.try_into().unwrap());
                    }
                    "Animated" => {
                        self.animated_palette_ids.push(palette_ids[0]);
                    }
                    _ => panic!("unexpected group_name"),
                }
            }
        }
        update_palette_order(self.state);
        Ok(())
    }

    fn load_graphics(&mut self) -> Result<()> {
        let rom = &self.rom;
        let gfx_bank = rom.read_u16(self.constants.gfx_bank_addr.into())?;
        let gfx_high = rom.read_u16(self.constants.gfx_high_addr.into())?;
        let gfx_low = rom.read_u16(self.constants.gfx_low_addr.into())?;

        for i in 0..113 {
            let bank = rom.read_u8(SnesAddr::from_bank_offset(0x00, gfx_bank + i).into())?;
            let high = rom.read_u8(SnesAddr::from_bank_offset(0x00, gfx_high + i).into())?;
            let low = rom.read_u8(SnesAddr::from_bank_offset(0x00, gfx_low + i).into())?;
            let addr = SnesAddr::from_bytes(bank, high, low);
            let data = decompress(rom, addr.into(), false)?;
            if data.len() != 0x600 {
                bail!("Unexpected graphics sheet length: {}", data.len());
            }

            for j in 0..64 {
                let mut tile: [[u8; 8]; 8] = [[0; 8]; 8];
                for (y, row) in tile.iter_mut().enumerate() {
                    for (x, pixel) in row.iter_mut().enumerate() {
                        let c0 = (data[j * 24 + y * 2] >> (7 - x)) & 1;
                        let c1 = (data[j * 24 + y * 2 + 1] >> (7 - x)) & 1;
                        let c2 = (data[j * 24 + y + 16] >> (7 - x)) & 1;
                        let c = c0 | (c1 << 1) | (c2 << 2);
                        *pixel = c;
                    }
                }
                self.tiles8.push(tile);
            }
        }
        Ok(())
    }

    fn load_16x16_tiles(&mut self) -> Result<()> {
        for i in 0..self.constants.tiles16_cnt {
            let addr = self.constants.tiles16_addr + i * 8;
            let tl = self.rom.read_u16(addr.into())?;
            let tr = self.rom.read_u16((addr + 2).into())?;
            let bl = self.rom.read_u16((addr + 4).into())?;
            let br = self.rom.read_u16((addr + 6).into())?;
            self.tiles16.push([
                Tile8::from_vram_tilemap_word(tl),
                Tile8::from_vram_tilemap_word(tr),
                Tile8::from_vram_tilemap_word(bl),
                Tile8::from_vram_tilemap_word(br),
            ]);
        }
        Ok(())
    }

    fn load_32x32_tiles(&mut self) -> Result<()> {
        let rom = &self.rom;
        let quadrant_base_addrs: [PcAddr; 4] = [
            self.constants.tiles32_tl_addr.into(),
            self.constants.tiles32_tr_addr.into(),
            self.constants.tiles32_bl_addr.into(),
            self.constants.tiles32_br_addr.into(),
        ];
        let mut offset = 0;
        let tiles32_size = self.constants.tiles32_cnt * 6 / 4;
        while offset < tiles32_size {
            for i in 0..4 {
                let mut quadrant_idxs = vec![];
                for base_addr in quadrant_base_addrs {
                    let addr = base_addr + offset;
                    let tile16_idx = match i {
                        0 => {
                            rom.read_u8(addr)? as u16 | (rom.read_u8(addr + 4)? as u16 & 0xF0) << 4
                        }
                        1 => {
                            rom.read_u8(addr + 1)? as u16
                                | (rom.read_u8(addr + 4)? as u16 & 0x0F) << 8
                        }
                        2 => {
                            rom.read_u8(addr + 2)? as u16
                                | (rom.read_u8(addr + 5)? as u16 & 0xF0) << 4
                        }
                        3 => {
                            rom.read_u8(addr + 3)? as u16
                                | (rom.read_u8(addr + 5)? as u16 & 0x0F) << 8
                        }
                        _ => panic!("error loading 32x32 map tiles"),
                    };
                    ensure!((tile16_idx as usize) < self.tiles16.len());
                    quadrant_idxs.push(tile16_idx);
                }
                self.tiles32.push([
                    quadrant_idxs[0],
                    quadrant_idxs[1],
                    quadrant_idxs[2],
                    quadrant_idxs[3],
                ]);
            }
            offset += 6;
        }
        Ok(())
    }

    fn load_map_tiles(&mut self) -> Result<()> {
        let rom = &self.rom;
        for i in 0..MAP_COUNT {
            let high_addr = SnesAddr(rom.read_u24((self.constants.map_high_addr + i * 3).into())?);
            let high_data = decompress(rom, high_addr.into(), true)?;

            let low_addr = SnesAddr(rom.read_u24((self.constants.map_low_addr + i * 3).into())?);
            let low_data = decompress(rom, low_addr.into(), true)?;

            ensure!(high_data.len() == 256);
            ensure!(low_data.len() == 256);

            let mut block: [[Tile32Idx; 16]; 16] = [[0; 16]; 16];
            for (y, row) in block.iter_mut().enumerate() {
                for (x, tile) in row.iter_mut().enumerate() {
                    let j = y * 16 + x;
                    let mut tile32_idx = (high_data[j] as u16) << 8 | low_data[j] as u16;
                    if (tile32_idx as u32) >= self.constants.tiles32_cnt {
                        // This happens in the US ROM (TODO: look into why).
                        info!(
                            "World block ${:X} (x={}, y={}): tile32 index {} out of bounds ({})",
                            i, x, y, tile32_idx, self.constants.tiles32_cnt
                        );
                        tile32_idx = 0;
                    }

                    *tile = tile32_idx;
                }
            }
            self.map_tiles.push(block);
        }
        Ok(())
    }

    fn load_map_parents(&mut self) -> Result<()> {
        let mut parents: Vec<MapIdx> = (0..MAP_COUNT as MapIdx).collect();

        // Large areas:
        for i in [0, 3, 5, 24, 27, 30, 48, 53] {
            for j in [0, 64] {
                parents[i + j + 1] = (i + j) as MapIdx;
                parents[i + j + 8] = (i + j) as MapIdx;
                parents[i + j + 9] = (i + j) as MapIdx;
            }
        }

        parents[130] = 129;
        parents[137] = 129;
        parents[138] = 129;
        parents[148] = 128;
        parents[149] = 3;
        parents[150] = 91;
        parents[151] = 0;
        parents[156] = 67;
        parents[157] = 0;
        parents[158] = 0;
        parents[159] = 44;

        self.map_parents = parents;
        Ok(())
    }

    fn load_map_palettes(&mut self) -> Result<()> {
        let rom = &self.rom;
        for i in 0..AREA_MAP_COUNT as usize {
            let parent = self.map_parents[i];
            let main = if let Some(main_pal_addr) = self.constants.custom_map_main_pal_set_addr {
                rom.read_u8((main_pal_addr + parent as u32).into())?
            } else {
                match i {
                    3 | 5 | 7 => 2,          // Light World: death mountain
                    0..0x40 => 0,            // Rest of Light World
                    0x43 | 0x45 | 0x47 => 3, // Dark World: death mountain
                    0x40..0x80 => 1,         // Rest of Dark World
                    0x88 => 4,               // Triforce room
                    0x80..0xA0 => 0,         // Special World
                    _ => panic!("internal error"),
                }
            };
            let pal_set = if i == 0x88 {
                0
            } else if parent >= 0x80 {
                rom.read_u8(
                    (self.constants.special_map_pal_set_addr + (parent as u32 - 0x80)).into(),
                )?
            } else {
                rom.read_u8((self.constants.map_aux_pal_set_addr + parent as u32).into())?
            };
            let prev_pal_set = if parent >= 1 {
                rom.read_u8((self.constants.map_aux_pal_set_addr + (parent - 1) as u32).into())?
            } else {
                0
            };
            let pal_set_addr = self.constants.pal_set_addr + pal_set as u32 * 4;
            let mut aux1 = rom.read_u8(pal_set_addr.into())?;
            let mut aux2 = rom.read_u8((pal_set_addr + 1).into())?;
            let mut animated = rom.read_u8((pal_set_addr + 2).into())?;
            if aux1 >= 20 {
                warn!("{:02X}: out-of-range aux1: {}", i, aux1);
                aux1 = 0;
            }
            if aux2 >= 20 {
                info!(
                    "{:x}: {} {} {}",
                    parent,
                    self.constants.pal_set_addr,
                    prev_pal_set,
                    self.constants.pal_set_addr + prev_pal_set as u32 * 4 + 1
                );
                aux2 = rom
                    .read_u8((self.constants.pal_set_addr + prev_pal_set as u32 * 4 + 1).into())?;
                if aux2 >= 20 {
                    warn!("{:02X}: out-of-range aux2: {}", i, aux2);
                    aux2 = 0;
                }
            }
            if animated >= 14 {
                warn!("{:02X}: out-of-range animated: {}", i, animated);
                animated = 0;
            }
            self.map_palettes.push(MapPalettes {
                main,
                aux1,
                aux2,
                animated,
            });
        }

        Ok(())
    }

    fn load_map_gfx(&mut self) -> Result<()> {
        let rom = &self.rom;
        let global_gfx_set_addr = self.constants.global_gfx_set_addr;
        let local_gfx_set_addr = self.constants.local_gfx_set_addr;
        let map_gfx_set_addr = self.constants.map_gfx_set_addr;
        let special_gfx_set_addr = self.constants.special_gfx_set_addr;
        for i in 0..AREA_MAP_COUNT as usize {
            let parent = self.map_parents[i];
            let global_idx = match parent {
                0x40..0x80 => 0x21, // Dark World
                0x88 => 0x24,       // Triforce room
                _ => 0x20,          // Light World
            };
            let mut gfx: Vec<u8> = rom
                .read_n((global_gfx_set_addr + global_idx * 8).into(), 8)?
                .to_owned();
            if let Some(custom_gfx_set_addr) = self.constants.custom_gfx_set_addr {
                let local_gfx = rom
                    .read_n((custom_gfx_set_addr + parent as u32 * 8).into(), 8)?
                    .to_owned();
                for i in 0..8 {
                    if local_gfx[i] != 0xff {
                        gfx[i] = local_gfx[i];
                    }
                }
            } else {
                let local_idx = match parent {
                    0x88 => 81,
                    0x80.. => {
                        rom.read_u8((special_gfx_set_addr + (parent - 0x80) as u32).into())?
                    }
                    _ => rom.read_u8((map_gfx_set_addr + parent as u32).into())?,
                };
                let local_gfx: &[u8] =
                    rom.read_n((local_gfx_set_addr + local_idx as u32 * 4).into(), 4)?;
                for j in 0..4 {
                    if local_gfx[j] != 0 {
                        gfx[3 + j] = local_gfx[j];
                    }
                }
            }
            if i == 0x34 {
                info!("gfx: {:x?}", gfx);
            }
            self.map_gfx.push(gfx.try_into().unwrap());
        }

        Ok(())
    }

    fn load_tile_types(&mut self) -> Result<()> {
        self.tile_types = self
            .rom
            .read_n(self.constants.tile_types.into(), 512)?
            .to_owned();
        Ok(())
    }

    fn load_areas(&mut self) -> Result<()> {
        let tile32_offsets = [(0, 0), (2, 0), (0, 2), (2, 2)];
        let tile16_offsets = [(0, 0), (1, 0), (0, 1), (1, 1)];

        let mut tile_lookup: Vec<HashMap<Tile, (TileIdx, Flip)>> =
            vec![HashMap::new(); self.state.palettes.len()];
        let mut animated_groups: HashMap<(usize, u16), TileIdx> = HashMap::new();
        let mut animated_priorities: HashMap<(usize, u16, u16), bool> = HashMap::new();

        for (palette, lookup) in self.state.palettes.iter().zip(&mut tile_lookup) {
            for (idx, tile) in palette.tiles.iter().enumerate() {
                for flip in [Flip::None, Flip::Horizontal, Flip::Vertical, Flip::Both] {
                    lookup.insert(
                        strip_tile(flip.apply_to_tile(*tile)),
                        (idx as TileIdx, flip),
                    );
                }
            }
        }

        for parent in 0..=0x81 {
            if self.map_parents[parent] as usize != parent {
                continue;
            }
            let world_idx = parent / 64;
            let _block_y = (parent / 8) % 8;
            let block_x = parent % 8;
            let size = if block_x <= 6 && self.map_parents[parent + 1] as usize == parent {
                (2, 2)
            } else {
                (1, 1)
            };
            let mut gfx_idxs: Vec<u16> = vec![];
            for idx in self.map_gfx[parent] {
                gfx_idxs.extend((idx as u16 * 64)..((idx + 1) as u16 * 64));
            }
            let animated_gfx = if [0x03, 0x05, 0x07, 0x43, 0x45, 0x47].contains(&parent) {
                0x59
            } else {
                0x5B
            };
            gfx_idxs[0x1C0..0x1E0]
                .copy_from_slice(&((animated_gfx * 64)..(animated_gfx * 64 + 32)).collect_vec());

            let pal = &self.map_palettes[parent];
            let bg_color = if let Some(custom_bg_colors_addr) = self.constants.custom_bg_colors_addr
            {
                let c = self
                    .rom
                    .read_u16((custom_bg_colors_addr + parent as u32 * 2).into())?;
                [
                    (c & 31) as u8,
                    ((c >> 5) & 31) as u8,
                    ((c >> 10) & 31) as u8,
                ]
            } else {
                match parent {
                    0x03 | 0x05 | 0x07 | 0x43 | 0x45 | 0x47 => [0, 0, 0], // Death mountain: black background
                    0x80 | 0x81 => [6, 14, 6], // Special World: dark green background
                    0x40..0x80 => [18, 17, 10], // Dark world: tan background
                    _ => [9, 19, 9],           // Default: green background
                }
            };

            let area_name = match self.area_name_by_map_id.get(&(parent as u8)) {
                Some(name) => name.clone(),
                None => match world_idx {
                    0 => format!("{:02X} Light World", parent),
                    1 => format!("{:02X} Dark World", parent),
                    2 => format!("{:02X} Special World", parent),
                    _ => bail!("unexpected world_idx: {}", world_idx),
                },
            };
            let overlays: &[(usize, &str)] = match parent {
                0x00 => &[(0x9D, "Woods Fog"), (0x9E, "Woods Clear")],
                0x03 | 0x05 | 0x07 => &[(0x95, "Mountain Overlay")],
                0x40 => &[(0x9D, "Woods Fog")],
                0x43 | 0x45 | 0x47 => &[(0x9C, "Dark Mountain Overlay")],
                0x5B => &[(0x96, "Pyramid Background")],
                0x80 => &[(0x97, "Grove Fog"), (0x94, "Bridge Shadow")],
                _ => &[],
            };
            let (
                bg_layering,
                bg_camera_follow_x,
                bg_camera_drift_x,
                bg_camera_follow_y,
                bg_camera_drift_y,
            ) = match parent {
                0x00 | 0x40 | 0x80 => (BackgroundLayering::HalfAdd, 0.0, 0.125, 0.0, 0.125),
                0x03 | 0x05 | 0x07 => (BackgroundLayering::Backdrop, 0.25, 0.0, 0.5, 0.0),
                0x43 | 0x45 => (BackgroundLayering::Backdrop, 1.0, 0.0, 1.5, -0.125),
                0x47 | 0x5B => (BackgroundLayering::Backdrop, 0.5, 0.0, 0.5, 0.0),
                _ => (BackgroundLayering::None, 1.0, 0.0, 1.0, 0.0),
            };
            let bg_color = match parent {
                0x03 | 0x05 | 0x07 => [6, 12, 12],
                0x43 | 0x45 | 0x47 => [6, 10, 7],
                0x5B => [19, 15, 9],
                _ => bg_color,
            };
            let mut area: Area = Area {
                modified: false,
                name: area_name,
                theme: self.theme.clone(),
                vanilla_map_id: Some(parent as u8),
                bg_color,
                bg_layering,
                bg_camera_follow_x,
                bg_camera_drift_x,
                bg_camera_follow_y,
                bg_camera_drift_y,
                size: (size.0 * 2, size.1 * 2),
                layers: vec![Layer {
                    modified: true,
                    name: "Main".to_string(),
                    background: Background::Bg2,
                    tiles: vec![
                        vec![Some(TilePlacement::default()); size.0 as usize * 64];
                        size.1 as usize * 64
                    ],
                }],
            };
            self.state.area_names.push(area.name.clone());
            for my in 0..size.1 as usize {
                for mx in 0..size.0 as usize {
                    let map_idx = parent + my * 8 + mx;
                    let tiles = &self.map_tiles[map_idx];
                    for (ty, row) in tiles.iter().enumerate() {
                        for (tx, &t32_idx) in row.iter().enumerate() {
                            let t32 = self.tiles32[t32_idx as usize];
                            for i in 0..4 {
                                let t16_idx = t32[i];
                                let t16 = self.tiles16[t16_idx as usize];
                                for j in 0..4 {
                                    let t8 = t16[j];
                                    let x = mx * 64
                                        + tx * 4
                                        + tile32_offsets[i].0
                                        + tile16_offsets[j].0;
                                    let y = my * 64
                                        + ty * 4
                                        + tile32_offsets[i].1
                                        + tile16_offsets[j].1;
                                    let tiles8_idx = gfx_idxs[t8.gfx_char as usize];
                                    let gfx_sheet = t8.gfx_char / 64;
                                    ensure!(gfx_sheet < 8);
                                    let pal_high = [0, 3, 4, 5].contains(&gfx_sheet);
                                    let pal_id = match (t8.pal_idx, pal_high) {
                                        (p @ (0 | 1), _) => self.hud_palette_ids[0][p as usize],
                                        (p @ 2..=6, false) => {
                                            self.main_palette_ids[pal.main as usize][p as usize - 2]
                                        }
                                        (7, false) => {
                                            self.animated_palette_ids[pal.animated as usize]
                                        }
                                        (p @ 2..=4, true) => {
                                            self.aux_palette_ids[pal.aux1 as usize][p as usize - 2]
                                        }
                                        (p @ 5..=7, true) => {
                                            self.aux_palette_ids[pal.aux2 as usize][p as usize - 5]
                                        }
                                        _ => {
                                            bail!("unexpected palette: {} {}", t8.pal_idx, pal_high)
                                        }
                                    };
                                    let palette_idx = self.state.palettes_id_idx_map[&pal_id];
                                    let collision = self.tile_types[t8.gfx_char as usize];
                                    let (tile_idx, flip) = if let Some(slot) =
                                        animated_tile_slot(t8.gfx_char)
                                    {
                                        let key = (palette_idx, animated_gfx);
                                        let base_tile = match animated_groups.get(&key) {
                                            Some(&base_tile) => base_tile,
                                            None => {
                                                let base_tile = add_vanilla_animated_tile_groups(
                                                    &mut self.state.palettes[palette_idx],
                                                );
                                                animated_groups.insert(key, base_tile);
                                                base_tile
                                            }
                                        };
                                        let tile_idx = base_tile + slot;
                                        match animated_priorities.entry((
                                            palette_idx,
                                            animated_gfx,
                                            slot,
                                        )) {
                                            Entry::Vacant(entry) => {
                                                entry.insert(t8.priority);
                                                set_vanilla_animated_tile(
                                                    &mut self.state.palettes[palette_idx],
                                                    &self.tiles8,
                                                    &self.tile_types,
                                                    animated_gfx,
                                                    base_tile,
                                                    slot,
                                                    t8.priority,
                                                );
                                            }
                                            Entry::Occupied(entry)
                                                if *entry.get() != t8.priority =>
                                            {
                                                warn!(
                                                    "Conflicting priority for animated tile ${:03X} in palette {}; keeping the first value",
                                                    t8.gfx_char, pal_id
                                                );
                                            }
                                            Entry::Occupied(_) => {}
                                        }
                                        (tile_idx, t8.flip)
                                    } else {
                                        let collision = match t8.flip {
                                            Flip::Horizontal | Flip::Both
                                                if (0x10..0x1c).contains(&collision) =>
                                            {
                                                collision | 1
                                            }
                                            _ => collision,
                                        };
                                        let tile = Tile {
                                            id: None,
                                            priority: t8.priority,
                                            h_flippable: false,
                                            v_flippable: false,
                                            collision,
                                            pixels: t8.flip.apply_to_pixels(
                                                self.tiles8[tiles8_idx as usize],
                                            ),
                                        };
                                        match tile_lookup[palette_idx].get(&tile) {
                                            Some(x) => *x,
                                            None => {
                                                let idx =
                                                    self.state.palettes[palette_idx].tiles.len()
                                                        as TileIdx;
                                                self.state.palettes[palette_idx].tiles.push(tile);
                                                for flip in [
                                                    Flip::None,
                                                    Flip::Horizontal,
                                                    Flip::Vertical,
                                                    Flip::Both,
                                                ] {
                                                    tile_lookup[palette_idx].insert(
                                                        strip_tile(flip.apply_to_tile(tile)),
                                                        (idx, flip),
                                                    );
                                                }
                                                (idx, Flip::None)
                                            }
                                        }
                                    };

                                    match flip {
                                        Flip::None => {}
                                        Flip::Horizontal => {
                                            self.state.palettes[palette_idx].tiles
                                                [tile_idx as usize]
                                                .h_flippable = true;
                                        }
                                        Flip::Vertical => {
                                            self.state.palettes[palette_idx].tiles
                                                [tile_idx as usize]
                                                .v_flippable = true;
                                        }
                                        Flip::Both => {
                                            self.state.palettes[palette_idx].tiles
                                                [tile_idx as usize]
                                                .h_flippable = true;
                                            self.state.palettes[palette_idx].tiles
                                                [tile_idx as usize]
                                                .v_flippable = true;
                                        }
                                    }

                                    area.set_layer_placement(
                                        0,
                                        x as u16,
                                        y as u16,
                                        Some(TilePlacement {
                                            palette: pal_id,
                                            tile: tile_idx,
                                            flip,
                                        }),
                                    )?;

                                    match self.pal_bg_color.entry(pal_id) {
                                        Entry::Occupied(mut occupied_entry) => {
                                            if occupied_entry.get() != &bg_color {
                                                // Use black as a marker of ambiguous BG color
                                                occupied_entry.insert([0, 0, 0]);
                                            }
                                        }
                                        Entry::Vacant(vacant_entry) => {
                                            vacant_entry.insert(bg_color);
                                        }
                                    }
                                }
                            }
                        }
                    }
                }
            }
            for &(map_id, name) in overlays {
                let map16 = self.build_map16(map_id);
                let overlay = self.build_dynamic_grid(parent, &map16, false)?;
                let width = size.0 as usize * 64;
                let height = size.1 as usize * 64;
                let mut tiles = vec![vec![None; width]; height];
                for y in 0..height {
                    for x in 0..width {
                        let Some(placement) = overlay.tiles[y % 64][x % 64] else {
                            continue;
                        };
                        let palette_idx = self.state.palettes_id_idx_map[&placement.palette];
                        let tile = self.state.palettes[palette_idx].tiles[placement.tile as usize];
                        let mut visible = false;
                        for row in tile.pixels {
                            for color in row {
                                if color != 0 {
                                    visible = true;
                                }
                            }
                        }
                        if visible {
                            tiles[y][x] = Some(placement);
                        }
                    }
                }
                area.layers.insert(
                    area.layers.len() - 1,
                    Layer {
                        modified: true,
                        name: name.to_string(),
                        background: Background::Bg1,
                        tiles,
                    },
                );
            }
            self.state
                .set_area(crate::state::AreaPosition::Main, area)?;
            save_area_json(self.state, &self.state.main_area_id.clone())?;
        }
        Ok(())
    }

    fn build_dynamic_grid(
        &mut self,
        parent: usize,
        map16: &[Vec<u16>],
        jp_indices: bool,
    ) -> Result<DynamicTileGrid> {
        let mut gfx_idxs: Vec<u16> = vec![];
        for idx in self.map_gfx[parent] {
            gfx_idxs.extend((idx as u16 * 64)..((idx + 1) as u16 * 64));
        }
        let animated_gfx = if [0x03, 0x05, 0x07, 0x43, 0x45, 0x47].contains(&parent) {
            0x59
        } else {
            0x5B
        };
        for slot in 0..ANIMATED_TILE_COUNT {
            gfx_idxs[ANIMATED_TILE_START as usize + slot as usize] = animated_gfx * 64 + slot;
        }

        let pal = &self.map_palettes[parent];
        let mut tiles = vec![vec![None; map16[0].len() * 2]; map16.len() * 2];
        for (map_y, row) in map16.iter().enumerate() {
            for (map_x, &tile16_idx) in row.iter().enumerate() {
                let tile16_idx = if jp_indices {
                    self.constants.convert_map16(tile16_idx)
                } else {
                    tile16_idx
                };
                for (tile8_idx, tile8) in self.tiles16[tile16_idx as usize].iter().enumerate() {
                    let gfx_sheet = tile8.gfx_char / 64;
                    ensure!(gfx_sheet < 8);
                    let pal_high = [0, 3, 4, 5].contains(&gfx_sheet);
                    let palette_id = match (tile8.pal_idx, pal_high) {
                        (p @ (0 | 1), _) => self.hud_palette_ids[0][p as usize],
                        (p @ 2..=6, false) => {
                            self.main_palette_ids[pal.main as usize][p as usize - 2]
                        }
                        (7, false) => self.animated_palette_ids[pal.animated as usize],
                        (p @ 2..=4, true) => {
                            self.aux_palette_ids[pal.aux1 as usize][p as usize - 2]
                        }
                        (p @ 5..=7, true) => {
                            self.aux_palette_ids[pal.aux2 as usize][p as usize - 5]
                        }
                        _ => bail!("unexpected palette: {} {}", tile8.pal_idx, pal_high),
                    };
                    let palette_idx = self.state.palettes_id_idx_map[&palette_id];
                    let mut collision = self.tile_types[tile8.gfx_char as usize];
                    if matches!(tile8.flip, Flip::Horizontal | Flip::Both)
                        && (0x10..0x1c).contains(&collision)
                    {
                        collision |= 1;
                    }
                    let tile = Tile {
                        id: None,
                        priority: tile8.priority,
                        collision,
                        pixels: tile8.flip.apply_to_pixels(
                            self.tiles8[gfx_idxs[tile8.gfx_char as usize] as usize],
                        ),
                        ..Tile::default()
                    };

                    let mut placement = None;
                    for (idx, candidate) in
                        self.state.palettes[palette_idx].tiles.iter().enumerate()
                    {
                        for flip in [Flip::None, Flip::Horizontal, Flip::Vertical, Flip::Both] {
                            if strip_tile(flip.apply_to_tile(*candidate)) == tile {
                                placement = Some(TilePlacement {
                                    palette: palette_id,
                                    tile: idx as TileIdx,
                                    flip,
                                });
                                break;
                            }
                        }
                        if placement.is_some() {
                            break;
                        }
                    }
                    let placement = match placement {
                        Some(placement) => placement,
                        None => {
                            let tile_idx = self.state.palettes[palette_idx].tiles.len() as TileIdx;
                            self.state.palettes[palette_idx].tiles.push(tile);
                            TilePlacement {
                                palette: palette_id,
                                tile: tile_idx,
                                flip: Flip::None,
                            }
                        }
                    };
                    match placement.flip {
                        Flip::None => {}
                        Flip::Horizontal => {
                            self.state.palettes[palette_idx].tiles[placement.tile as usize]
                                .h_flippable = true;
                        }
                        Flip::Vertical => {
                            self.state.palettes[palette_idx].tiles[placement.tile as usize]
                                .v_flippable = true;
                        }
                        Flip::Both => {
                            self.state.palettes[palette_idx].tiles[placement.tile as usize]
                                .h_flippable = true;
                            self.state.palettes[palette_idx].tiles[placement.tile as usize]
                                .v_flippable = true;
                        }
                    }
                    let x = map_x * 2 + tile8_idx % 2;
                    let y = map_y * 2 + tile8_idx / 2;
                    tiles[y][x] = Some(placement);
                }
            }
        }
        Ok(DynamicTileGrid { tiles })
    }

    fn add_dynamic_variant(
        &mut self,
        kind: DynamicTileType,
        parent: usize,
        before: Vec<Vec<u16>>,
        after_frames: Vec<Vec<Vec<u16>>>,
    ) -> Result<()> {
        let before = self.build_dynamic_grid(parent, &before, false)?;
        let mut frames = Vec::with_capacity(after_frames.len());
        for frame in after_frames {
            frames.push(self.build_dynamic_grid(parent, &frame, true)?);
        }
        let variant = DynamicTileVariant {
            before,
            after_frames: frames,
        };
        let group = self
            .state
            .dynamic_tiles
            .groups
            .iter_mut()
            .find(|group| group.kind == kind)
            .unwrap();
        if !group.variants.contains(&variant) {
            group.variants.push(variant);
        }
        Ok(())
    }

    fn build_map16(&self, parent: usize) -> Vec<Vec<u16>> {
        let block_x = parent % 8;
        let map_size = if block_x <= 6 && self.map_parents[parent + 1] as usize == parent {
            2
        } else {
            1
        };
        let mut map16 = vec![vec![0; map_size * 32]; map_size * 32];
        for map_y in 0..map_size {
            for map_x in 0..map_size {
                let map_idx = parent + map_y * 8 + map_x;
                for (tile32_y, row) in self.map_tiles[map_idx].iter().enumerate() {
                    for (tile32_x, &tile32_idx) in row.iter().enumerate() {
                        let tile32 = self.tiles32[tile32_idx as usize];
                        let x = map_x * 32 + tile32_x * 2;
                        let y = map_y * 32 + tile32_y * 2;
                        map16[y][x] = tile32[0];
                        map16[y][x + 1] = tile32[1];
                        map16[y + 1][x] = tile32[2];
                        map16[y + 1][x + 1] = tile32[3];
                    }
                }
            }
        }
        map16
    }

    fn load_dynamic_tiles(&mut self) -> Result<()> {
        self.state.dynamic_tiles = DynamicTiles {
            modified: true,
            groups: DynamicTileType::ALL
                .into_iter()
                .map(|kind| DynamicTileGroup {
                    kind,
                    variants: vec![],
                })
                .collect(),
        };

        let small_patterns = [
            (DynamicTileType::CutGrass, 0x037D, 0x0DBF),
            (DynamicTileType::GreenBush, 0x0036, 0x0DC1),
            (DynamicTileType::HeavyBush, 0x0727, 0x0DC2),
            (DynamicTileType::HammerPeg, 0x021B, 0x0DC5),
            (DynamicTileType::LiftSign, 0x0101, 0x0DC0),
            (DynamicTileType::SmallGrayRock, 0x020F, 0x0DC4),
            (DynamicTileType::SmallBlackRock, 0x0239, 0x0DC4),
        ];
        let dig_tiles = [
            0x0034, 0x0035, 0x0071, 0x00DA, 0x00E1, 0x00E2, 0x00F8, 0x010D, 0x010E, 0x010F,
        ];
        let large_patterns = [
            (
                DynamicTileType::LargeGrayRock,
                [[0x036C, 0x036D], [0x0373, 0x0374]],
            ),
            (
                DynamicTileType::LargeBlackRock,
                [[0x023B, 0x023C], [0x023D, 0x023E]],
            ),
            (
                DynamicTileType::RockPile,
                [[0x0226, 0x0227], [0x0228, 0x0229]],
            ),
        ];
        let large_after = vec![vec![0x0DC7, 0x0DC8], vec![0x0DC9, 0x0DCA]];

        for parent in 0..=0x81 {
            if self.map_parents[parent] as usize != parent {
                continue;
            }
            let map16 = self.build_map16(parent);
            let width = map16[0].len();
            let height = map16.len();

            for y in 0..height {
                for x in 0..width {
                    let current = map16[y][x];
                    for &(kind, source, replacement) in &small_patterns {
                        if current == self.constants.convert_map16(source) {
                            self.add_dynamic_variant(
                                kind,
                                parent,
                                vec![vec![current]],
                                vec![vec![vec![replacement]]],
                            )?;
                        }
                    }
                    if dig_tiles
                        .iter()
                        .any(|&tile| current == self.constants.convert_map16(tile))
                    {
                        self.add_dynamic_variant(
                            DynamicTileType::DigTerrain,
                            parent,
                            vec![vec![current]],
                            vec![vec![vec![0x0DC3]]],
                        )?;
                    }

                    for &(kind, source) in &large_patterns {
                        let mut quadrant = None;
                        for (source_y, row) in source.iter().enumerate() {
                            for (source_x, &tile) in row.iter().enumerate() {
                                if current == self.constants.convert_map16(tile) {
                                    quadrant = Some((source_x, source_y));
                                }
                            }
                        }
                        if let Some((source_x, source_y)) = quadrant {
                            if x >= source_x
                                && y >= source_y
                                && x - source_x + 1 < width
                                && y - source_y + 1 < height
                            {
                                let start_x = x - source_x;
                                let start_y = y - source_y;
                                let before = vec![
                                    map16[start_y][start_x..start_x + 2].to_vec(),
                                    map16[start_y + 1][start_x..start_x + 2].to_vec(),
                                ];
                                self.add_dynamic_variant(
                                    kind,
                                    parent,
                                    before,
                                    vec![large_after.clone()],
                                )?;
                            }
                        }
                    }
                }
            }

            if parent == 0x18 {
                for (x, y) in [(0x0C, 0x17), (0x34, 0x1B)] {
                    self.add_dynamic_variant(
                        DynamicTileType::WoodenDoor,
                        parent,
                        vec![vec![map16[y][x], map16[y][x + 1]]],
                        vec![vec![vec![0x0D9E, 0x0DA0]]],
                    )?;
                }
            }

            'entrances: for entrance in 0..129 {
                let screen = self
                    .rom
                    .read_u16((SnesAddr(0x1BB96F) + entrance * 2).into())?
                    as usize;
                if self.map_parents[screen] as usize != parent {
                    continue;
                }
                let offset = self
                    .rom
                    .read_u16((SnesAddr(0x1BBA71) + entrance * 2).into())?
                    as usize;
                let x = (offset & 0x7F) / 2;
                let y = offset / 0x80;
                if x >= width || y >= height {
                    continue;
                }
                for door_y in y..=(y + 1).min(height - 1) {
                    let tile16 = self.tiles16[map16[door_y][x] as usize];
                    let (door_x, door_gfx) = if tile16[1].flip == Flip::None {
                        (x, tile16[1].gfx_char)
                    } else if x > 0 && tile16[0].flip == Flip::Horizontal {
                        (x - 1, tile16[0].gfx_char)
                    } else {
                        continue;
                    };
                    if door_x + 1 >= width
                        || door_y == 0
                        || (door_gfx != 0x0149 && door_gfx != 0x0169)
                    {
                        continue;
                    }
                    let kind = if door_gfx == 0x0149 {
                        DynamicTileType::SanctuaryDoor
                    } else {
                        DynamicTileType::HyruleCastleDoor
                    };
                    let before = vec![
                        vec![map16[door_y - 1][door_x], map16[door_y - 1][door_x + 1]],
                        vec![map16[door_y][door_x], map16[door_y][door_x + 1]],
                    ];
                    let after = if kind == DynamicTileType::SanctuaryDoor {
                        vec![
                            vec![vec![0x0DA2, 0x0DA3], vec![0x0DA4, 0x0DA5]],
                            vec![vec![0x0DA6, 0x0DA7], vec![0x0DA8, 0x0DA9]],
                            vec![vec![0x0DAA, 0x0DAB], vec![0x0DAC, 0x0DAD]],
                        ]
                    } else {
                        vec![
                            vec![vec![0x0DB0, 0x0DB1], vec![0x0DB2, 0x0DB3]],
                            vec![vec![0x0DB4, 0x0DB5], vec![0x0DB6, 0x0DB7]],
                        ]
                    };
                    self.add_dynamic_variant(kind, parent, before, after)?;
                    continue 'entrances;
                }
            }

            for screen in 0..0x80 {
                if self.map_parents[screen] as usize != parent {
                    continue;
                }
                let pointer = self
                    .rom
                    .read_u16((SnesAddr(0x1BC2F9) + screen as u32 * 2).into())?;
                let mut address = SnesAddr::from_bank_offset(0x1B, pointer);
                loop {
                    let offset = self.rom.read_u16(address.into())?;
                    if offset == 0xFFFF {
                        break;
                    }
                    let secret = self.rom.read_u8((address + 2).into())?;
                    address += 3;
                    let (kind, after) = match secret {
                        0x80 => (DynamicTileType::SecretHole, vec![vec![0x0DC6]]),
                        0x82 => (DynamicTileType::SecretPortal, vec![vec![0x0212]]),
                        0x84 => (
                            DynamicTileType::SecretStairs,
                            vec![vec![0x0912, 0x0913], vec![0x0914, 0x0915]],
                        ),
                        0x86 => (
                            DynamicTileType::SecretBombableEntrance,
                            vec![vec![0x0DAE, 0x0DAF]],
                        ),
                        _ => continue,
                    };
                    let x = (offset as usize & 0x7F) / 2;
                    let y = offset as usize / 0x80;
                    let map_width = after[0].len();
                    let map_height = after.len();
                    if x + map_width > width || y + map_height > height {
                        continue;
                    }
                    let mut before = Vec::with_capacity(map_height);
                    for row in &map16[y..y + map_height] {
                        before.push(row[x..x + map_width].to_vec());
                    }
                    self.add_dynamic_variant(kind, parent, before, vec![after])?;
                }
            }
        }

        let gate_parent = self.map_parents[0x1B] as usize;
        let gate_map = self.build_map16(gate_parent);
        let gate_x = 0x1E;
        let gate_y = 0x27;
        self.add_dynamic_variant(
            DynamicTileType::HyruleCastleGate,
            gate_parent,
            vec![
                gate_map[gate_y][gate_x..gate_x + 4].to_vec(),
                gate_map[gate_y + 1][gate_x..gate_x + 4].to_vec(),
            ],
            vec![vec![
                vec![0x0DBB, 0x0DBC, 0x0DBC, 0x0DBD],
                vec![0x0DB8, 0x0DB9, 0x0DB9, 0x0DBA],
            ]],
        )?;

        let grave_parent = self.map_parents[0x14] as usize;
        let grave_map = self.build_map16(grave_parent);
        for grave in 0..15 {
            if grave == 5 {
                continue;
            }
            let offset = self
                .rom
                .read_u16((self.constants.grave_tilemap_addr + grave * 2).into())?
                as usize;
            let revealed = self
                .rom
                .read_u16((self.constants.grave_reveal_addr + grave * 2).into())?;
            let (kind, after) = match revealed {
                0x30 => (
                    DynamicTileType::GraveCorpse,
                    vec![
                        vec![0x0DCB, 0x0DCC],
                        vec![0x0DCD, 0x0DCE],
                        vec![0x0DCF, 0x0DD0],
                    ],
                ),
                0x38 => (
                    DynamicTileType::GraveStairs,
                    vec![
                        vec![0x0DCB, 0x0DCC],
                        vec![0x0DD1, 0x0DD2],
                        vec![0x0DD3, 0x0DD4],
                    ],
                ),
                0x58 => (
                    DynamicTileType::GravePit,
                    vec![
                        vec![0x0DCB, 0x0DCC],
                        vec![0x0DD5, 0x0DD6],
                        vec![0x0DD7, 0x0DD8],
                    ],
                ),
                _ => continue,
            };
            let x = (offset & 0x7F) / 2;
            let y = offset / 0x80;
            if y == 0 || y + 1 >= grave_map.len() || x + 1 >= grave_map[0].len() {
                continue;
            }
            let before = vec![
                grave_map[y - 1][x..x + 2].to_vec(),
                grave_map[y][x..x + 2].to_vec(),
                grave_map[y + 1][x..x + 2].to_vec(),
            ];
            self.add_dynamic_variant(kind, grave_parent, before, vec![after])?;
        }

        for group in &self.state.dynamic_tiles.groups {
            info!("Imported {} {} variants", group.variants.len(), group.kind);
        }
        Ok(())
    }

    fn ensure_palette_full_rows(&mut self) -> Result<()> {
        for pal in &mut self.state.palettes {
            let size = (pal.tiles.len().div_ceil(16) * 16).max(16);
            pal.tiles.resize(size, Tile::default());
            pal.modified = true;
        }
        Ok(())
    }

    fn assign_bg_colors(&mut self) -> Result<()> {
        // If a given BG color is consistently used with a palette, then assign
        // it to color 0 of the palette. This won't have any effect in-game, but
        // it helps with rendering the tileset more accurately in the editor.
        for p in &mut self.state.palettes {
            if let Some(&c) = self.pal_bg_color.get(&p.id) {
                p.colors[0] = c;
            }
        }
        Ok(())
    }
}

fn decompress(rom: &Rom, mut addr: PcAddr, big_endian_offset: bool) -> Result<Vec<u8>> {
    let mut out: Vec<u8> = Vec::new();
    loop {
        let byte = rom.read_u8(addr)? as isize;
        addr += 1;
        if byte == 0xFF {
            return Ok(out);
        }
        let mut block_type = byte >> 5;
        let size: usize;
        if block_type != 7 {
            size = ((byte & 0x1F) + 1) as usize;
        } else {
            size = (((byte & 3) << 8 | rom.read_u8(addr)? as isize) + 1) as usize;
            addr += 1;
            block_type = (byte >> 2) & 7;
        }

        match block_type {
            0 => {
                // Raw block
                out.extend(rom.read_n(addr, size)?);
                addr += size as u32;
            }
            1 => {
                // Byte-level RLE block
                let value = rom.read_u8(addr)?;
                addr += 1;
                out.extend(&vec![value; size]);
            }
            2 => {
                // Word-level RLE block
                let b0 = rom.read_u8(addr)?;
                let b1 = rom.read_u8(addr + 1)?;
                addr += 2;
                out.extend(&[b0, b1].repeat(size >> 1));
                if size & 1 == 1 {
                    out.push(b0);
                }
            }
            3 => {
                // Incrementing sequence
                let mut b = rom.read_u8(addr)?;
                addr += 1;
                for _ in 0..size {
                    out.push(b);
                    b = b.wrapping_add(1);
                }
            }
            4 => {
                // Copy earlier output, with absolute offset:
                let offset = if big_endian_offset {
                    (rom.read_u8(addr)? as usize) << 8 | rom.read_u8(addr + 1)? as usize
                } else {
                    rom.read_u16(addr)? as usize
                };
                assert!(offset < out.len());
                addr += 2;
                for i in offset..(offset + size) {
                    out.push(out[i]);
                }
            }
            _ => {
                bail!("Unexpected/impossible block type: {block_type}");
            }
        }
    }
}
