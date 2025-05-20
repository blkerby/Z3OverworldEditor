use anyhow::{bail, Context, Result};
use hashbrown::{HashMap, HashSet};
use log::info;
use notify::Watcher;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

use serde::{Deserialize, Serialize};

use crate::{
    message::{Message, SelectionSource},
    persist::{self, load_area, save_area},
};

pub type ColorValue = u8; // Color value (0-31)
pub type ColorIdx = u8; // Index into 4bpp palette (0-15)
pub type PaletteId = u16; // external ID of the palette
pub type PaletteIdx = usize; // internal ID of the palette (index into State.palettes)
pub type TileIdx = u16; // Index into palette's tile list
pub type PixelCoord = u8; // Index into 8x8 row or column (0-7)
pub type TileCoord = u16; // Index into area: number of 8x8 tiles from top-left corner
pub type AreaName = String;
pub type ThemeName = String;
pub type CollisionType = u8;
pub type ColorRGB = [ColorValue; 3];

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AreaId {
    pub area: AreaName,
    pub theme: ThemeName,
}

#[derive(Copy, Clone, Serialize, Deserialize, Default, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub priority: bool,
    pub collision: CollisionType,
    pub h_flippable: bool,
    pub v_flippable: bool,
    pub pixels: [[ColorIdx; 8]; 8],
}

#[derive(Clone, Serialize, Deserialize, Default, Debug)]
pub struct Palette {
    #[serde(skip_serializing, skip_deserializing)]
    pub modified: bool,
    #[serde(skip_serializing, skip_deserializing)]
    pub name: String,
    pub id: PaletteId,
    pub colors: [ColorRGB; 16],
    pub tiles: Vec<Tile>,
}

#[derive(Serialize, Deserialize, Default, Clone)]
pub struct GlobalConfig {
    #[serde(skip_serializing, skip_deserializing)]
    pub modified: bool,
    pub project_dir: Option<PathBuf>,
    #[serde(default = "default_pixel_size")]
    pub pixel_size: f32,
    #[serde(default = "default_grid_alpha")]
    pub grid_alpha: f32,
}

pub const MIN_PIXEL_SIZE: f32 = 1.0;
pub const MAX_PIXEL_SIZE: f32 = 8.0;

fn default_pixel_size() -> f32 {
    3.0
}

fn default_grid_alpha() -> f32 {
    0.1
}

#[derive(Clone, Copy, Serialize_repr, Deserialize_repr, Default, Debug, PartialEq, Eq)]
#[repr(u8)]
pub enum Flip {
    #[default]
    None = 0,
    Horizontal = 1,
    Vertical = 2,
    Both = 3,
}

impl Flip {
    pub fn flip_horizontally(self) -> Self {
        match self {
            Flip::None => Flip::Horizontal,
            Flip::Horizontal => Flip::None,
            Flip::Vertical => Flip::Both,
            Flip::Both => Flip::Vertical,
        }
    }

    pub fn flip_vertically(self) -> Self {
        match self {
            Flip::None => Flip::Vertical,
            Flip::Horizontal => Flip::Both,
            Flip::Vertical => Flip::None,
            Flip::Both => Flip::Horizontal,
        }
    }

    pub fn apply_to_pixels(self, mut pixels: [[ColorIdx; 8]; 8]) -> [[ColorIdx; 8]; 8] {
        match self {
            Flip::None => {}
            Flip::Horizontal => {
                for row in pixels.iter_mut() {
                    row.reverse();
                }
            }
            Flip::Vertical => {
                pixels.reverse();
            }
            Flip::Both => {
                for row in pixels.iter_mut() {
                    row.reverse();
                }
                pixels.reverse();
            }
        }
        pixels
    }

    pub fn apply_to_tile(self, mut tile: Tile) -> Tile {
        // TODO: also apply flips to slope collisions
        tile.pixels = self.apply_to_pixels(tile.pixels);
        tile
    }

    pub fn apply_to_flip(self, mut flip: Flip) -> Flip {
        match self {
            Flip::None => {}
            Flip::Horizontal => {
                flip = flip.flip_horizontally();
            }
            Flip::Vertical => {
                flip = flip.flip_vertically();
            }
            Flip::Both => {
                flip = flip.flip_horizontally();
                flip = flip.flip_vertically();
            }
        }
        flip
    }
}

#[derive(Serialize, Deserialize, Default)]
pub struct Screen {
    // X and Y position of the screen (256 x 256 block) within the area, in screen counts:
    // The screens are always listed in row-major order, so `position` is
    // redundant; its only purpose is to improve readability of the JSON.
    pub position: (u8, u8),
    pub palettes: [[PaletteId; 32]; 32],
    pub tiles: [[TileIdx; 32]; 32],
    pub flips: [[Flip; 32]; 32],
}

