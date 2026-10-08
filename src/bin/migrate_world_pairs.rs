use std::{collections::HashMap, fs, path::PathBuf};

use anyhow::Result;
use clap::Parser;
use serde_json::Value;

#[derive(Parser)]
struct Args {
    project_dir: PathBuf,
}

fn main() -> Result<()> {
    let args = Args::parse();
    let mut paths = Vec::new();
    for area_entry in fs::read_dir(args.project_dir.join("Areas"))? {
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

    let mut names = HashMap::new();
    let mut areas = Vec::new();
    for path in paths {
        let data = fs::read_to_string(&path)?;
        let value: Value = serde_json::from_str(&data)?;
        let theme = path.file_stem().unwrap().to_str().unwrap().to_owned();
        let name = path
            .parent()
            .unwrap()
            .file_name()
            .unwrap()
            .to_str()
            .unwrap();
        let map_id = value["vanilla_map_id"].as_u64();
        if let Some(map_id) = map_id {
            names.insert((theme.clone(), map_id), name.to_owned());
        }
        let has_pair = value.get("other_world_area").is_some();
        areas.push((path, data, theme, map_id, has_pair));
    }

    let mut updated = 0;
    for (path, mut data, theme, map_id, has_pair) in areas {
        if has_pair {
            continue;
        }
        let Some(map_id) = map_id else {
            continue;
        };
        if map_id >= 0x80 {
            continue;
        }
        let Some(name) = names.get(&(theme, map_id ^ 0x40)) else {
            continue;
        };
        // Insert only the new field, preserving all existing data and formatting.
        let position = data.find('{').unwrap() + 1;
        data.insert_str(
            position,
            &format!(
                "\n  \"other_world_area\": {},",
                serde_json::to_string(name)?
            ),
        );
        let temp_path = path.with_extension("json.tmp");
        fs::write(&temp_path, data)?;
        fs::rename(&temp_path, &path)?;
        updated += 1;
    }

    println!("Updated {updated} area files.");
    Ok(())
}
