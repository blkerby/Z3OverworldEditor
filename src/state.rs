use anyhow::{bail, Context, Result};
use hashbrown::{HashMap, HashSet};
use log::info;
use notify::Watcher;
use serde_repr::{Deserialize_repr, Serialize_repr};
use std::{
    path::{Path, PathBuf},
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
pub type TilePixels = [[ColorIdx; 8]; 8];

#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
#[serde(rename_all = "snake_case")]
pub enum DynamicTileType {
    CutGrass,
    DigTerrain,
    GreenBush,
    HeavyBush,
    HammerPeg,
    LiftSign,
    SmallGrayRock,
    SmallBlackRock,
    LargeGrayRock,
    LargeBlackRock,
    RockPile,
    SecretHole,
    SecretPortal,
    SecretBombableEntrance,
    SecretStairs,
    WoodenDoor,
    SanctuaryDoor,
    HyruleCastleDoor,
    GraveCorpse,
    GraveStairs,
    GravePit,
    HyruleCastleGate,
}

impl DynamicTileType {
    pub const ALL: [Self; 22] = [
        Self::CutGrass,
        Self::DigTerrain,
        Self::GreenBush,
        Self::HeavyBush,
        Self::HammerPeg,
        Self::LiftSign,
        Self::SmallGrayRock,
        Self::SmallBlackRock,
        Self::LargeGrayRock,
        Self::LargeBlackRock,
        Self::RockPile,
        Self::SecretHole,
        Self::SecretPortal,
        Self::SecretBombableEntrance,
        Self::SecretStairs,
        Self::WoodenDoor,
        Self::SanctuaryDoor,
        Self::HyruleCastleDoor,
        Self::GraveCorpse,
        Self::GraveStairs,
        Self::GravePit,
        Self::HyruleCastleGate,
    ];

    pub fn size(self) -> (usize, usize) {
        match self {
            Self::LargeGrayRock
            | Self::LargeBlackRock
            | Self::RockPile
            | Self::SecretStairs
            | Self::SanctuaryDoor
            | Self::HyruleCastleDoor => (4, 4),
            Self::GraveCorpse | Self::GraveStairs | Self::GravePit => (4, 6),
            Self::HyruleCastleGate => (8, 4),
            Self::SecretBombableEntrance | Self::WoodenDoor => (4, 2),
            _ => (2, 2),
        }
    }

    pub fn after_frame_count(self) -> usize {
        match self {
            Self::SanctuaryDoor => 3,
            Self::HyruleCastleDoor => 2,
            _ => 1,
        }
    }

    pub fn expected_property(self) -> Option<CollisionType> {
        match self {
            Self::CutGrass => Some(0x40),
            Self::DigTerrain => Some(0x48),
            Self::GreenBush => Some(0x50),
            Self::HeavyBush => Some(0x51),
            Self::HammerPeg => Some(0x27),
            Self::LiftSign => Some(0x54),
            Self::SmallGrayRock => Some(0x52),
            Self::SmallBlackRock => Some(0x53),
            Self::LargeGrayRock => Some(0x55),
            Self::LargeBlackRock => Some(0x56),
            Self::RockPile => Some(0x57),
            Self::GraveCorpse | Self::GraveStairs | Self::GravePit => Some(0x42),
            _ => None,
        }
    }

    pub fn expects_property_at(self, x: usize, y: usize, width: usize, height: usize) -> bool {
        match self {
            Self::GraveCorpse | Self::GraveStairs | Self::GravePit => {
                y + 2 == height && (x == width / 2 - 1 || x == width / 2)
            }
            _ => true,
        }
    }
}

impl std::fmt::Display for DynamicTileType {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::CutGrass => "Cut grass",
            Self::DigTerrain => "Dig terrain",
            Self::GreenBush => "Cut/lift green bush",
            Self::HeavyBush => "Cut/lift heavy bush",
            Self::HammerPeg => "Hammer peg",
            Self::LiftSign => "Lift sign",
            Self::SmallGrayRock => "Lift small gray rock",
            Self::SmallBlackRock => "Lift small black rock",
            Self::LargeGrayRock => "Lift large gray rock",
            Self::LargeBlackRock => "Lift large black rock",
            Self::RockPile => "Dash through rock pile",
            Self::SecretHole => "Reveal hole",
            Self::SecretPortal => "Reveal portal",
            Self::SecretBombableEntrance => "Reveal bombable entrance",
            Self::SecretStairs => "Reveal stairs",
            Self::WoodenDoor => "Open wooden door",
            Self::SanctuaryDoor => "Open Sanctuary door",
            Self::HyruleCastleDoor => "Open Hyrule Castle door",
            Self::GraveCorpse => "Open grave with corpse",
            Self::GraveStairs => "Open grave with stairs",
            Self::GravePit => "Open grave with pit",
            Self::HyruleCastleGate => "Open Hyrule Castle gate",
        })
    }
}