#[derive(Serialize, Deserialize, Default)]
pub struct Area {
    #[serde(skip_serializing, skip_deserializing)]
    pub modified: bool,
    #[serde(skip_serializing, skip_deserializing)]
    pub name: AreaName,
    #[serde(skip_serializing, skip_deserializing)]
    pub theme: ThemeName,
    pub vanilla_map_id: Option<u8>,
    pub bg_color: ColorRGB,
    // X and Y dimensions, measured in number of screens:
    pub size: (u8, u8),
    // A 'screen' is a 256x256 pixel section, roughly the size that fits on camera at once.
    // Splitting it up like this helps with formatting of the JSON, e.g. for viewing git diffs.
    pub screens: Vec<Screen>,
}

impl Area {
    pub fn id(&self) -> AreaId {
        AreaId {
            area: self.name.clone(),
            theme: self.theme.clone(),
        }
    }

    pub fn get_screen_coords(&self, x: TileCoord, y: TileCoord) -> Result<(usize, usize, usize)> {
        if x >= self.size.0 as TileCoord * 32 || y >= self.size.1 as TileCoord * 32 {
            bail!("out of range");
        }
        let screen_x = (x / 32) as usize;
        let screen_y = (y / 32) as usize;
        let screen_i = screen_y * self.size.0 as usize + screen_x;
        Ok((screen_i, (x % 32) as usize, (y % 32) as usize))
    }

    pub fn get_palette(&self, x: TileCoord, y: TileCoord) -> Result<PaletteId> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        Ok(self.screens[i].palettes[sy][sx])
    }

    pub fn get_tile(&self, x: TileCoord, y: TileCoord) -> Result<TileIdx> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        Ok(self.screens[i].tiles[sy][sx])
    }

    pub fn get_flip(&self, x: TileCoord, y: TileCoord) -> Result<Flip> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        Ok(self.screens[i].flips[sy][sx])
    }

    pub fn set_tile(&mut self, x: TileCoord, y: TileCoord, tile_idx: TileIdx) -> Result<()> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        self.screens[i].tiles[sy][sx] = tile_idx;
        Ok(())
    }

    pub fn set_palette(&mut self, x: TileCoord, y: TileCoord, palette_id: PaletteId) -> Result<()> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        self.screens[i].palettes[sy][sx] = palette_id;
        Ok(())
    }

    pub fn set_flip(&mut self, x: TileCoord, y: TileCoord, flip: Flip) -> Result<()> {
        let (i, sx, sy) = self.get_screen_coords(x, y)?;
        self.screens[i].flips[sy][sx] = flip;
        Ok(())
    }

    pub fn get_unique_palettes(&self) -> Vec<PaletteId> {
        let mut palettes: HashSet<PaletteId> = HashSet::new();
        for s in &self.screens {
            for y in 0..32 {
                for x in 0..32 {
                    palettes.insert(s.palettes[y][x]);
                }
            }
        }
        let mut palettes: Vec<PaletteId> = palettes.into_iter().collect();
        palettes.sort();
        palettes
    }
}

pub enum Dialogue {
    Settings,
    ImportROMConfirm,
    ImportROMProgress,
    AddPalette {
        name: String,
        id: PaletteId,
    },
    RenamePalette {
        name: String,
    },
    DeletePalette,
    AddArea {
        name: AreaName,
        size: (u8, u8),
    },
    EditArea {
        name: AreaName,
    },
    DeleteArea,
    AddTheme {
        name: ThemeName,
    },
    RenameTheme {
        name: ThemeName,
    },
    DeleteTheme,
    Help,
    RebuildProject,
    ModifiedReload,
    MovingTilesProgress,
    MoveTiles {
        src_selection: TileBlock,
        dst_selection: TileBlock,
    },
}

#[derive(Default, Debug, Clone, PartialEq, Eq)]
pub struct TileBlock {
    pub size: (TileCoord, TileCoord),
    pub palettes: Vec<Vec<PaletteId>>,
    pub tiles: Vec<Vec<TileIdx>>,
    pub flips: Vec<Vec<Flip>>,
}

// At the moment, Iced's support for tracking widget focus is fairly incomplete,
// so we handle it manually. This is used to determine the behavior of
// keyboard inputs (e.g. arrow keys to move through pick-lists or navigate grids).
#[derive(Copy, Clone, Default, Debug)]
pub enum Focus {
    #[default]
    None,
    PickArea(AreaPosition),
    PickTheme(AreaPosition),
    Area(AreaPosition),
    PickPalette,
    PaletteColor,
    GraphicsPixel,
    TilesetTile,
}

