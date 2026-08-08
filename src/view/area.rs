// Module for displaying/editing an area
use hashbrown::HashMap;
use iced::{
    alignment::Vertical,
    mouse,
    widget::{
        button, canvas, checkbox, column, container, pick_list, row,
        scrollable::{Direction, Scrollbar},
        stack, text, text_input, Scrollable, Space,
    },
    Element, Length, Padding, Point, Rectangle, Size,
};
use iced_aw::number_input;

use crate::{
    helpers::{alpha_blend, scale_color},
    message::{Message, SelectionSource},
    state::{
        Area, AreaId, AreaPosition, Background, BackgroundLayering, ColorIdx, EditorState, Focus,
        Palette, PaletteId, Tile, TileBlock, TileCoord, TileIdx, Tool,
    },
};

use super::modal_background_style;

#[derive(Clone, Copy, PartialEq)]
struct CameraFollow(f32, &'static str);

impl std::fmt::Display for CameraFollow {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}", self.1)
    }
}

const CAMERA_FOLLOW_OPTIONS: [CameraFollow; 5] = [
    CameraFollow(1.5, "150%"),
    CameraFollow(1.0, "100%"),
    CameraFollow(0.5, "50%"),
    CameraFollow(0.25, "25%"),
    CameraFollow(0.0, "0%"),
];

// We use two separate canvases: one for drawing the tile raster and one for the tile selection.
// This is to work around a limitation in Iced's rendering pipeline that does not allow drawing
// primitives (e.g. rectangles) on top of images within a single canvas.

struct AreaGrid<'a> {
    position: AreaPosition,
    area_id: AreaId,
    area: &'a Area,
    selected_layer_idx: usize,
    visible_layers: &'a [bool],
    palettes: &'a [Palette],
    palettes_id_idx_map: &'a HashMap<PaletteId, usize>,
    end_coords: Option<(TileCoord, TileCoord)>,
    pixel_size: f32,
    // thickness: f32,
    palette_only_brush: bool,
    tile_block: &'a TileBlock,
    identify_tile: bool,
    palette_idx: usize,
    tile_idx: Option<TileIdx>,
    identify_color: bool,
    color_idx: Option<ColorIdx>,
    tool: Tool,
    snap_grid_16: bool,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
enum InternalStateAction {
    #[default]
    None,
    Selecting,
    Brushing,
}

#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
struct InternalState {
    action: InternalStateAction,
    coords: Option<Point<TileCoord>>,
}

fn clamped_position_in(
    p: Point,
    bounds: iced::Rectangle,
    size: (u8, u8),
    pixel_size: f32,
    snap_grid_16: bool,
) -> Point<TileCoord> {
    let x = f32::max(p.x - bounds.x - 1.0 - pixel_size / 2.0, 0.0) / (8.0 * pixel_size);
    let y = f32::max(p.y - bounds.y - 1.0 - pixel_size / 2.0, 0.0) / (8.0 * pixel_size);
    if snap_grid_16 {
        Point {
            x: ((x / 2.0) as TileCoord * 2).min(size.0 as TileCoord * 32 - 2),
            y: ((y / 2.0) as TileCoord * 2).min(size.1 as TileCoord * 32 - 2),
        }
    } else {
        Point {
            x: (x as TileCoord).min(size.0 as TileCoord * 32 - 1),
            y: (y as TileCoord).min(size.1 as TileCoord * 32 - 1),
        }
    }
}