#[derive(Copy, Clone, Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
pub struct TilePlacement {
    pub palette: PaletteId,
    pub tile: TileIdx,
    pub flip: Flip,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum DynamicTileTarget {
    Before,
    After(usize),
}

#[derive(Clone, Serialize, Deserialize, Default, Debug, PartialEq, Eq)]
pub struct DynamicTileGrid {
    pub tiles: Vec<Vec<Option<TilePlacement>>>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DynamicTileVariant {
    pub before: DynamicTileGrid,
    pub after_frames: Vec<DynamicTileGrid>,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct DynamicTileGroup {
    #[serde(rename = "type")]
    pub kind: DynamicTileType,
    pub variants: Vec<DynamicTileVariant>,
}

#[derive(Serialize, Deserialize, Default, Debug)]
pub struct DynamicTiles {
    #[serde(skip)]
    pub modified: bool,
    pub groups: Vec<DynamicTileGroup>,
}

#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct AreaId {
    pub area: AreaName,
    pub theme: ThemeName,
}

#[derive(Copy, Clone, Serialize, Deserialize, Default, Debug, PartialEq, Eq, Hash)]
pub struct Tile {
    pub id: Option<TileIdx>,
    pub priority: bool,
    pub collision: CollisionType,
    pub h_flippable: bool,
    pub v_flippable: bool,
    pub pixels: TilePixels,
}

#[derive(Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
pub struct AnimatedTileGroup {
    pub base_tile: TileIdx,
    pub frames: Vec<[TilePixels; 16]>,
    pub frame_hold: u16,
    pub phase_offset: u16,
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
    #[serde(default)]
    pub animated_tile_groups: Vec<AnimatedTileGroup>,
}

impl Palette {
    pub fn validate_animated_tile_groups(&self) -> Result<()> {
        let mut base_tiles = HashSet::new();
        for group in &self.animated_tile_groups {
            if group.base_tile % 16 != 0
                || group.base_tile as usize + 16 > self.tiles.len()
                || group.frames.is_empty()
                || group.frame_hold == 0
                || !base_tiles.insert(group.base_tile)
            {
                bail!("invalid animated tile group at tile {}", group.base_tile);
            }
        }
        Ok(())
    }
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum PixelTarget {
    Regular(TileIdx),
    Animated { tile_idx: TileIdx, frame: usize },
}

#[derive(Serialize, Deserialize, Clone)]
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

impl Default for GlobalConfig {
    fn default() -> Self {
        Self {
            modified: true,
            project_dir: None,
            pixel_size: default_pixel_size(),
            grid_alpha: default_grid_alpha(),
        }
    }
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
        if (0x10..0x1c).contains(&tile.collision) {
            tile.collision ^= self as CollisionType;
        }
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

#[derive(Copy, Clone, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Background {
    Bg1,
    Bg2,
}

impl std::fmt::Display for Background {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Bg1 => write!(f, "BG1"),
            Self::Bg2 => write!(f, "BG2"),
        }
    }
}

#[derive(Copy, Clone, Default, Serialize, Deserialize, Debug, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum BackgroundLayering {
    #[default]
    None,
    HalfAdd,
    Backdrop,
}