#[derive(Copy, Clone, Default, Debug)]
pub enum SidePanelView {
    #[default]
    Tileset,
    Area,
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub enum AreaPosition {
    #[default]
    Main,
    Side,
}

#[derive(Copy, Clone, Default, Debug, PartialEq, Eq)]
pub enum Tool {
    #[default]
    Select,
    Brush,
    Move,
}

pub struct EditorState {
    pub global_config_path: PathBuf,
    pub global_config: GlobalConfig,

    // Project data: Areas are loaded/unloaded dynamically
    // to limit memory usage and start-up time. Everything else is fully loaded.
    pub palettes: Vec<Palette>,
    pub areas: HashMap<AreaId, Area>,
    pub area_names: Vec<AreaName>,
    pub theme_names: Vec<ThemeName>,

    // Undo functionality:
    pub undo_stack: Vec<(Message, Message)>,
    pub redo_stack: Vec<(Message, Message)>,

    // Settings-related data:
    pub rom_path: Option<PathBuf>,

    // General editing state:
    pub focus: Focus,
    pub tool: Tool,
    pub shift_brush: bool,
    pub side_panel_view: SidePanelView,

    // Palette editing state:
    pub palette_idx: PaletteIdx,
    pub color_idx: Option<ColorIdx>,
    pub selected_color: ColorRGB,
    pub identify_color: bool,

    // Tile editing state:
    pub tile_idx: Option<TileIdx>,
    pub identify_tile: bool,

    // Graphics editing state:
    pub pixel_coords: Option<(PixelCoord, PixelCoord)>,

    // Area editing state:
    pub main_area_id: AreaId,
    pub side_area_id: AreaId,
    pub selection_source: SelectionSource,
    pub start_coords: Option<(TileCoord, TileCoord)>,
    pub end_coords: Option<(TileCoord, TileCoord)>,
    pub selected_tile_block: TileBlock,
    pub selected_gfx: Vec<Vec<Tile>>,
    pub show_grid: bool,

    // Filesystem watch (to detect externa modifications)
    pub watcher: Option<notify::RecommendedWatcher>,
    pub watch_paths: Vec<PathBuf>,
    pub watch_enabled: bool,
    pub files_modified_notification: Arc<Mutex<bool>>,

    // Other editor state:
    pub dialogue: Option<Dialogue>,

    // Cached data:
    pub palettes_id_idx_map: HashMap<PaletteId, usize>,
}

impl EditorState {
    pub fn main_area(&self) -> &Area {
        &self.areas[&self.main_area_id]
    }

    pub fn main_area_mut(&mut self) -> &mut Area {
        self.areas.get_mut(&self.main_area_id.clone()).unwrap()
    }

    pub fn side_area(&self) -> &Area {
        &self.areas[&self.side_area_id]
    }

    pub fn area_id(&self, position: AreaPosition) -> &AreaId {
        match position {
            AreaPosition::Main => &self.main_area_id,
            AreaPosition::Side => &self.side_area_id,
        }
    }

    pub fn area_id_mut(&mut self, position: AreaPosition) -> &mut AreaId {
        match position {
            AreaPosition::Main => &mut self.main_area_id,
            AreaPosition::Side => &mut self.side_area_id,
        }
    }

    pub fn area(&self, position: AreaPosition) -> &Area {
        &self.areas[self.area_id(position)]
    }

    pub fn area_mut(&mut self, position: AreaPosition) -> &mut Area {
        self.areas.get_mut(&self.area_id(position).clone()).unwrap()
    }

    pub fn set_area(&mut self, position: AreaPosition, area: Area) -> Result<()> {
        let id = area.id();
        self.areas.insert(id.clone(), area);
        match position {
            AreaPosition::Main => {
                self.main_area_id = id;
            }
            AreaPosition::Side => {
                self.side_area_id = id;
            }
        }
        self.cleanup_areas()?;
        Ok(())
    }

    pub fn load_area(&mut self, area_id: &AreaId) -> Result<()> {
        let area = load_area(self, area_id)?;
        self.areas.insert(area_id.clone(), area);
        Ok(())
    }

    pub fn switch_area(&mut self, position: AreaPosition, area_id: &AreaId) -> Result<()> {
        if !self.areas.contains_key(area_id) {
            self.load_area(area_id)?;
        }
        *self.area_id_mut(position) = area_id.clone();
        self.cleanup_areas()?;
        Ok(())
    }

