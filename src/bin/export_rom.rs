// This CLI is just a quick-and-dirty tool for testing. It will eventually
// be absorbed into the editor.

use std::hash::Hash;

use anyhow::{Context, Result};
use clap::Parser;

use hashbrown::{HashMap, HashSet};
use heuristic_graph_coloring::VecVecGraph;
use log::info;
use z3_overworld_editor::{
    persist::{self, load_area, load_project},
    state::{self, get_cloud_world, AreaId, EditorState, PaletteId, ThemeName},
};

#[derive(Parser, Debug)]
struct Args {
    #[arg(long)]
    theme: String,
}

type MapIdx = usize; // Vanilla map number of area

fn get_area_neighbors() -> Vec<(MapIdx, Vec<MapIdx>)> {
    vec![
        (0x00, vec![0x02, 0x10, 0x11, 0x80]),
        (0x02, vec![0x00, 0x0A]),
        (0x03, vec![0x05]),
        (0x05, vec![0x03, 0x07]),
        (0x07, vec![0x05]),
        (0x0A, vec![0x02, 0x12]),
        (0x0F, vec![0x81, 0x17]),
        (0x10, vec![0x00, 0x18]),
        (0x11, vec![0x00, 0x12, 0x18]),
        (0x12, vec![0x0A, 0x11, 0x13, 0x1A]),
        (0x13, vec![0x12, 0x14]),
        (0x14, vec![0x13, 0x15]),
        (0x15, vec![0x14, 0x16, 0x1D]),
        (0x16, vec![0x15, 0x17]),
        (0x17, vec![0x0F, 0x16]),
        (0x18, vec![0x10, 0x11, 0x22, 0x29]),
        (0x1A, vec![0x12, 0x1B]),
        (0x1B, vec![0x1A, 0x25, 0x2B, 0x2C]),
        (0x1D, vec![0x15, 0x25]),
        (0x1E, vec![0x2E, 0x2F]),
        (0x22, vec![0x18]),
        (0x25, vec![0x1B, 0x1D, 0x2D]),
        (0x28, vec![0x29]),
        (0x29, vec![0x18, 0x28, 0x2A]),
        (0x2A, vec![0x29, 0x32]),
        (0x2B, vec![0x1B, 0x2C, 0x33]),
        (0x2C, vec![0x1B, 0x2B, 0x2D, 0x34]),
        (0x2D, vec![0x25, 0x2C, 0x2E, 0x35]),
        (0x2E, vec![0x1E, 0x2D, 0x35]),
        (0x2F, vec![0x1E]),
        (0x30, vec![0x3A]),
        (0x32, vec![0x2A, 0x33]),
        (0x33, vec![0x2B, 0x32, 0x34, 0x3B]),
        (0x34, vec![0x2C, 0x33, 0x3C]),
        (0x35, vec![0x2D, 0x2E, 0x3C, 0x3F]),
        (0x37, vec![0x3F]),
        (0x3A, vec![0x30, 0x3B]),
        (0x3B, vec![0x33, 0x3A, 0x3C]),
        (0x3C, vec![0x34, 0x3B, 0x35]),
        (0x3F, vec![0x35, 0x37]),
        (0x40, vec![0x42, 0x50, 0x51]),
        (0x42, vec![0x40, 0x4A]),
        (0x43, vec![0x45]),
        (0x45, vec![0x43, 0x47]),
        (0x47, vec![0x45]),
        (0x4A, vec![0x42, 0x52]),
        (0x4F, vec![0x57]),
        (0x50, vec![0x40, 0x58]),
        (0x51, vec![0x40, 0x52, 0x58]),
        (0x52, vec![0x4A, 0x51, 0x53, 0x5A]),
        (0x53, vec![0x52, 0x54]),
        (0x54, vec![0x53, 0x55]),
        (0x55, vec![0x54, 0x56, 0x5D]),
        (0x56, vec![0x55, 0x57]),
        (0x57, vec![0x4F, 0x56]),
        (0x58, vec![0x50, 0x51, 0x62, 0x69]),
        (0x5A, vec![0x52]),
        (0x5B, vec![0x65, 0x6B, 0x6C]),
        (0x5D, vec![0x55, 0x65]),
        (0x5E, vec![0x6E, 0x6F]),
        (0x62, vec![0x58]),
        (0x65, vec![0x5B, 0x5D, 0x6D]),
        (0x68, vec![0x69]),
        (0x69, vec![0x58, 0x68, 0x6A]),
        (0x6A, vec![0x69, 0x72]),
        (0x6B, vec![0x5B, 0x6C, 0x73]),
        (0x6C, vec![0x5B, 0x6B, 0x6D, 0x74]),
        (0x6D, vec![0x65, 0x6C, 0x6E, 0x75]),
        (0x6E, vec![0x5E, 0x6D, 0x75]),
        (0x6F, vec![0x5E]),
        (0x70, vec![]),
        (0x72, vec![0x6A, 0x73]),
        (0x73, vec![0x6B, 0x72, 0x74, 0x7B]),
        (0x74, vec![0x6C, 0x73, 0x7C]),
        (0x75, vec![0x6D, 0x6E, 0x7C, 0x7F]),
        (0x77, vec![0x7F]),
        (0x7A, vec![0x7B]),
        (0x7B, vec![0x73, 0x7A, 0x7C]),
        (0x7C, vec![0x74, 0x7B, 0x75]),
        (0x7F, vec![0x75, 0x77]),
        (0x80, vec![0x00]),
        (0x81, vec![0x0F]),
    ]
}