impl<'a> canvas::Program<Message> for AreaGrid<'a> {
    type State = InternalState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let Some(p) = cursor.position_over(bounds) {
            state.coords = Some(clamped_position_in(
                p,
                bounds,
                self.area.size,
                self.pixel_size,
                self.snap_grid_16,
            ));
        } else {
            state.coords = None;
        }
        if let canvas::Event::Mouse(mouse_event) = event {
            match mouse_event {
                mouse::Event::ButtonPressed(btn @ (mouse::Button::Left | mouse::Button::Right)) => {
                    if let Some(p) = cursor.position_over(bounds) {
                        match (self.tool, btn) {
                            (Tool::Brush, mouse::Button::Left) => {
                                state.action = InternalStateAction::Brushing;
                                let coords = clamped_position_in(
                                    p,
                                    bounds,
                                    self.area.size,
                                    self.pixel_size,
                                    self.snap_grid_16,
                                );
                                return (
                                    canvas::event::Status::Captured,
                                    Some(Message::AreaBrush {
                                        position: self.position,
                                        area_id: self.area_id.clone(),
                                        layer_idx: self.selected_layer_idx,
                                        coords,
                                        selection: self.tile_block.clone(),
                                        palette_only: self.palette_only_brush,
                                    }),
                                );
                            }
                            (Tool::Erase, mouse::Button::Left) => {
                                state.action = InternalStateAction::Brushing;
                                return (
                                    canvas::event::Status::Captured,
                                    Some(Message::AreaErase {
                                        position: self.position,
                                        area_id: self.area_id.clone(),
                                        layer_idx: self.selected_layer_idx,
                                        coords: clamped_position_in(
                                            p,
                                            bounds,
                                            self.area.size,
                                            self.pixel_size,
                                            self.snap_grid_16,
                                        ),
                                        size: if self.snap_grid_16 { 2 } else { 1 },
                                    }),
                                );
                            }
                            (Tool::Select, mouse::Button::Left | mouse::Button::Right)
                            | (Tool::Brush | Tool::Erase, mouse::Button::Right) => {
                                state.action = InternalStateAction::Selecting;
                                return (
                                    canvas::event::Status::Captured,
                                    Some(Message::StartTileSelection(
                                        clamped_position_in(
                                            p,
                                            bounds,
                                            self.area.size,
                                            self.pixel_size,
                                            self.snap_grid_16,
                                        ),
                                        crate::message::SelectionSource::Area(self.position),
                                    )),
                                );
                            }
                            _ => {}
                        }
                    };
                }
                mouse::Event::ButtonReleased(mouse::Button::Left | mouse::Button::Right) => {
                    let state0 = *state;
                    state.action = InternalStateAction::None;
                    if state0.action == InternalStateAction::Selecting {
                        let coords = if let Some(p) = cursor.position() {
                            clamped_position_in(
                                p,
                                bounds,
                                self.area.size,
                                self.pixel_size,
                                self.snap_grid_16,
                            )
                        } else if let Some(c) = self.end_coords {
                            Point::new(c.0, c.1)
                        } else {
                            return (canvas::event::Status::Ignored, None);
                        };
                        return (
                            canvas::event::Status::Captured,
                            Some(Message::EndTileSelection(coords)),
                        );
                    }
                }
                mouse::Event::CursorMoved { .. } => match state.action {
                    InternalStateAction::None => {
                        if let Some(p) = cursor.position() {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::HoverArea(clamped_position_in(
                                    p,
                                    bounds,
                                    self.area.size,
                                    self.pixel_size,
                                    self.snap_grid_16,
                                ))),
                            );
                        } else {
                            return (canvas::event::Status::Ignored, None);
                        }
                    }
                    InternalStateAction::Selecting => {
                        if let Some(p) = cursor.position() {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::ProgressTileSelection(clamped_position_in(
                                    p,
                                    bounds,
                                    self.area.size,
                                    self.pixel_size,
                                    self.snap_grid_16,
                                ))),
                            );
                        } else {
                            return (canvas::event::Status::Captured, Some(Message::HoverAreaEnd));
                        }
                    }
                    InternalStateAction::Brushing => {
                        if let Some(p) = cursor.position() {
                            let coords = clamped_position_in(
                                p,
                                bounds,
                                self.area.size,
                                self.pixel_size,
                                self.snap_grid_16,
                            );
                            let message = if self.tool == Tool::Erase {
                                Message::AreaErase {
                                    position: self.position,
                                    area_id: self.area_id.clone(),
                                    layer_idx: self.selected_layer_idx,
                                    coords,
                                    size: if self.snap_grid_16 { 2 } else { 1 },
                                }
                            } else {
                                Message::AreaBrush {
                                    position: self.position,
                                    area_id: self.area_id.clone(),
                                    layer_idx: self.selected_layer_idx,
                                    coords,
                                    selection: self.tile_block.clone(),
                                    palette_only: self.palette_only_brush,
                                }
                            };
                            return (canvas::event::Status::Captured, Some(message));
                        } else {
                            return (canvas::event::Status::Captured, Some(Message::HoverAreaEnd));
                        }
                    }
                },
                mouse::Event::CursorLeft => {
                    return (canvas::event::Status::Captured, Some(Message::HoverAreaEnd));
                }
                _ => {}
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        state: &InternalState,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let mut color_bytes: Vec<Vec<[u8; 3]>> = vec![];

        for i in 0..self.palettes.len() {
            let mut colors = self.palettes[i].colors;
            colors[0] = self.area.bg_color;
            let cb = colors
                .iter()
                .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
                .collect();
            color_bytes.push(cb);
        }

        // Add a pixel of transparent padding around the image, since Iced's
        // "nearest neighbor" filter results in the edge pixels having the wrong size.
        let num_cols = self.area.size.0 as usize * 256 + 2;
        let num_rows = self.area.size.1 as usize * 256 + 2;
        let mut data: Vec<u8> = vec![0; num_rows * num_cols * 4];
        let mut bg1_data = (self.area.bg_layering == BackgroundLayering::HalfAdd)
            .then(|| vec![0; data.len()]);
        let col_stride = 4;
        let row_stride = num_cols * col_stride;
        let bg = self.area.bg_color.map(scale_color);
        for y in 1..num_rows - 1 {
            for x in 1..num_cols - 1 {
                let addr = y * row_stride + x * col_stride;
                data[addr..addr + 3].copy_from_slice(&bg);
                data[addr + 3] = 255;
            }
        }
        for (layer_idx, layer) in self.area.layers.iter().enumerate() {
            let visible = match self.position {
                AreaPosition::Main => self.visible_layers.get(layer_idx) == Some(&true),
                AreaPosition::Side => layer_idx == self.selected_layer_idx,
            };
            if !visible {
                continue;
            }
            let layer_data = match (&mut bg1_data, layer.background) {
                (Some(bg1_data), Background::Bg1) => bg1_data,
                _ => &mut data,
            };
            for (ty, row) in layer.tiles.iter().enumerate() {
                for (tx, placement) in row.iter().enumerate() {
                    let Some(placement) = placement else {
                        continue;
                    };
                    let palette_id = placement.palette;
                    let Some(&palette_idx) = self.palettes_id_idx_map.get(&palette_id) else {
                        // TODO: draw some indicator of the broken tile (due to invalid palette reference)
                        continue;
                    };
                    let tile_idx = placement.tile;
                    if tile_idx as usize >= self.palettes[palette_idx].tiles.len() {
                        continue;
                    }
                    let flip = placement.flip;
                    let tile = self.palettes[palette_idx].tiles[tile_idx as usize];
                    let tile = flip.apply_to_tile(tile);
                    let cb = &color_bytes[palette_idx];
                    let mut tile_addr = (ty * 8 + 1) * row_stride + (tx * 8 + 1) * col_stride;

                    let illegal_flip = match flip {
                        crate::state::Flip::None => false,
                        crate::state::Flip::Horizontal => !tile.h_flippable,
                        crate::state::Flip::Vertical => !tile.v_flippable,
                        crate::state::Flip::Both => !tile.h_flippable || !tile.v_flippable,
                    };
                    let identify_tile = self.identify_tile
                        && self.palette_idx == palette_idx
                        && self.tile_idx == Some(tile_idx);
                    for py in 0..8 {
                        let mut addr = tile_addr;
                        for px in 0..8 {
                            let color_idx = tile.pixels[py][px];
                            if color_idx == 0 {
                                addr += 4;
                                continue;
                            }
                            let mut color = cb[color_idx as usize];
                            let identify_color = self.identify_color
                                && self.color_idx == Some(color_idx)
                                && self.palette_idx == palette_idx;

                            if illegal_flip && !self.identify_tile && !self.identify_color {
                                let red_highlight = [255, 0, 0];
                                let alpha = 0.5;
                                color = alpha_blend(color, red_highlight, alpha);
                            }

                            let pink_highlight = [255, 105, 180];
                            if identify_tile {
                                let alpha = 0.5;
                                color = alpha_blend(color, pink_highlight, alpha);
                            } else if identify_color {
                                color = pink_highlight;
                            }
                            layer_data[addr..(addr + 3)].copy_from_slice(&color);
                            layer_data[addr + 3] = 255;
                            addr += 4;
                        }
                        tile_addr += row_stride;
                    }
                }
            }
        }
        if let Some(bg1_data) = bg1_data {
            for addr in (0..data.len()).step_by(4) {
                if bg1_data[addr + 3] == 0 {
                    continue;
                }
                for channel in 0..3 {
                    data[addr + channel] =
                        ((data[addr + channel] as u16 + bg1_data[addr + channel] as u16) / 2) as u8;
                }
            }
        }

        if self.tool == Tool::Brush && self.end_coords.is_none() {
            // Overlay the block to be pasted/brushed onto the area:
            if let Some(Point {
                x: base_x,
                y: base_y,
            }) = state.coords
            {
                let base_addr =
                    (base_y * 8 + 1) as usize * row_stride + (base_x * 8 + 1) as usize * col_stride;
                let alpha = 0.75;
                for ty in 0..self.tile_block.size.1 as usize {
                    for tx in 0..self.tile_block.size.0 as usize {
                        if tx + base_x as usize >= self.area.size.0 as usize * 32
                            || ty + base_y as usize >= self.area.size.1 as usize * 32
                        {
                            continue;
                        }
                        let Some(placement) = self.tile_block.placements[ty][tx] else {
                            continue;
                        };
                        if self.palette_only_brush
                            && self
                                .area
                                .get_layer_placement(
                                    self.selected_layer_idx,
                                    base_x + tx as TileCoord,
                                    base_y + ty as TileCoord,
                                )
                                .ok()
                                .flatten()
                                .is_none()
                        {
                            continue;
                        }
                        let palette_id = placement.palette;
                        if let Some(&palette_idx) = self.palettes_id_idx_map.get(&palette_id) {
                            let tile = if self.palette_only_brush {
                                let x1 = base_x + tx as TileCoord;
                                let y1 = base_y + ty as TileCoord;
                                let destination = self
                                    .area
                                    .get_layer_placement(self.selected_layer_idx, x1, y1)
                                    .unwrap()
                                    .unwrap();
                                let tile_idx = destination.tile;
                                let flip = destination.flip;
                                let clamped_tile_idx = std::cmp::min(
                                    tile_idx,
                                    self.palettes[palette_idx].tiles.len() as TileIdx - 1,
                                );
                                // TODO: indicate out-of-bounds tile index with some consistent broken tile indicator
                                let t = *self.palettes[palette_idx]
                                    .tiles
                                    .get(clamped_tile_idx as usize)
                                    .unwrap();
                                flip.apply_to_tile(t)
                            } else {
                                let tile_idx = placement.tile;
                                let flip = placement.flip;
                                let t = if (tile_idx as usize)
                                    < self.palettes[palette_idx].tiles.len()
                                {
                                    self.palettes[palette_idx].tiles[tile_idx as usize]
                                } else {
                                    Tile::default()
                                };
                                flip.apply_to_tile(t)
                            };
                            let cb = &color_bytes[palette_idx];
                            let mut tile_addr =
                                base_addr + ty * 8 * row_stride + tx * 8 * col_stride;
                            for py in 0..8 {
                                let mut addr = tile_addr;
                                for px in 0..8 {
                                    let color_idx = tile.pixels[py][px];
                                    if color_idx == 0 {
                                        addr += 4;
                                        continue;
                                    }
                                    let old_color = [data[addr], data[addr + 1], data[addr + 2]];
                                    let new_color = cb[color_idx as usize];
                                    let blended_color = alpha_blend(old_color, new_color, alpha);
                                    data[addr..addr + 3].copy_from_slice(&blended_color);
                                    addr += 4;
                                }
                                tile_addr += row_stride;
                            }
                        } else {
                            // TODO: draw some indicator of the broken tile (due to invalid palette reference)
                        }
                    }
                }
            }
        }

        let image = iced::advanced::image::Image::new(iced::advanced::image::Handle::from_rgba(
            num_cols as u32,
            num_rows as u32,
            data,
        ))
        .filter_method(iced::widget::image::FilterMethod::Nearest);

        frame.draw_image(
            Rectangle::new(
                Point::new(0.0, 0.0),
                Size {
                    width: num_cols as f32 * self.pixel_size,
                    height: num_rows as f32 * self.pixel_size,
                },
            ),
            image,
        );

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _interaction: &Self::State,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) {
            match self.tool {
                Tool::Select => mouse::Interaction::default(),
                Tool::Brush => mouse::Interaction::Crosshair,
                Tool::Erase => mouse::Interaction::Cell,
                Tool::Move => mouse::Interaction::NotAllowed,
            }
        } else {
            mouse::Interaction::default()
        }
    }
}

