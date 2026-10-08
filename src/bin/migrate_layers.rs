use std::{collections::HashSet, fs, path::PathBuf};

use anyhow::{bail, Context, Result};
use clap::Parser;
use json_pretty_compact::PrettyCompactFormatter;
use serde::{Deserialize, Serialize};
use serde_json::{Serializer, Value};

#[derive(Parser)]
struct Args {
    project_dir: PathBuf,
}

#[derive(Deserialize)]
struct LegacyArea {
    other_world_area: Option<String>,
    vanilla_map_id: Option<u8>,
    bg_color: [u8; 3],
    size: (u8, u8),
    screens: Vec<LegacyScreen>,
}

#[derive(Deserialize)]
struct LegacyScreen {
    position: (u8, u8),
    palettes: Vec<Vec<u16>>,
    tiles: Vec<Vec<u16>>,
    flips: Vec<Vec<u8>>,
}

#[derive(Serialize)]
struct Area {
    #[serde(skip_serializing_if = "Option::is_none")]
    other_world_area: Option<String>,
    vanilla_map_id: Option<u8>,
    bg_color: [u8; 3],
    size: (u8, u8),
    layers: Vec<Layer>,
}

#[derive(Serialize)]
struct Layer {
    name: &'static str,
    background: &'static str,
    screens: Vec<Screen>,
}

#[derive(Serialize)]
struct Screen {
    position: (u16, u16),
    size: (u16, u16),
    palettes: Vec<Vec<u16>>,
    tiles: Vec<Vec<u16>>,
    flips: Vec<Vec<u8>>,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let area_dir = args.project_dir.join("Areas");
    let mut paths = Vec::new();
    for area_entry in fs::read_dir(&area_dir)? {
        let area_path = area_entry?.path();
        if !area_path.is_dir() {
            continue;
        }
        for theme_entry in fs::read_dir(area_path)? {
            let path = theme_entry?.path();
            if path.extension().and_then(|extension| extension.to_str()) == Some("json") {
                paths.push(path);
            }
        }
    }
    paths.sort();

    let mut converted = 0;
    for path in paths {
        let bytes = fs::read(&path)?;
        let value: Value = serde_json::from_slice(&bytes)
            .with_context(|| format!("failed to parse {}", path.display()))?;
        let object = value
            .as_object()
            .with_context(|| format!("area is not an object: {}", path.display()))?;
        let has_layers = object.contains_key("layers");
        let has_screens = object.contains_key("screens");
        if has_layers && !has_screens {
            continue;
        }
        if has_layers || !has_screens {
            bail!(
                "expected exactly one of screens or layers: {}",
                path.display()
            );
        }

        let legacy: LegacyArea = serde_json::from_slice(&bytes)
            .with_context(|| format!("invalid legacy area: {}", path.display()))?;
        let mut positions = HashSet::new();
        for screen in &legacy.screens {
            if screen.position.0 >= legacy.size.0
                || screen.position.1 >= legacy.size.1
                || !positions.insert(screen.position)
                || screen.palettes.len() != 32
                || screen.tiles.len() != 32
                || screen.flips.len() != 32
                || screen.palettes.iter().any(|row| row.len() != 32)
                || screen.tiles.iter().any(|row| row.len() != 32)
                || screen.flips.iter().any(|row| row.len() != 32)
            {
                bail!("invalid legacy screen: {}", path.display());
            }
        }
        if positions.len() != legacy.size.0 as usize * legacy.size.1 as usize {
            bail!("incomplete legacy area: {}", path.display());
        }
        let mut screens = Vec::with_capacity(legacy.screens.len());
        for screen in legacy.screens {
            screens.push(Screen {
                position: (screen.position.0 as u16 * 32, screen.position.1 as u16 * 32),
                size: (32, 32),
                palettes: screen.palettes,
                tiles: screen.tiles,
                flips: screen.flips,
            });
        }
        let area = Area {
            other_world_area: legacy.other_world_area,
            vanilla_map_id: legacy.vanilla_map_id,
            bg_color: legacy.bg_color,
            size: legacy.size,
            layers: vec![Layer {
                name: "Main",
                background: "bg2",
                screens,
            }],
        };

        let mut output = Vec::new();
        let formatter = PrettyCompactFormatter::new().with_max_line_length(200);
        let mut serializer = Serializer::with_formatter(&mut output, formatter);
        area.serialize(&mut serializer)?;
        serde_json::from_slice::<Value>(&output)?;

        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, output)?;
        fs::rename(&temp_path, &path)?;
        converted += 1;
    }

    println!("Converted {converted} area files.");
    Ok(())
}
