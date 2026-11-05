# Overworld map artwork

## Goal and scope

Add editor support for creating overworld map artwork at vanilla's scale, with imported vanilla graphics and tilemaps as starting material. Leave the runtime choice between Mode 7 and Mode 1 undecided. A Mode 1 implementation would allow scrolling to see the map and may crop the authored 512×512 extent to 384×384 or smaller; the crop and viewport are not yet decided. Defer ROM injection, runtime loading, marker implementation, and changes to the randomizer catalogs or patcher. A separate zoomed-in view and zoom toggle are not currently planned.

Keep the existing organization where tiles belong to palettes and reuse the palette, pixel, tile-placement, selection, and undo tools. There is no requirement to reuse one graphics tile across multiple palettes.

## Assets and tile counts

Every area belongs to exactly one world, identified by an area-level `world` field with value `"light"`, `"dark"`, or `"special"`. World membership is not selected independently for individual layers. Special-world areas do not use room map artwork and have no cloud area.

Keep world membership on areas, not on palettes or tiles. For each world, support:

- Existing palettes with sixteen colors each, shared freely between gameplay, terrain, clouds, and worlds. Show the number actually used out of eight as a reference, without an ownership-based cap.
- Palette-owned graphics tiles with no determined tile budget. Do not display or enforce a terrain, cloud, or combined tile cap.
- A shared 512×512 cloud tilemap, stored as a 64×64 grid of 8×8 tiles in a dedicated Base-only area.
- Room map artwork attached to the existing area/theme and layer structure.

Do not classify palettes by map world or tiles by intended use. The area containing a placement determines its world and whether it is room-map or cloud artwork. Palettes remain selectable through the existing Palette selector, with no placement restrictions or designation-mismatch errors. Unplaced tiles and ordinary gameplay artwork do not contribute to map usage. A palette used in both worlds counts independently in each.

Use the existing tile palette ownership, flip, and priority fields. Pixel index zero remains transparent; the preview must also represent the world's backdrop color. Palette colors must retain the editor's SNES color representation.

### On-demand world tile count

Provide one `Count map tiles` action that reports both worlds, independent of the selected palette. Calculate under a Mode 7 assumption, regardless of the eventual runtime mode: terrain and clouds share one tile pool within a world, and different placed flips count as separate tiles. Use `(palette ID, tile index, flip)` as the counting identity. Repeated placements of the same identity count once; priority and collision metadata do not create additional identities. Do not merge different flip variants or distinct tile references by comparing pixel data.

Scan each world's room-level `map` grids across all areas, themes, and layers, plus the ordinary layers of that world's Base cloud area. Include conditional and edge-variant layers, regardless of the currently selected preview or layer visibility. Use current in-memory edits for loaded areas and saved data for the others. Group a room by its area identity across themes, not by individual theme files. Exclude ordinary area artwork, including the imported `Light World Map` and `Dark World Map` reference canvases; their room-level `map` fields remain absent.

For each world:

1. Build the distinct tile set for every room/theme, taking the union across its map layers, and the distinct tile set used by the shared Base cloud area.
2. Classify as general any tile used in more than one room across all themes, or used by the cloud area. Count the union of these general tiles once, so tiles shared by rooms and clouds are not double-counted.
3. All remaining used tiles occur in exactly one room and are room-specific. For each room/theme, count its distinct room-specific tiles. Take the maximum of those counts across that room's themes, then sum these maxima across the world's rooms.
4. Report `world tile count = general tiles + sum of per-room maximum room-specific tiles`. Show the general contribution and the room-specific contribution alongside the total, without a cap or percentage. Also report the number of distinct palette IDs referenced across all scanned themes and clouds, out of eight.

For example, if there are 20 general tiles including clouds, one room needs 3 or 5 room-specific tiles in its themes, and another needs 2 or 4, the world count is `20 + max(3, 5) + max(2, 4) = 29`. Tiles reused across themes of the same room remain room-specific unless another room or the cloud area also uses them. This assumes one theme per room in a generated world; general tiles are conservatively retained across all themes.

Calculate only on explicit request, not continuously or on save. Label the displayed result as the last calculated count; users recalculate after edits. Do not serialize counts or maintain a persistent usage index. This is an authored-tile usage estimate, not a guarantee of final VRAM usage: runtime packing and any generated composite graphics remain undecided.

## Room map editing

Each existing room layer in a light-world or dark-world area can contain corresponding map tile edits. Use a 4×4 grid of 8×8 tiles for a small room and an 8×8 grid for a large room, matching vanilla's scale. The artwork dimensions are 32×32 and 64×64 pixels respectively. Existing area sizes count 256-pixel screens, so multiply each stored area dimension by two to obtain its room-map tile dimension.