struct AreaSelect {
    top: TileCoord,
    bottom: TileCoord,
    left: TileCoord,
    right: TileCoord,
    selecting_active: bool,
    pixel_size: f32,
    show_grid_16: bool,
    snap_grid_16: bool,
    grid_alpha: f32,
    hover_coords: Option<(TileCoord, TileCoord)>,
}

impl canvas::Program<Message> for AreaSelect {
    // No internal state
    type State = ();

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        if !self.selecting_active && !self.show_grid_16 && self.hover_coords.is_none() {
            return vec![];
        }

        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let pixel_size_x =
            self.pixel_size * bounds.size().width / (bounds.size().width - self.pixel_size);
        let pixel_size_y =
            self.pixel_size * bounds.size().height / (bounds.size().height - self.pixel_size);
        if self.show_grid_16 {
            let rows16 = (bounds.size().height / (pixel_size_y * 16.0)) as u16;
            let cols16 = (bounds.size().width / (pixel_size_x * 16.0)) as u16;

            let path = canvas::Path::new(|p| {
                for i in 0..=rows16 {
                    let x = i as f32 * pixel_size_x * 16.0 + pixel_size_x / 2.0;
                    p.move_to(Point::new(x, self.pixel_size / 2.0));
                    p.line_to(Point::new(x, bounds.height - self.pixel_size / 2.0));
                }
                for i in 0..=cols16 {
                    let y = i as f32 * pixel_size_y * 16.0 + pixel_size_y / 2.0;
                    p.move_to(Point::new(self.pixel_size / 2.0, y));
                    p.line_to(Point::new(bounds.width - self.pixel_size / 2.0, y));
                }
            });
            frame.stroke(
                &path,
                canvas::Stroke {
                    style: canvas::stroke::Style::Solid(iced::Color::from_rgba(
                        1.0,
                        1.0,
                        1.0,
                        self.grid_alpha,
                    )),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }

        if self.selecting_active {
            let x0 = self.left as f32 * pixel_size_x * 8.0 + pixel_size_x / 2.0;
            let x1 = (self.right + 1) as f32 * pixel_size_x * 8.0 + pixel_size_x / 2.0;
            let y0 = self.top as f32 * pixel_size_y * 8.0 + pixel_size_y / 2.0;
            let y1 = (self.bottom + 1) as f32 * pixel_size_y * 8.0 + pixel_size_y / 2.0;
            let path = canvas::Path::rectangle(
                iced::Point { x: x0, y: y0 },
                Size {
                    width: x1 - x0,
                    height: y1 - y0,
                },
            );
            for i in 0..2 {
                frame.stroke(
                    &path,
                    canvas::Stroke {
                        style: if i == 0 {
                            canvas::stroke::Style::Solid(iced::Color::WHITE)
                        } else {
                            canvas::stroke::Style::Solid(iced::Color::BLACK)
                        },
                        width: 1.0,
                        line_dash: canvas::LineDash {
                            offset: i,
                            segments: &[0.0, 0.0, 4.0, 4.0],
                        },
                        line_join: canvas::LineJoin::Miter,
                        ..Default::default()
                    },
                );
            }
        } else if let Some(h) = self.hover_coords {
            let x0 = h.0 as f32 * pixel_size_x * 8.0 + pixel_size_x / 2.0;
            let mut x1 = (h.0 + 1) as f32 * pixel_size_x * 8.0 + pixel_size_x / 2.0;
            let y0 = h.1 as f32 * pixel_size_y * 8.0 + pixel_size_y / 2.0;
            let mut y1 = (h.1 + 1) as f32 * pixel_size_y * 8.0 + pixel_size_y / 2.0;
            if self.snap_grid_16 {
                x1 += 8.0 * pixel_size_x;
                y1 += 8.0 * pixel_size_y;
            }

            let path = canvas::Path::rectangle(
                iced::Point { x: x0, y: y0 },
                Size {
                    width: x1 - x0,
                    height: y1 - y0,
                },
            );
            frame.stroke(
                &path,
                canvas::Stroke {
                    style: canvas::stroke::Style::Solid(iced::Color::from_rgba(1.0, 1.0, 1.0, 0.3)),
                    width: 1.0,
                    ..Default::default()
                },
            );
        }
        vec![frame.into_geometry()]
    }
}

pub fn area_grid_view(state: &EditorState, position: AreaPosition) -> Element<'_, Message> {
    let area = state.area(position);
    let num_cols = area.size.0 * 32;
    let num_rows = area.size.1 * 32;
    let pixel_size = state.global_config.pixel_size;

