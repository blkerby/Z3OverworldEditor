# Area layers

## Goal

Allow each area to contain an ordered stack of independently editable tile
layers. Layers can target BG1 or BG2, can be shown or hidden in the editor, and
use transparency to expose lower visible layers.

Layers are stored sparsely on disk but expanded to full-area grids in memory.
The existing main tile grid becomes an ordinary BG2 layer.

## Data model

An area contains at least one layer. Layers are ordered from bottom to top and
have:

- a nonempty name that is unique within the area and safe to use as a filename;
- a background selection of `bg1` or `bg2`;
- a full-area grid of optional tile placements in memory; and
- a non-persisted dirty flag for PNG export.

Each tile placement contains its palette, tile index, and flip. An absent
placement exposes the lower layer. A present placement replaces the whole 8x8
cell; its color-zero pixels remain transparent only when BG1 and BG2 are
composited.

Every area must contain at least one BG2 layer. Deleting the final BG2 layer or
changing it to BG1 is rejected.

### JSON format

Layers are serialized as cropped fragments:

```json
{
  "vanilla_map_id": 85,
  "bg_color": [18, 17, 10],
  "size": [2, 2],
  "layers": [
    {
      "name": "Main",
      "background": "bg2",
      "screens": [
        {
          "position": [36, 6],
          "size": [3, 2],
          "palettes": [[null, 8, null], [8, null, null]],
          "tiles": [[null, 142, null], [158, null, null]],
          "flips": [[null, 0, null], [0, null, null]]
        }
      ]
    }
  ]
}
```

Fragment `position` is `[x, y]` and `size` is `[width, height]`, both measured
in 8x8 tiles. Loading accepts fragments of any size. It rejects fragments whose
declared size does not match their tile grid, extend outside the area, or
overlap another fragment in the same layer. Transparent cells are `null` in
all three grids; mismatched null patterns are invalid.

Saving divides the area into fixed 32x32 regions, crops each region to its
occupied bounds, and omits empty regions. This keeps output deterministic and
limits saved fragments to 32x32 without making that limit part of the input
format.

## Rendering and editing

Visible layers are resolved from bottom to top within BG1 and BG2 separately.
A higher placement replaces the lower 8x8 cell, then the resolved backgrounds
are composited per pixel according to the area's background mode.

When an area is first loaded, the editor:

- shows the bottom BG1 layer, if one exists;
- shows and selects the bottom BG2 layer; and
- keeps all other layers hidden.

Selection and visibility are editor state, not project data. Selecting a hidden
layer makes it visible.

Selection reads only the selected layer. Tile selections contain optional tile
placements so transparent cells can be copied. Pasting a transparent cell
erases the destination cell.

Add an explicit erase tool. Left-dragging it removes placements from the
selected layer, and the canvas uses Iced's distinct `Cell` mouse cursor while
the tool is active.

Layer add, delete, rename, reorder, BG selection, brush, paste, and erase
operations are undoable. Visibility, selection, and drawer state are not.

## User interface

Add a collapsible layer drawer between the main area canvas and the existing
right-side panel. A Layers button in the main-area toolbar opens and closes it;
when closed, the drawer consumes no width.

The drawer contains:

- add, delete, move-up, and move-down controls;
- a scrollable bottom-to-top layer list with visibility toggles;
- layer selection by clicking a row; and
- rename and BG1/BG2 controls for the selected layer.

The secondary-area toolbar uses a layer dropdown instead of the drawer. Its
selected layer is the only layer shown and is also the source or target for
selection and editing.

## PNG export

Export one full-area RGBA PNG per layer containing only that layer. Absent tile
placements and palette color index zero remain transparent. Files use the layer
name:

```text
Areas/<area>/<theme>/<layer name>.png
```

Normal saves export only dirty layers. Renaming or deleting a layer removes its
old PNG at that time. A full PNG rebuild removes all PNG files first and then
exports every layer, cleaning up any stale files left by interrupted or unusual
workflows.

## Implementation phases

### 1. Format conversion and data foundation

- Add a small migration script that converts each old area grid into a `Main`
  BG2 layer.
- Verify the migration on a copied area.
- Replace the area screen grid with uniform layers and full-size in-memory
  grids.
- Implement cropped-fragment loading and saving.
- Add per-layer PNG dirty tracking and raw layer export.
- Route existing editing through the selected bottom BG2 layer.
- Update `theme_check` to read the bottom BG2 layer as the existing map.
- Run the migration over all area JSON files and verify serialization round
  trips against the converted project.

### 2. Editor behavior and UI

- Render visible layers with pixel transparency.
- Add the main-area drawer and secondary-area layer dropdown.
- Add layer management, visibility, and BG controls.
- Update selection, brush, paste, erase, and undo behavior for optional layer
  placements.

### 3. Integration and generated output

- Perform a full PNG rebuild.
- Run editor and repository checks against the converted project.

The migration script is written first, but the bulk conversion waits until the
editor and `theme_check` can both read the new format. This keeps the project
usable throughout the first phase.