impl BackgroundLayering {
    pub const ALL: [Self; 3] = [Self::None, Self::HalfAdd, Self::Backdrop];
}

impl std::fmt::Display for BackgroundLayering {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::None => write!(f, "None"),
            Self::HalfAdd => write!(f, "Half add"),
            Self::Backdrop => write!(f, "Backdrop"),
        }
    }
}

pub fn is_valid_layer_name(name: &str) -> bool {
    !name.is_empty() && Path::new(name).file_name().and_then(|name| name.to_str()) == Some(name)
}

#[derive(Clone, Debug)]
pub struct Layer {
    pub modified: bool,
    pub name: String,
    pub background: Background,
    pub tiles: Vec<Vec<Option<TilePlacement>>>,
}

pub struct Area {
    pub modified: bool,
    pub name: AreaName,
    pub theme: ThemeName,
    pub vanilla_map_id: Option<u8>,
    pub other_world_area: Option<AreaName>,
    pub bg_color: ColorRGB,
    pub bg_layering: BackgroundLayering,
    pub bg_camera_follow_x: f32,
    pub bg_camera_drift_x: f32,
    pub bg_camera_follow_y: f32,
    pub bg_camera_drift_y: f32,
    // X and Y dimensions, measured in number of screens:
    pub size: (u8, u8),
    pub layers: Vec<Layer>,
}

impl Area {
    pub fn id(&self) -> AreaId {
        AreaId {
            area: self.name.clone(),
            theme: self.theme.clone(),
        }
    }

    pub fn get_layer_coords(&self, x: TileCoord, y: TileCoord) -> Result<(usize, usize)> {
        if x >= self.size.0 as TileCoord * 32 || y >= self.size.1 as TileCoord * 32 {
            bail!("out of range");
        }
        Ok((x as usize, y as usize))
    }

    pub fn get_layer_placement(
        &self,
        layer_idx: usize,
        x: TileCoord,
        y: TileCoord,
    ) -> Result<Option<TilePlacement>> {
        let (x, y) = self.get_layer_coords(x, y)?;
        Ok(self.layers.get(layer_idx).context("layer not found")?.tiles[y][x])
    }

    pub fn set_layer_placement(
        &mut self,
        layer_idx: usize,
        x: TileCoord,
        y: TileCoord,
        placement: Option<TilePlacement>,
    ) -> Result<()> {
        let (x, y) = self.get_layer_coords(x, y)?;
        let layer = self.layers.get_mut(layer_idx).context("layer not found")?;
        layer.tiles[y][x] = placement;
        layer.modified = true;
        self.modified = true;
        Ok(())
    }