    let mut left = 0;
    let mut right = 0;
    let mut top = 0;
    let mut bottom = 0;

    if let (Some(p0), Some(p1)) = (state.start_coords, state.end_coords) {
        left = p0.0.min(p1.0);
        right = p0.0.max(p1.0);
        top = p0.1.min(p1.1);
        bottom = p0.1.max(p1.1);
    }

    if state.snap_grid_16 {
        right += 1;
        bottom += 1;
    }

    Scrollable::with_direction(
        column![stack![
            canvas(AreaGrid {
                position,
                area_id: state.area_id(position).clone(),
                area: state.area(position),
                selected_layer_idx: state.selected_layer_idx(position),
                visible_layers: &state.visible_layers,
                palettes: &state.palettes,
                palettes_id_idx_map: &state.palettes_id_idx_map,
                pixel_size,
                end_coords: state.end_coords,
                // thickness: 1.0,
                palette_only_brush: state.shift_brush,
                tile_block: &state.selected_tile_block,
                identify_tile: state.identify_tile,
                palette_idx: state.palette_idx,
                tile_idx: state.tile_idx,
                identify_color: state.identify_color,
                color_idx: state.color_idx,
                tool: state.tool,
                snap_grid_16: state.snap_grid_16,
            })
            .width((num_cols as f32 * 8.0 + 2.0) * pixel_size)
            .height((num_rows as f32 * 8.0 + 2.0) * pixel_size),
            canvas(AreaSelect {
                selecting_active: state.selection_source == SelectionSource::Area(position)
                    && state.start_coords.is_some()
                    && state.end_coords.is_some(),
                left,
                right,
                top,
                bottom,
                pixel_size,
                show_grid_16: state.show_grid_16,
                snap_grid_16: state.snap_grid_16,
                hover_coords: state.hover_coords,
                grid_alpha: state.global_config.grid_alpha,
            })
            .width((num_cols as f32 * 8.0 + 2.0) * pixel_size)
            .height((num_rows as f32 * 8.0 + 2.0) * pixel_size),
        ]]
        .padding(Padding::new(0.0).right(16.0).bottom(16.0)),
        Direction::Both {
            vertical: Scrollbar::default(),
            horizontal: Scrollbar::default(),
        },
    )
    .width(Length::Fill)
    .height(Length::Fill)
    .into()
}

