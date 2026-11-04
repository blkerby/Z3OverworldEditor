# Overworld map artwork

## Goal and scope

Add editor support for creating overworld map artwork from scratch. Target a single, fixed, zoomed-out Mode 1 view with room-based terrain on BG1, screen-sized clouds on BG2, and pendant/crystal sprites. Defer ROM injection, runtime loading, marker implementation, and changes to the randomizer catalogs or patcher. Do not implement a zoomed-in view, zoom switching, or map scrolling.

Keep the existing organization where tiles belong to palettes and reuse the palette, pixel, tile-placement, selection, and undo tools. There is no requirement to reuse one graphics tile across multiple palettes.

## Assets and budgets

Every area belongs to exactly one world, identified by an area-level `world` field with value `"light"`, `"dark"`, or `"special"`. Light-world and dark-world areas use their corresponding map palette groups; world membership is not selected independently for individual layers. Special-world areas have no map artwork, and there is no special-world map palette group or cloud tilemap.

Maintain separate light-world and dark-world map assets. For each world, support:

- At most eight map palettes, shared between terrain and clouds, with sixteen colors per palette.
- At most 1,024 terrain graphics tiles, summed across the world's map palettes.
- Initially budget 256 cloud graphics tiles, also stored within palettes.
- A 32×28 cloud tilemap for the visible region, padded to 32×32 by the future ROM builder.
- Room map artwork attached to the existing area/theme and layer structure.

Mark map palettes as a special subset of the existing palettes. Keep them selectable through the existing Palette selector alongside gameplay palettes. Show the map world's palette count out of eight, terrain tile count out of 1,024, and cloud tile count out of 256. Keep existing gameplay artwork separate from these budgets.

Give each map tile a single intended-use field: Terrain or Clouds. Count every tile toward its designated budget, including unplaced tiles. These are allocation budgets calculated from tileset data alone; counting does not scan room/layer maps or cloud tilemaps. A palette may contain tiles of both kinds and still counts only once toward the shared eight-palette limit. Intended use is separate from tile priority.

Allow painting and pasting tiles onto either kind of map. Highlight placements whose tile designation does not match the map, using the existing bad-flip error presentation. Check only maps being displayed; do not block placement, remove mismatched tiles, or perform a global scan. Changing a tile's designation updates the budgets and displayed errors without modifying its placements. Mismatches in unopened maps remain undiscovered until those maps are displayed or checked by a future build.

Use the existing tile palette ownership, flip, and priority fields. Pixel index zero remains transparent; the preview must also represent the world's backdrop color. Palette colors must retain the editor's SNES color representation.

## Room map editing

Each existing room layer in a light-world or dark-world area can contain corresponding map tile edits. Size the map grid as three tiles per room screen in each dimension: 3×3 tiles for a small room and 6×6 for a large room. Each graphics tile is 8×8 pixels, giving artwork dimensions of 24×24 and 48×48 pixels respectively.

Use the existing layer conditions, ordering, visibility controls, and edge-variant preview to select and compose these edits. All selected room map edits compose into the single map background, regardless of the gameplay background targeted by their owning layers. Do not introduce separate map-specific edge conditions or a second layer stack.

An absent placement leaves the lower map layer unchanged. A present placement replaces the whole map cell, including when it selects a blank tile. Its transparent pixels expose the background during final rendering; they do not recover a replaced lower-layer placement.

Edge variants can replace map placements with references to different graphics tiles. Graphics edits remain shared by every placement referencing that tile; a room-specific graphics variation uses a separate tile slot.

Reuse brush, erase, selection, copy/paste, flip, and undo behavior. Adding, deleting, reordering, or renaming an existing layer must preserve the association with its map edits. Existing projects without map artwork should remain editable without requiring artwork for every room first.

## Clouds and priority preview

Provide a cloud editor for each of the light and dark worlds, using its cloud graphics and map palettes. Display and allow editing only the 256×224 visible region, corresponding to the first 32×28 tiles of the 32×32 hardware tilemap. The bottom four tile rows are outside the editable canvas. Apply these bounds to painting, selection, and pasting.

Place the cloud tilemap at screen position (0, 0). Center a fixed 192×192 terrain footprint in the viewport with its upper-left corner at (32, 16), corresponding to an 8×8 grid of small-room units. Draw its outline in a contrasting color such as red underneath the cloud pixels, so opaque clouds cover the outline and transparent holes reveal it. The outline is an editor guide, not saved artwork.