Use the existing layer conditions, ordering, visibility controls, and edge-variant preview to select and compose these edits. All selected room map edits compose into the single map background, regardless of the gameplay background targeted by their owning layers. Do not introduce separate map-specific edge conditions or a second layer stack.

An absent placement leaves the lower map layer unchanged. A present placement replaces the whole map cell, including when it selects a blank tile. Its transparent pixels expose the background during final rendering; they do not recover a replaced lower-layer placement.

Edge variants can replace map placements with references to different graphics tiles. Graphics edits remain shared by every placement referencing that tile; a room-specific graphics variation uses a separate tile slot.

Reuse brush, erase, selection, copy/paste, flip, and undo behavior. Adding, deleting, reordering, or renaming an existing layer must preserve the association with its map edits. Existing projects without map artwork should remain editable without requiring artwork for every room first.

## Cloud areas and priority preview

Store cloud artwork in dedicated ordinary areas named `Light World Clouds` and `Dark World Clouds`. These are special-purpose canvases with `world: "light"` or `"dark"`, not `world: "special"`, and no `vanilla_map_id`. Each has only a `Base` theme, shared by all room themes in its world. Use `size: [2, 2]` with an ordinary BG2 layer named `Main`, giving a fully editable 512×512 canvas of 64×64 tiles. Leave room-level `map` fields absent. Additional ordinary cloud layers, if present, use the existing layer tools and are included in the shared cloud tile set.

Assume the two cloud area names and the Base theme remain unchanged; do not add rename protection or another identifier. The normal Add Area action recognizes those names and creates an empty 512×512 Base-only cloud area with the corresponding world.

Completed: empty Base-only cloud areas and their layer PNGs have been added to `~/ALTTPRetiling`. The existing project had no room-map artwork or legacy cloud files to migrate. Save/reload, 4×4 and 8×8 room maps, cloud painting/undo, theme operations, tile scans/remapping, PNG export, and a non-Base ROM import with more than 1,024 terrain tiles were verified on copied projects; temporary tests were removed.

Edit clouds through the normal area selector and main canvas, reusing the Layers panel, palette selector, painting, selection, copy/paste, undo, and area backdrop-color controls. Do not add a cloud-editing mode, world selector, separate backdrop editor, or mode-specific return navigation. The theme selector and project-wide operations, including ROM import, theme creation/copy, tile remapping, and PNG export, must support the cloud areas having only Base; do not create or synchronize copies in other themes.

Use the full authored canvas for editing even if a future runtime crops the map.

Clouds use opaque colors with transparent holes: pixel index zero is transparent and every other pixel is opaque. Do not add color math or translucency controls.

Preserve the desired preview ordering using the terrain tile's existing priority field: high-priority terrain above clouds, clouds above ordinary terrain, and the backdrop visible through transparent pixels. Keep this composition rule independent of the runtime mode. BG1/BG2 assignments, sprite priorities, and how Mode 7 would compose terrain with clouds remain runtime design decisions. Preserve the existing meaning of priority on gameplay artwork.

Preview only the selected room's composed map layers, responding to existing layer and edge-condition controls. Do not add a neighboring-room arrangement or a generated-world preview. Clouds use the ordinary area preview; generated-seed integration is deferred. The imported reference areas remain ordinary editable canvases.

## User interface

### Room map in the Layers panel

Keep the normal room artwork in the main area canvas. Insert the selected room's map editing canvas into the existing Layers panel below the layer list and above the layer name, background, condition, and other properties. Do not add a toolbar view selector or a separate room-map editing mode.

Show the composed visible map layers in this small canvas, with painting and other edits applied to the selected layer, matching the main area's editing behavior. Layer selection, visibility, and condition previews update both canvases. Use the same pixel scale for the 4×4 and 8×8 grids, sized to fit the large room within the panel; small-room artwork is half the width and height. Omit the room-map canvas for special-world areas and the dedicated cloud areas.

### Palettes and properties

Keep the existing Palette selector, palette rename/delete modal, color editor, tileset editor, and tile properties unchanged. No palette world or tile intended-use controls are needed.

Add a `Count map tiles` action inside Project Settings. Display both worlds' last calculated tile counts, general/room-specific contributions, and used palette counts out of eight below the button in Project Settings. Do not show a tile limit or recalculate automatically after edits.

The area creation and editing dialogs provide `World: Light / Dark / Special`. New areas default to the current area's world; edits apply across all themes and support undo. Changing the world preserves existing artwork; Special excludes room maps from usage and the planned map canvas. The canonical cloud areas display their name-derived world as fixed text.

### Tools menu

Add one `Tools` menu button to the toolbar. Normally only the button is visible; clicking it reveals the less-frequently used actions:

- `Conditions`: toggle the existing Conditions panel, replacing its standalone toolbar button.
- `Dynamic tiles`: toggle the existing dynamic-tile editor, moving its entry out of Project Settings.

Indicate open Conditions and Dynamic tiles panels with checkmarks. Close the menu after choosing an action, clicking outside, or pressing Escape. Keep Layers directly accessible on the toolbar.

## Persistence

Extend the existing project save/load and undo paths to cover area world association and per-layer room map edits. Cloud artwork uses ordinary area-layer persistence and undo. Reuse existing tile placement and palette serialization where practical. Saving unrelated gameplay artwork must not discard map artwork, and palette/tile edits must update the map previews through the existing dirty-state behavior.

### Palette and tile fields

Keep palettes in `Palettes/<name>.json`, preserving their IDs, colors, and tile collections. Preserve tile IDs, priority, flip constraints, and pixel data. Remove the previously introduced `map_world` and `map_use` fields; no replacement classification is needed. Derive palette and tile usage on demand from placements using the counting rules above, without serializing counts or usage indexes.

### Area and layer fields

Keep area metadata in `Areas/<area>/<theme>.json`. Serialize the required `world` field immediately before `other_world_area`, with value `"light"`, `"dark"`, or `"special"`. Store it in each theme file alongside the existing area metadata, and populate it for existing areas when introducing the field.

Add an optional `map` object to each existing layer, using parallel `palettes`, `tiles`, and `flips` grids as in existing area serialization. For example, a small room's layer can contain:

```json
"map": {
  "palettes": [
    [120, 120, 120, 120],
    [120, null, 120, 120],
    [120, 120, 120, 120],
    [120, 120, 120, 120]
  ],
  "tiles": [
    [0, 1, 2, 3],
    [4, null, 5, 6],
    [7, 8, 9, 10],
    [11, 12, 13, 14]
  ],
  "flips": [
    [0, 0, 0, 0],
    [0, null, 0, 0],
    [0, 0, 0, 0],
    [0, 0, 0, 0]
  ]
}
```

Palette references are editor palette IDs, not hardware palette-row numbers. Tile references are indices within their owning palette, and flips use the existing encoding. An absent placement has `null` in all three grids; a placed blank tile has ordinary numeric references. Existing area `size` values count 256-pixel screens: a small room is `[2, 2]` and a large room is `[4, 4]`. Multiply each dimension by two to obtain the 4×4 or 8×8 map grid. Store full grids without position/size fields or cropped fragments. Omit `map` when the layer has no map edits. Conditions and ordering remain on the owning layer rather than being duplicated inside `map`. Room map artwork retained in special-world areas is inactive.

### Cloud area files

Store the cloud tilemaps in `Areas/Light World Clouds/Base.json` and `Areas/Dark World Clouds/Base.json`, using the existing area `bg_color` and ordinary layer `screens`/`palettes`/`tiles`/`flips` serialization. Their 64×64 grids cover the entire 512×512 artwork; there are no inaccessible rows or hardware padding in the authored data. Their ordinary layer PNGs use the existing export paths. The canonical cloud areas are the source of cloud placements for counting and eventual runtime generation.

Retire the separate `OverworldMap/light.json` and `OverworldMap/dark.json` design and its previously implemented cloud state/persistence paths. Cloud editing, saving, undo, tile remapping, and usage scans use ordinary area layers. Do not redesign gameplay palettes or introduce a general-purpose asset system.

## Vanilla map graphics import

As a follow-up to the persistence foundation in step 1, extend the ROM importer to extract the vanilla Mode 7 overworld map palettes and graphics and convert them into the editor's palette-owned 4bpp map tiles. Also import these assets into the existing project at `~/ALTTPRetiling`. Import palettes, their corresponding graphics tiles, and the reference areas described below; defer populating per-room map artwork and the dedicated cloud areas.

Determine and pack each world's vanilla graphics using that world's colors. Reuse any existing palette with matching opaque colors, including palettes also used in gameplay or the other world. Preserve pixel colors and transparency during conversion; the assumption that each tile can be converted losslessly must be checked before importing.

For each world, collect each tile's distinct opaque colors and group tiles into the minimum number of 4bpp palettes. Each palette can contain at most fifteen opaque colors plus transparent index zero. Tiles may share a palette when the union of their opaque colors fits within those fifteen entries. Remap each tile's pixels to its assigned palette while preserving the 8×8 artwork. A tile requiring more than fifteen opaque colors, or a world requiring more than eight palettes, needs a design decision rather than silent color reduction or omitted artwork.

Report the resulting palette and tile counts for each world, and verify the converted graphics against the source pixels. Add the converted palettes and tiles to `ALTTPRetiling` using unused palette IDs and distinct names, preserving existing artwork. This import task is separate from the initial persistence implementation and does not require the later map-editing UI.