pub fn main_area_controls(state: &EditorState) -> Element<'_, Message> {
    row![
        text("Area"),
        pick_list(
            state.area_names.clone(),
            Some(state.main_area().name.clone()),
            |x| Message::SelectArea(AreaPosition::Main, x)
        )
        .on_open(Message::Focus(Focus::PickArea(AreaPosition::Main)))
        .width(200),
        button(text("\u{F64D}").font(iced_fonts::BOOTSTRAP_FONT))
            .style(button::success)
            .on_press(Message::AddAreaDialogue),
        button(text("\u{F4CB}").font(iced_fonts::BOOTSTRAP_FONT))
            .on_press(Message::EditAreaDialogue),
        text("Theme"),
        pick_list(
            state.theme_names.clone(),
            Some(state.main_area().theme.clone()),
            |x| Message::SelectTheme(AreaPosition::Main, x)
        )
        .on_open(Message::Focus(Focus::PickTheme(AreaPosition::Main)))
        .width(200),
        button(text("\u{F64D}").font(iced_fonts::BOOTSTRAP_FONT))
            .style(button::success)
            .on_press(Message::AddThemeDialogue),
        button(text("\u{F4CB}").font(iced_fonts::BOOTSTRAP_FONT))
            .on_press(Message::RenameThemeDialogue),
        button(text("Layers"))
            .style(if state.layer_drawer_open {
                button::primary
            } else {
                button::secondary
            })
            .on_press(Message::ToggleLayerDrawer),
    ]
    .spacing(10)
    .clip(true)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