    pub fn get_unique_palettes(&self) -> Vec<PaletteId> {
        let mut palettes: HashSet<PaletteId> = HashSet::new();
        for layer in &self.layers {
            for row in &layer.tiles {
                for placement in row.iter().flatten() {
                    palettes.insert(placement.palette);
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
    AnimatedTiles {
        base_tile: Option<TileIdx>,
        frame: usize,
        tile: usize,
    },
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
    pub placements: Vec<Vec<Option<TilePlacement>>>,
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
    PickDynamicTileType,
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
    Erase,
    Move,
}

pub struct EditorState {
    pub global_config_path: PathBuf,
    pub global_config: GlobalConfig,

    // Project data: Areas are loaded/unloaded dynamically
    // to limit memory usage and start-up time. Everything else is fully loaded.
    pub palettes: Vec<Palette>,
    pub dynamic_tiles: DynamicTiles,
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
    pub dynamic_tiles_open: bool,
    pub dynamic_tile_type: DynamicTileType,
    pub dynamic_tile_frames: Vec<usize>,

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
    pub pixel_target: Option<PixelTarget>,

    // Area editing state:
    pub main_area_id: AreaId,
    pub side_area_id: AreaId,
    pub selection_source: SelectionSource,
    pub start_coords: Option<(TileCoord, TileCoord)>,
    pub end_coords: Option<(TileCoord, TileCoord)>,
    pub hover_coords: Option<(TileCoord, TileCoord)>,
    pub selected_tile_block: TileBlock,
    pub selected_gfx: Vec<Vec<Option<Tile>>>,
    pub show_grid_16: bool,
    pub snap_grid_16: bool,
    pub main_layer_idx: usize,
    pub side_layer_idx: usize,
    pub visible_layers: Vec<bool>,
    pub layer_drawer_open: bool,

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

    pub fn selected_layer_idx(&self, position: AreaPosition) -> usize {
        match position {
            AreaPosition::Main => self.main_layer_idx,
            AreaPosition::Side => self.side_layer_idx,
        }
    }

    pub fn reset_layer_state(&mut self, position: AreaPosition) {
        let area = self.area(position);
        let bg1 = area
            .layers
            .iter()
            .position(|layer| layer.background == Background::Bg1);
        let bg2 = area
            .layers
            .iter()
            .position(|layer| layer.background == Background::Bg2)
            .unwrap_or(0);
        let layer_count = area.layers.len();
        match position {
            AreaPosition::Main => {
                self.main_layer_idx = bg2;
                self.visible_layers = vec![false; layer_count];
                if let Some(bg1) = bg1 {
                    self.visible_layers[bg1] = true;
                }
                self.visible_layers[bg2] = true;
            }
            AreaPosition::Side => self.side_layer_idx = bg2,
        }
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
        let changed = self.area_id(position) != area_id;
        if !self.areas.contains_key(area_id) {
            self.load_area(area_id)?;
        }
        *self.area_id_mut(position) = area_id.clone();
        if changed {
            self.reset_layer_state(position);
        }
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
    if state.theme_names.is_empty() {
        state.theme_names.push("Base".to_string());
    }
}

pub fn ensure_areas_non_empty(state: &mut EditorState) -> Result<()> {
    if state.area_names.is_empty() {
        state.area_names.push("Example".to_string());
        let area = Area {
            modified: true,
            name: "Example".to_string(),
            theme: "Base".to_string(),
            vanilla_map_id: None,
            other_world_area: None,
            bg_color: [0; 3],
            bg_layering: BackgroundLayering::None,
            bg_camera_follow_x: 1.0,
            bg_camera_drift_x: 0.0,
            bg_camera_follow_y: 1.0,
            bg_camera_drift_y: 0.0,
            size: (2, 2),
            layers: vec![Layer {
                modified: true,
                name: "Main".to_string(),
                background: Background::Bg2,
                tiles: vec![vec![Some(TilePlacement::default()); 64]; 64],
            }],
        };
        state.set_area(AreaPosition::Main, area)?;
    }
    Ok(())
}

pub fn ensure_palettes_non_empty(state: &mut EditorState) {
    if state.palettes.is_empty() {
        let pal = Palette {
            modified: true,
            name: "Default".to_string(),
            tiles: vec![
                Tile {
                    id: None,
                    priority: false,
                    collision: 0,
                    h_flippable: true,
                    v_flippable: true,
                    pixels: [[0; 8]; 8]
                };
                16
            ],
            ..Palette::default()
        };
        state.palettes.push(pal);
    }
}

pub fn get_initial_state() -> Result<EditorState> {
    let mut state = EditorState {
        global_config_path: get_global_config_path()?,
        global_config: GlobalConfig::default(),
        rom_path: None,
        palettes: vec![],
        dynamic_tiles: DynamicTiles::default(),
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
        dynamic_tiles_open: false,
        dynamic_tile_type: DynamicTileType::CutGrass,
        dynamic_tile_frames: vec![],
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
        hover_coords: None,
        selected_tile_block: TileBlock::default(),
        selected_gfx: vec![],
        show_grid_16: false,
        snap_grid_16: false,
        main_layer_idx: 0,
        side_layer_idx: 0,
        visible_layers: vec![],
        layer_drawer_open: false,
        pixel_coords: None,
        pixel_target: None,
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
    state.reset_layer_state(AreaPosition::Main);
    state.reset_layer_state(AreaPosition::Side);
    Ok(state)
}