    pub fn cleanup_areas(&mut self) -> Result<()> {
        // Unload areas that aren't currently in use.
        let mut delete_keys: HashSet<AreaId> = self.areas.keys().cloned().collect();
        delete_keys.remove(&self.main_area_id);
        delete_keys.remove(&self.side_area_id);
        for key in delete_keys {
            save_area(self, &key)?;
            self.areas.remove(&key);
        }
        Ok(())
    }

    pub fn enable_watch_file_changes(&mut self) -> Result<()> {
        if let Some(watcher) = &mut self.watcher {
            if !self.watch_enabled {
                for p in &self.watch_paths {
                    if let Err(e) = watcher.watch(p, notify::RecursiveMode::Recursive) {
                        info!("Unable to watch path {}: {}", p.display(), e);
                    }
                }
            }
            self.watch_enabled = true;
        }
        Ok(())
    }

    pub fn disable_watch_file_changes(&mut self) -> Result<()> {
        if let Some(watcher) = &mut self.watcher {
            self.watch_enabled = false;
            for p in &self.watch_paths {
                if let Err(e) = watcher.unwatch(p) {
                    info!("Unable to unwatch path {}: {}", p.display(), e);
                }
            }
        }
        Ok(())
    }
}

fn get_global_config_path() -> Result<PathBuf> {
    let project_dirs = directories::ProjectDirs::from("", "", "Z3OverworldEditor")
        .context("Unable to open global config directory.")?;
    let config_dir = project_dirs.config_dir();
    let config_path = config_dir.join("config.json");
    Ok(config_path)
}

pub fn ensure_themes_non_empty(state: &mut EditorState) {
    if state.theme_names.len() == 0 {
        state.theme_names.push("Base".to_string());
    }
}

pub fn ensure_areas_non_empty(state: &mut EditorState) -> Result<()> {
    if state.area_names.len() == 0 {
        state.area_names.push("Example".to_string());
        let mut area = Area::default();
        area.name = "Example".to_string();
        area.theme = "Base".to_string();
        area.size = (2, 2);
        for y in 0..2 {
            for x in 0..2 {
                area.screens.push(Screen {
                    position: (x, y),
                    palettes: [[0; 32]; 32],
                    tiles: [[0; 32]; 32],
                    flips: [[Flip::None; 32]; 32],
                });
            }
        }
        area.modified = true;
        state.set_area(AreaPosition::Main, area)?;
    }
    Ok(())
}

pub fn ensure_palettes_non_empty(state: &mut EditorState) {
    if state.palettes.len() == 0 {
        let mut pal = Palette::default();
        pal.modified = true;
        pal.name = "Default".to_string();
        pal.tiles = vec![
            Tile {
                priority: false,
                collision: 0,
                h_flippable: true,
                v_flippable: true,
                pixels: [[0; 8]; 8]
            };
            16
        ];
        state.palettes.push(pal);
    }
}

pub fn get_initial_state() -> Result<EditorState> {
    let mut state = EditorState {
        global_config_path: get_global_config_path()?,
        global_config: GlobalConfig::default(),
        rom_path: None,
        palettes: vec![],
        areas: HashMap::new(),
        main_area_id: AreaId {
            area: "Example".to_string(),
            theme: "Base".to_string(),
        },
        side_area_id: AreaId {
            area: "Example".to_string(),
            theme: "Base".to_string(),
        },
        area_names: vec![],
        theme_names: vec![],
        undo_stack: vec![],
        redo_stack: vec![],
        tool: Tool::default(),
        shift_brush: false,
        side_panel_view: SidePanelView::default(),
        focus: Focus::None,
        palette_idx: 0,
        color_idx: None,
        selected_color: [0, 0, 0],
        identify_color: false,
        tile_idx: None,
        identify_tile: false,
        selection_source: SelectionSource::Area(AreaPosition::Main),
        start_coords: None,
        end_coords: None,
        selected_tile_block: TileBlock::default(),
        selected_gfx: vec![],
        show_grid: false,
        pixel_coords: None,
        watcher: None,
        watch_enabled: false,
        watch_paths: vec![],
        files_modified_notification: Arc::new(Mutex::new(false)),
        dialogue: None,
        palettes_id_idx_map: HashMap::new(),
    };
    if let Err(err) = persist::load_global_config(&mut state) {
        info!("Unable to load global config, using default: {}", err);
    }
    if let Err(err) = persist::load_project(&mut state) {
        info!("Unable to load project: {}", err);
        state.global_config.project_dir = None;
    }
    ensure_themes_non_empty(&mut state);
    ensure_areas_non_empty(&mut state)?;
    ensure_palettes_non_empty(&mut state);
    Ok(state)
}