pub fn side_area_controls(state: &EditorState) -> Element<'_, Message> {
    let layer_names: Vec<String> = state
        .side_area()
        .layers
        .iter()
        .map(|layer| layer.name.clone())
        .collect();
    let find_layer = layer_names.clone();
    row![
        pick_list(
            state.area_names.clone(),
            Some(state.side_area().name.clone()),
            |x| Message::SelectArea(AreaPosition::Side, x)
        )
        .on_open(Message::Focus(Focus::PickArea(AreaPosition::Side)))
        .width(125),
        pick_list(
            state.theme_names.clone(),
            Some(state.side_area().theme.clone()),
            |x| Message::SelectTheme(AreaPosition::Side, x)
        )
        .on_open(Message::Focus(Focus::PickTheme(AreaPosition::Side)))
        .width(125),
        pick_list(
            layer_names,
            Some(state.side_area().layers[state.side_layer_idx].name.clone()),
            move |name| Message::SelectLayer(
                AreaPosition::Side,
                find_layer.iter().position(|layer| layer == &name).unwrap()
            )
        )
        .width(150),
    ]
    .spacing(10)
    .clip(true)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

pub fn layer_drawer_view(state: &EditorState) -> Element<'_, Message> {
    let area_id = state.main_area_id.clone();
    let selected = state.main_layer_idx;
    let layers = &state.main_area().layers;
    let bg2_count = layers
        .iter()
        .filter(|layer| layer.background == Background::Bg2)
        .count();

    let mut list = column![];
    for (layer_idx, layer) in layers.iter().enumerate() {
        let row_button = button(text(&layer.name).width(Length::Fill))
            .style(if layer_idx == selected {
                button::primary
            } else {
                button::secondary
            })
            .width(Length::Fill)
            .on_press(Message::SelectLayer(AreaPosition::Main, layer_idx));
        list = list.push(
            row![
                checkbox("", state.visible_layers.get(layer_idx) == Some(&true))
                    .on_toggle(move |_| Message::ToggleLayerVisibility(layer_idx)),
                row_button,
            ]
            .spacing(5)
            .align_y(Vertical::Center),
        );
    }

    let can_delete =
        layers.len() > 1 && (layers[selected].background != Background::Bg2 || bg2_count > 1);
    let backgrounds = if layers[selected].background == Background::Bg2 && bg2_count == 1 {
        vec![Background::Bg2]
    } else {
        vec![Background::Bg1, Background::Bg2]
    };
    let delete = button(text("\u{F63B}").font(iced_fonts::BOOTSTRAP_FONT)).style(button::danger);
    let delete = if can_delete {
        delete.on_press(Message::DeleteLayer {
            position: AreaPosition::Main,
            area_id: area_id.clone(),
            layer_idx: selected,
        })
    } else {
        delete
    };

    container(
        column![
            row![
                button(text("\u{F64D}").font(iced_fonts::BOOTSTRAP_FONT))
                    .style(button::success)
                    .on_press(Message::AddLayer(AreaPosition::Main)),
                delete,
                button(text("\u{F128}").font(iced_fonts::BOOTSTRAP_FONT)).on_press_maybe(
                    (selected + 1 < layers.len()).then(|| Message::MoveLayer {
                        position: AreaPosition::Main,
                        area_id: area_id.clone(),
                        layer_idx: selected,
                        new_idx: selected + 1,
                    })
                ),
                button(text("\u{F148}").font(iced_fonts::BOOTSTRAP_FONT)).on_press_maybe(
                    (selected > 0).then(|| Message::MoveLayer {
                        position: AreaPosition::Main,
                        area_id: area_id.clone(),
                        layer_idx: selected,
                        new_idx: selected - 1,
                    })
                ),
            ]
            .spacing(5),
            Scrollable::new(list.spacing(5)).height(Length::Fill),
            text_input("Layer name", &layers[selected].name).on_input(move |name| {
                Message::RenameLayer {
                    position: AreaPosition::Main,
                    area_id: area_id.clone(),
                    layer_idx: selected,
                    name,
                }
            }),
            pick_list(
                backgrounds,
                Some(layers[selected].background),
                move |background| Message::SetLayerBackground {
                    position: AreaPosition::Main,
                    area_id: state.main_area_id.clone(),
                    layer_idx: selected,
                    background,
                }
            ),
        ]
        .spacing(10),
    )
    .padding(10)
    .width(260)
    .height(Length::Fill)
    .into()
}