Completed: the importer supports the Japanese and US vanilla layouts, and the assets have been added to `~/ALTTPRetiling` without changing existing files. Exact palette packing produces six palettes for 223 Light World tiles and six palettes for 208 Dark World tiles; five palettes cannot represent either world's artwork losslessly. Include the shared outer border, which the Dark World retains from the Light World tilemap, using each world's colors. Imported palette names are `Light World Map 1` through `6` and `Dark World Map 1` through `6`.

Follow the existing importer's matching behavior: reuse palettes with matching opaque colors and matching tiles, append missing artwork, and preserve edited assets. The vanilla conversion must fit within eight palettes per source world; unused existing palettes do not count against this. Preserve existing palettes on conversion failure. Remove the importer's existing terrain tile-cap check; tile usage has no enforced cap while the runtime design is undecided. Pixel comparisons, JP/US equivalence, repeat imports, preservation of edits, save/reload, and the full ROM-import workflow have been verified on copied projects; temporary verification code was removed.

Import the assembled vanilla tilemaps into two ordinary reference/template areas named `Light World Map` and `Dark World Map`, with the corresponding `world`, no `vanilla_map_id`, and reciprocal `other_world_area` links. Each uses `size: [2, 2]` and one ordinary BG2 layer named `Main`, containing the 64×64-tile map including its outer border. The Dark World replaces the central 32×32 tiles of the Light World layout and retains the border. Placements reference the imported palettes and tiles without duplicating graphics. Leave the layer's room-level `map` field absent. Create missing theme files for both areas, matching normal area creation; subsequent ROM imports replace the selected theme's reference maps while preserving existing reference maps in other themes. These canvases preserve vanilla's scale for editing and copying into the planned 4×4 and 8×8 room maps. They are reference artwork and do not contribute their ordinary layer placements to the world usage count.

Completed: both reference areas have been added to all five themes in `~/ALTTPRetiling`, with their layer PNGs and reciprocal world links. Saved placements and PNGs were checked pixel-for-pixel against the ROM, along with save/reload, JP/US imports, reuse or creation of map palettes, preservation of other themes, and the full ROM-import workflow.

## Runtime mode and VRAM layout

Mode 7 versus Mode 1, the final tile budget, tilemap packing, and the VRAM layout remain undecided. The previous fixed Mode 1 layout and separate terrain/cloud tile caps are superseded. Keep the editor's palette-owned assets and vanilla-scale artwork independent of that decision.

Use Mode 7 assumptions only for the on-demand usage estimate: flips occupy separate counted tiles, and clouds and terrain share a world tile pool. If Mode 1 is chosen, support scrolling and decide whether to crop the 512×512 authored map to 384×384 or smaller. Cropping must not reduce the editor's stored cloud canvas. Defer renderer-specific tile deduplication, generated composite graphics, hardware layer assignment, and sprite placement until runtime work is authorized.

## Implementation sequence

1. Completed foundation: area `world` metadata, palette and room-map persistence, vanilla graphics import, and paired reference-map areas. Room-map dimensions now use `area.size * 2`; Base-only ordinary cloud areas replace the separate cloud-map state/files; the importer's terrain tile cap is removed. Theme selectors, theme operations, tile scans/remapping, PNG export, and ROM import handle Base-only clouds. Keep runtime and randomizer changes deferred.
2. Completed: remove palette world and tile intended-use classifications, and add one on-demand usage report for both worlds with general/room-specific contributions and used palette counts out of eight. Show no tile cap. Area creation and editing dialogs now expose world selection, with edits applied across themes and undo support.
3. Completed: the selected room's 4×4 or 8×8 map canvas appears below the Layers panel's layer list. It uses the selected layer, composes visible map edits independently of gameplay background settings, and shares painting, erasing, selection, copy/paste, flips, and undo with the existing canvases. Special-world and cloud areas omit the canvas. Save/reload, layer operations, edge-condition previews, blank/transparent replacement, clipping, and undo were checked with temporary fixtures; both canvas sizes were visually checked using headless rendering. Temporary verification code was removed.
4. Base-only cloud canvases already reuse the ordinary area tools and participate in project-wide operations. Their room-map canvases are omitted. No new cloud editor mode is needed. Add the Tools menu for Conditions and Dynamic tiles as already planned.
5. Verify save/reload, undo, palette/tile edits, 4×4 and 8×8 map persistence, layer/edge-variant composition, and cloud-area editing on a copied project. Verify counting with flipped placements, repeated placements, conditional layers, tiles shared across rooms or with clouds, room-specific theme maxima, unused tiles, excluded reference artwork, Base-only clouds, and unsaved edits. Check that recounting is explicit and no tile cap is shown or enforced. Run applicable build checks and remove temporary verification code before completion.