fn get_area_palettes(
    state: &EditorState,
    theme: &ThemeName,
) -> Result<HashMap<MapIdx, Vec<PaletteId>>> {
    let mut out: HashMap<MapIdx, Vec<PaletteId>> = HashMap::new();
    for area_name in &state.area_names {
        if get_cloud_world(area_name).is_some() {
            continue;
        }
        let area_id = AreaId {
            area: area_name.clone(),
            theme: theme.clone(),
        };
        let area = load_area(state, &area_id)?;
        let Some(map_idx) = area.vanilla_map_id else {
            continue;
        };
        let palettes = area.get_unique_palettes();
        out.insert(map_idx as MapIdx, palettes);
    }
    Ok(out)
}

#[derive(Default, Clone)]
pub struct IndexedVec<T: Hash + Eq> {
    pub keys: Vec<T>,
    pub index_by_key: HashMap<T, usize>,
}

impl<T: Hash + Eq> IndexedVec<T> {
    pub fn add<U: ToOwned<Owned = T> + ?Sized>(&mut self, name: &U) -> usize {
        if !self.index_by_key.contains_key(&name.to_owned()) {
            let idx = self.keys.len();
            self.index_by_key.insert(name.to_owned(), self.keys.len());
            self.keys.push(name.to_owned());
            idx
        } else {
            self.index_by_key[&name.to_owned()]
        }
    }
}

pub fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("export_rom=info,z3_overworld_editor=info"),
    )
    .format_timestamp_millis()
    .init();

    let args = Args::parse();
    let theme = args.theme;
    let mut state = state::get_initial_state()?;
    persist::load_global_config(&mut state)?;
    load_project(&mut state)?;
    assert!(state.theme_names.contains(&theme));

    let area_palettes = get_area_palettes(&state, &theme)?;
    let area_neighbors = get_area_neighbors();
    let mut edges: HashSet<(PaletteId, PaletteId)> = HashSet::new();
    for (map_idx1, neighbors) in &area_neighbors {
        let mut pal_set: HashSet<PaletteId> = HashSet::new();
        for &pal1 in &area_palettes[map_idx1] {
            pal_set.insert(pal1);
            for &pal2 in &area_palettes[map_idx1] {
                edges.insert((pal1, pal2));
            }
        }
        for map_idx2 in neighbors {
            for &pal1 in area_palettes
                .get(map_idx1)
                .context(format!("missing map index {}", map_idx1))?
            {
                pal_set.insert(pal1);
                for &pal2 in area_palettes
                    .get(map_idx2)
                    .context(format!("missing map index {}", map_idx2))?
                {
                    edges.insert((pal1, pal2));
                }
            }
        }
        info!("{:x}: {}", map_idx1, pal_set.len());
    }

    let mut palette_ids = IndexedVec::default();
    for pal in area_palettes.values().flatten() {
        palette_ids.add(pal);
    }

    // let mut edges_by_src: HashMap<PaletteId, HashSet<PaletteId>> = HashMap::new();
    // for &pal in &palette_ids.keys {
    //     edges_by_src.insert(pal, HashSet::new());
    // }
    // for &(pal1, pal2) in &edges {
    //     edges_by_src.get_mut(&pal1).unwrap().insert(pal2);
    //     edges_by_src.get_mut(&pal2).unwrap().insert(pal1);
    // }
    // for (pal, neighbors) in &edges_by_src {
    //     info!("{} ({}): {:?}", pal, neighbors.len(), neighbors);
    // }

    let mut graph = VecVecGraph::new(palette_ids.keys.len());
    for &(pal1, pal2) in &edges {
        let v1 = palette_ids.index_by_key[&pal1];
        let v2 = palette_ids.index_by_key[&pal2];
        if v1 < v2 {
            graph.add_edge(v1, v2);
        }
    }

    info!(
        "{:?}",
        heuristic_graph_coloring::color_rlf(&graph).iter().max()
    );
    Ok(())
}