pub fn add_area_view(name: &str, size: (u8, u8)) -> Element<'_, Message> {
    let add_area_msg = Message::AddArea {
        name: name.to_owned(),
        size,
    };
    container(
        column![
            text("Add a new area."),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("AddArea")
                    .on_input(Message::SetAddAreaName)
                    .on_submit(add_area_msg.clone())
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Size: ").width(70),
                number_input(&size.0, 1..=8, Message::SetAddAreaSizeX)
                    .width(50)
                    .on_submit(add_area_msg.clone()),
                text(" by "),
                number_input(&size.1, 1..=8, Message::SetAddAreaSizeY)
                    .width(50)
                    .on_submit(add_area_msg.clone()),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            button(text("Add area"))
                .style(button::success)
                .on_press(add_area_msg.clone()),
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn edit_area_view(state: &EditorState, name: &str) -> Element<'static, Message> {
    let old_name = state.main_area().name.clone();
    let area_id = state.main_area_id.clone();
    let area = state.main_area();
    let rgb_width = 80;
    let edit_area_msg = Message::EditArea {
        old_name: old_name.clone(),
        new_name: name.to_owned(),
    };
    container(
        column![
            text(format!("Edit area \"{}\"", old_name)),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("EditArea")
                    .on_input(Message::SetEditAreaName)
                    .on_submit(edit_area_msg.clone())
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![text("Background color:")],
            row![
                text("Red"),
                number_input(
                    &area.bg_color[0],
                    0..=31,
                    Message::EditAreaBGRed
                )
                .width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Green"),
                number_input(
                    &area.bg_color[1],
                    0..=31,
                    Message::EditAreaBGGreen
                )
                .width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Blue"),
                number_input(
                    &area.bg_color[2],
                    0..=31,
                    Message::EditAreaBGBlue
                )
                .width(rgb_width),
            ]
            .spacing(5)
            .align_y(iced::alignment::Vertical::Center),
            row![
                text("Background layering:").width(220),
                pick_list(BackgroundLayering::ALL, Some(area.bg_layering), {
                    let area_id = area_id.clone();
                    move |value| Message::EditAreaBGLayering {
                        area_id: area_id.clone(),
                        value,
                    }
                }),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Background X camera follow:").width(220),
                pick_list(
                    CAMERA_FOLLOW_OPTIONS,
                    CAMERA_FOLLOW_OPTIONS
                        .iter()
                        .find(|option| option.0 == area.bg_camera_follow_x),
                    {
                        let area_id = area_id.clone();
                        move |value| Message::EditAreaBGCameraFollowX {
                            area_id: area_id.clone(),
                            value: value.0,
                        }
                    },
                ),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Background X camera drift:").width(220),
                number_input(
                    &area.bg_camera_drift_x,
                    -512.0..=512.0,
                    Message::EditAreaBGCameraDriftX,
                )
                .width(100),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Background Y camera follow:").width(220),
                pick_list(
                    CAMERA_FOLLOW_OPTIONS,
                    CAMERA_FOLLOW_OPTIONS
                        .iter()
                        .find(|option| option.0 == area.bg_camera_follow_y),
                    {
                        let area_id = area_id.clone();
                        move |value| Message::EditAreaBGCameraFollowY {
                            area_id: area_id.clone(),
                            value: value.0,
                        }
                    },
                ),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Background Y camera drift:").width(220),
                number_input(
                    &area.bg_camera_drift_y,
                    -512.0..=512.0,
                    Message::EditAreaBGCameraDriftY,
                )
                .width(100),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                button(text("Edit area")).on_press(edit_area_msg.clone()),
                Space::with_width(Length::Fill),
                button(text("Delete area"))
                    .style(button::danger)
                    .on_press(Message::DeleteAreaDialogue),
            ],
        ]
        .spacing(15),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn delete_area_view(state: &EditorState) -> Element<'_, Message> {
    let name = state.main_area().name.clone();
    container(
        column![
            text(format!("Delete area \"{}\"?", name)),
            text("This will delete the area across all themes."),
            text("This action cannot be undone."),
            button(text("Delete area"))
                .style(button::danger)
                .on_press(Message::DeleteArea(name.clone())),
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn add_theme_view(name: &str) -> Element<'_, Message> {
    container(
        column![
            text("Add a new theme."),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("AddTheme")
                    .on_input(Message::SetAddThemeName)
                    .on_submit(Message::AddTheme(name.to_owned()))
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            button(text("Add theme"))
                .style(button::success)
                .on_press(Message::AddTheme(name.to_owned())),
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn rename_theme_view(state: &EditorState, name: &str) -> Element<'static, Message> {
    let old_name = state.main_area().theme.clone();
    let rename_msg = Message::RenameTheme {
        old_name: old_name.clone(),
        new_name: name.to_owned(),
    };
    container(
        column![
            text(format!("Rename theme \"{}\"", old_name)),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("RenameTheme")
                    .on_input(Message::SetRenameThemeName)
                    .on_submit(rename_msg.clone())
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                button(text("Rename theme")).on_press(rename_msg.clone()),
                Space::with_width(Length::Fill),
                button(text("Delete theme"))
                    .style(button::danger)
                    .on_press(Message::DeleteThemeDialogue),
            ],
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn delete_theme_view(state: &EditorState) -> Element<'_, Message> {
    let theme = state.main_area().theme.clone();
    container(
        column![
            text(format!("Delete theme \"{}\"?", theme)),
            text("This will delete the theme across all areas."),
            text("This action cannot be undone."),
            button(text("Delete theme"))
                .style(button::danger)
                .on_press(Message::DeleteTheme(theme.clone())),
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}
