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
        Area, AreaId, AreaName, ColorRGB, ColorValue, EditorState, Flip, Palette, PaletteId,
        Screen, Tile, TileIdx,
    },
    update::update_palette_order,
};

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
    map_cnt: u32,
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
            map_cnt: 0x90,
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
            map_cnt: 0x90,
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
        Ok(self.data[addr.0 as usize] as u8)
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
                for y in 0..8 {
                    for x in 0..8 {
                        let c0 = (data[j * 24 + y * 2] >> (7 - x)) & 1;
                        let c1 = (data[j * 24 + y * 2 + 1] >> (7 - x)) & 1;
                        let c2 = (data[j * 24 + y + 16] >> (7 - x)) & 1;
                        let c = c0 | (c1 << 1) | (c2 << 2);
                        tile[y][x] = c;
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
        for i in 0..self.constants.map_cnt {
            let high_addr = SnesAddr(rom.read_u24((self.constants.map_high_addr + i * 3).into())?);
            let high_data = decompress(rom, high_addr.into(), true)?;

            let low_addr = SnesAddr(rom.read_u24((self.constants.map_low_addr + i * 3).into())?);
            let low_data = decompress(rom, low_addr.into(), true)?;

            ensure!(high_data.len() == 256);
            ensure!(low_data.len() == 256);

            let mut block: [[Tile32Idx; 16]; 16] = [[0; 16]; 16];
            for y in 0..16 {
                for x in 0..16 {
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

                    block[y][x] = tile32_idx;
                }
            }
            self.map_tiles.push(block);
        }
        Ok(())
    }

    fn load_map_parents(&mut self) -> Result<()> {
        let mut parents: Vec<MapIdx> = (0..self.constants.map_cnt as MapIdx).collect();

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
        // parents[148] = 128;
        // parents[149] = 3;
        // parents[150] = 91;
        // parents[151] = 0;
        // parents[156] = 67;
        // parents[157] = 0;
        // parents[158] = 0;
        // parents[159] = 44;

        self.map_parents = parents;
        Ok(())
    }

    fn load_map_palettes(&mut self) -> Result<()> {
        let rom = &self.rom;
        for i in 0..self.constants.map_cnt as usize {
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
        for i in 0..self.constants.map_cnt as usize {
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

        fn strip_tile(mut tile: Tile) -> Tile {
            // We clear these fields when looking up matching tiles, since these
            // fields will vary over the course of processing.
            tile.id = None;
            tile.h_flippable = false;
            tile.v_flippable = false;
            tile
        }

        for palette_idx in 0..self.state.palettes.len() {
            for (idx, tile) in self.state.palettes[palette_idx].tiles.iter().enumerate() {
                for flip in [Flip::None, Flip::Horizontal, Flip::Vertical, Flip::Both] {
                    tile_lookup[palette_idx].insert(
                        strip_tile(flip.apply_to_tile(tile.clone())),
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
                    0x40..0x80 => [18, 17, 10],       // dark world
                    0x80 | 0x82 | 0x83 => [6, 14, 6], // dark green background (Special World)
                    _ => [9, 19, 9],                  // default green background
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
            let mut area: Area = Area {
                modified: false,
                name: area_name,
                theme: self.theme.clone(),
                vanilla_map_id: Some(parent as u8),
                bg_color,
                size: (size.0 * 2, size.1 * 2),
                screens: vec![],
            };
            self.state.area_names.push(area.name.clone());
            for y in 0..size.1 * 2 {
                for x in 0..size.0 * 2 {
                    area.screens.push(Screen {
                        position: (x, y),
                        palettes: [[0; 32]; 32],
                        tiles: [[0; 32]; 32],
                        flips: [[Flip::None; 32]; 32],
                    });
                }
            }
            for my in 0..size.1 as usize {
                for mx in 0..size.0 as usize {
                    let map_idx = parent + my as usize * 8 + mx as usize;
                    let tiles = &self.map_tiles[map_idx];
                    for ty in 0..16 {
                        for tx in 0..16 {
                            let t32_idx = tiles[ty][tx];
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
                                    let pixels =
                                        t8.flip.apply_to_pixels(self.tiles8[tiles8_idx as usize]);
                                    let tile = Tile {
                                        id: None,
                                        priority: t8.priority,
                                        h_flippable: false,
                                        v_flippable: false,
                                        collision,
                                        pixels,
                                    };
                                    let (tile_idx, flip) = match tile_lookup[palette_idx].get(&tile)
                                    {
                                        Some(x) => *x,
                                        None => {
                                            let idx = self.state.palettes[palette_idx].tiles.len()
                                                as TileIdx;
                                            self.state.palettes[palette_idx].tiles.push(tile);
                                            for flip in [
                                                Flip::None,
                                                Flip::Horizontal,
                                                Flip::Vertical,
                                                Flip::Both,
                                            ] {
                                                tile_lookup[palette_idx].insert(
                                                    strip_tile(flip.apply_to_tile(tile.clone())),
                                                    (idx, flip),
                                                );
                                            }
                                            (idx, Flip::None)
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

                                    area.set_tile(x as u16, y as u16, tile_idx as u16).unwrap();
                                    area.set_palette(x as u16, y as u16, pal_id).unwrap();
                                    area.set_flip(x as u16, y as u16, flip).unwrap();

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
            self.state
                .set_area(crate::state::AreaPosition::Main, area)?;
            save_area_json(self.state, &self.state.main_area_id.clone())?;
        }
        Ok(())
    }

    fn ensure_palette_full_rows(&mut self) -> Result<()> {
        for pal in &mut self.state.palettes {
            let size = ((pal.tiles.len() + 15) / 16 * 16).max(16);
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