Clouds use opaque colors with transparent holes: pixel index zero is transparent and every other pixel is opaque. Do not add color math or translucency controls.

Use the terrain tile's existing priority field to control whether it appears above the clouds. Render BG2 clouds at high priority, producing this front-to-back order with BG3 disabled:

1. BG1 terrain tiles with priority enabled, such as mountain peaks.
2. BG2 cloud tiles with high priority.
3. BG1 terrain tiles with priority disabled.
4. The backdrop through transparent pixels.

Future pendant/crystal sprites can use sprite priority 3 to appear above these backgrounds. Preserve the existing meaning of priority on gameplay artwork; only the map preview uses this map-specific composition.

Preview only the selected room's composed map layers, responding to the existing layer and edge-condition controls. Do not add a neighboring-room arrangement or a complete reference-world preview. The cloud editor uses the terrain-footprint outline rather than assembled room artwork; generated-seed integration is deferred.

## User interface

### Room map in the Layers panel

Keep the normal room artwork in the main area canvas. Insert the selected room's map editing canvas into the existing Layers panel below the layer list and above the layer name, background, condition, and other properties. Do not add a toolbar view selector or a separate room-map editing mode.

Show the composed visible map layers in this small canvas, with painting and other edits applied to the selected layer, matching the main area's editing behavior. Layer selection, visibility, and condition previews update both canvases. Enlarge the map pixels enough to edit comfortably while fitting the 3×3 or 6×6 grid within the panel. Omit the map canvas for special-world areas.

### Palettes and properties

Use the existing Palette selector for both gameplay and map palettes without automatic context filtering. Extend the palette editing modal, which currently supports rename and delete, with `Map world: None / Light / Dark`; this edits `map_world`, with None meaning an ordinary gameplay palette.

Keep the existing color, tileset, and pixel editors. For map tiles, expose `Map use: Terrain / Clouds` alongside the existing priority and flip constraints. Show the relevant world's allocation counts in the palette panel and update them as palette membership or tile designations change. Reuse the existing error presentation for mismatched placements.

Add `World: Light / Dark / Special` to the existing area creation and editing dialogs.

### Tools menu

Add one `Tools` menu button to the toolbar. Normally only the button is visible; clicking it reveals the less-frequently used actions:

- `Clouds…`: open cloud editing in the main area canvas.
- `Conditions`: toggle the existing Conditions panel, replacing its standalone toolbar button.
- `Dynamic tiles`: toggle the existing dynamic-tile editor, moving its entry out of Project Settings.

Indicate open Conditions and Dynamic tiles panels with checkmarks. Close the menu after choosing an action, clicking outside, or pressing Escape. Keep Layers directly accessible on the toolbar.

### Cloud editing in the main canvas

Opening Clouds replaces the main area canvas with the 256×224 cloud editor. Replace the area/theme controls with `World: Light / Dark`, backdrop-color editing using the existing RGB controls, and `Back to area`. Keep the palette, tileset, and pixel editing controls available.

Hide the Layers and Conditions panels while editing clouds. Returning to area editing restores the selected area, theme, layer, and prior panel state. The world selector is local to cloud editing; there is no general Area/Map/Clouds view selector.

## Persistence

Extend the existing project save/load and undo paths to cover map palette classification, world association, tile intended use, cloud maps, and per-layer room map edits. Reuse existing tile placement and palette serialization where practical. Saving unrelated gameplay artwork must not discard map artwork, and palette/tile edits must update the map previews through the existing dirty-state behavior.

### Palette and tile fields

Keep palettes in `Palettes/<name>.json`. Add `map_world` with value `"light"` or `"dark"` to map palettes; omit it for ordinary gameplay palettes. Preserve existing palette IDs, colors, and tile collections.

Add `map_use` with value `"terrain"` or `"clouds"` to each map tile's existing object; omit it for ordinary gameplay tiles. Preserve existing tile IDs, priority, flip constraints, and pixel data. Derive palette and tile budgets from these fields without serializing counts or usage indexes.

### Area and layer fields

Keep area metadata in `Areas/<area>/<theme>.json`. Serialize the required `world` field immediately before `other_world_area`, with value `"light"`, `"dark"`, or `"special"`. Store it in each theme file alongside the existing area metadata, and populate it for existing areas when introducing the field.

Add an optional `map` object to each existing layer, using parallel `palettes`, `tiles`, and `flips` grids as in existing area serialization. For example, a small room's layer can contain:

```json
"map": {
  "palettes": [
    [120, 120, 120],
    [120, null, 120],
    [120, 120, 120]
  ],
  "tiles": [
    [0, 1, 2],
    [3, null, 4],
    [5, 6, 7]
  ],
  "flips": [
    [0, 0, 0],
    [0, null, 0],
    [0, 0, 0]
  ]
}
```

Palette references are editor palette IDs, not hardware palette-row numbers. Tile references are indices within their owning palette, and flips use the existing encoding. An absent placement has `null` in all three grids; a placed blank tile has ordinary numeric references. Dimensions come from the area's size, giving 3×3 or 6×6 grids. Store full grids without position/size fields or cropped fragments. Omit `map` when the layer has no map edits. Conditions and ordering remain on the owning layer rather than being duplicated inside `map`. Special-world areas have no layer map artwork.

### World map files

Add `OverworldMap/light.json` and `OverworldMap/dark.json`, each containing:

- `bg_color`: the world-map backdrop in the existing RGB representation.
- `clouds`: parallel `palettes`, `tiles`, and `flips` grids using the same placement format as layer `map` objects.

Store exactly 28 rows of 32 cells in each cloud grid, matching the 256×224 editable region. The future ROM builder pads this to the hardware's 32×32 tilemap; do not store four inaccessible rows in the editor files. The fixed viewport offsets and terrain-footprint outline are editor conventions, not serialized artwork or configuration.

Do not redesign existing gameplay palettes or add a separate general-purpose asset system.

## Future VRAM layout

Load only the selected world's assets. Light and dark worlds reuse the same addresses. The following layout reserves the full graphics capacities while leaving 12 KiB unused; palette data resides separately in CGRAM.

| Purpose | VRAM byte addresses | VRAM word addresses | Size |
|---|---|---|---:|
| BG1 terrain graphics, 1,024 tiles | `$0000–$7FFF` | `$0000–$3FFF` | 32 KiB |
| BG2 cloud graphics, 256 tiles | `$8000–$9FFF` | `$4000–$4FFF` | 8 KiB |
| BG1 terrain tilemap, 32×32 | `$A000–$A7FF` | `$5000–$53FF` | 2 KiB |
| BG2 cloud tilemap, 32×32 | `$A800–$AFFF` | `$5400–$57FF` | 2 KiB |
| Unused | `$B000–$BFFF` | `$5800–$5FFF` | 4 KiB |
| Sprite graphics, up to 256 tiles | `$C000–$DFFF` | `$6000–$6FFF` | 8 KiB |
| Unused | `$E000–$FFFF` | `$7000–$7FFF` | 8 KiB |

The future ROM builder can pack tiles designated Terrain into the BG1 graphics region and tiles designated Clouds into the BG2 graphics region, translating editor references to hardware tile indices. A palette containing both kinds uses one background palette row. The builder can check placement designations when it reads the complete map data. Background palette rows occupy CGRAM entries `$00–$7F`; sprite palettes occupy `$80–$FF`. These runtime assignments are planning constraints, not editor implementation work.

The layout and priority ordering follow the [SNES background documentation](https://wiki.superfamicom.org/backgrounds) and [register documentation](https://wiki.superfamicom.org/registers).

## Implementation sequence

1. Add the area `world` field, populate existing areas, and add persistence for the map assets and per-layer map edits without modifying randomizer catalogs or ROM code.
2. Extend the palette editing modal with map-world selection, keep map palettes in the existing Palette selector, and add tile intended-use editing and allocation budget counts. Reuse the existing palette and graphics editing functionality, including tile priority, and expose world selection in the area dialogs.
3. Add the selected room's map canvas below the Layers panel's layer list, tied to the selected layer and its condition preview, with normal selection and undo operations, intended-use mismatch highlighting, and composed visible layers.
4. Add the Tools menu, move Conditions and Dynamic tiles into it, and open cloud editing from it in the main canvas. Add the two cloud tilemaps with mismatch highlighting, a 256×224 editable canvas, and the terrain-footprint outline underneath opaque cloud pixels. Restore the area editing context when leaving clouds.
5. Verify save/reload, undo, palette/tile edits, allocation counts, mismatch highlighting after painting/pasting and reclassification, small/large room dimensions, selected-room layer composition, edge variants, cloud editing bounds, outline visibility through transparent cloud pixels, menu behavior, and return-to-area state on a copied project. Run the editor's applicable build checks. Remove any temporary verification code before completion.
