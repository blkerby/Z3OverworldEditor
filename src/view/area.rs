// Module for displaying/editing an area
use hashbrown::HashMap;
use iced::{
    alignment::Vertical,
    mouse,
    widget::{
        button, canvas, column, container, pick_list, row,
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
        Area, AreaId, AreaPosition, ColorIdx, EditorState, Focus, Palette, PaletteId, TileBlock,
        TileCoord, TileIdx, Tool,
    },
};

use super::modal_background_style;

// We use two separate canvases: one for drawing the tile raster and one for the tile selection.
// This is to work around a limitation in Iced's rendering pipeline that does not allow drawing
// primitives (e.g. rectangles) on top of images within a single canvas.

struct AreaGrid<'a> {
    position: AreaPosition,
    area_id: AreaId,
    area: &'a Area,
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
) -> Point<TileCoord> {
    let x = f32::max(p.x - bounds.x - 1.0 - pixel_size / 2.0, 0.0) / (8.0 * pixel_size);
    let y = f32::max(p.y - bounds.y - 1.0 - pixel_size / 2.0, 0.0) / (8.0 * pixel_size);
    Point {
        x: (x as TileCoord).min(size.0 as TileCoord * 32 - 1),
        y: (y as TileCoord).min(size.1 as TileCoord * 32 - 1),
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
            ));
        } else {
            state.coords = None;
        }
        match event {
            canvas::Event::Mouse(mouse_event) => match mouse_event {
                mouse::Event::ButtonPressed(btn @ (mouse::Button::Left | mouse::Button::Right)) => {
                    if let Some(p) = cursor.position_over(bounds) {
                        match (self.tool, btn) {
                            (Tool::Brush, mouse::Button::Left) => {
                                state.action = InternalStateAction::Brushing;
                                let coords =
                                    clamped_position_in(p, bounds, self.area.size, self.pixel_size);
                                return (
                                    canvas::event::Status::Captured,
                                    Some(Message::AreaBrush {
                                        position: self.position,
                                        area_id: self.area_id.clone(),
                                        coords,
                                        selection: self.tile_block.clone(),
                                        palette_only: self.palette_only_brush,
                                    }),
                                );
                            }
                            (Tool::Select, mouse::Button::Left | mouse::Button::Right)
                            | (Tool::Brush, mouse::Button::Right) => {
                                state.action = InternalStateAction::Selecting;
                                return (
                                    canvas::event::Status::Captured,
                                    Some(Message::StartTileSelection(
                                        clamped_position_in(
                                            p,
                                            bounds,
                                            self.area.size,
                                            self.pixel_size,
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
                            clamped_position_in(p, bounds, self.area.size, self.pixel_size)
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
                    InternalStateAction::None => {}
                    InternalStateAction::Selecting => {
                        if let Some(p) = cursor.position() {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::ProgressTileSelection(clamped_position_in(
                                    p,
                                    bounds,
                                    self.area.size,
                                    self.pixel_size,
                                ))),
                            );
                        }
                    }
                    InternalStateAction::Brushing => {
                        if let Some(p) = cursor.position() {
                            let coords =
                                clamped_position_in(p, bounds, self.area.size, self.pixel_size);
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::AreaBrush {
                                    position: self.position,
                                    area_id: self.area_id.clone(),
                                    coords,
                                    selection: self.tile_block.clone(),
                                    palette_only: self.palette_only_brush,
                                }),
                            );
                        }
                    }
                },
                _ => {}
            },
            _ => {}
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
            let mut colors = self.palettes[i].colors.clone();
            colors[0] = self.area.bg_color;
            let cb = colors
                .iter()
                .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
                .collect();
            color_bytes.push(cb);
        }

        // Add a pixel of transparent padding around the image, since Iced's
        // "nearest neighbor" filter results in the edge pixels having the wrong size.
        let num_cols = self.area.size.1 as usize * 256 + 2;
        let num_rows = self.area.size.0 as usize * 256 + 2;
        let mut data: Vec<u8> = vec![0; num_rows * num_cols * 4];
        let col_stride = 4;
        let row_stride = num_cols * col_stride;
        for sy in 0..self.area.size.1 as usize {
            for sx in 0..self.area.size.0 as usize {
                let screen = &self.area.screens[sy * self.area.size.0 as usize + sx];
                let screen_addr = (sy * 256 + 1) * row_stride + (sx * 256 + 1) * col_stride;
                for ty in 0..32 {
                    for tx in 0..32 {
                        let palette_id = screen.palettes[ty][tx];
                        let Some(&palette_idx) = self.palettes_id_idx_map.get(&palette_id) else {
                            // TODO: draw some indicator of the broken tile (due to invalid palette reference)
                            continue;
                        };
                        let tile_idx = screen.tiles[ty][tx];
                        if tile_idx as usize >= self.palettes[palette_idx].tiles.len() {
                            continue;
                        }
                        let flip = screen.flips[ty][tx];
                        let tile = self.palettes[palette_idx].tiles[tile_idx as usize];
                        let tile = flip.apply_to_tile(tile);
                        let cb = &color_bytes[palette_idx];
                        let mut tile_addr = screen_addr + ty * 8 * row_stride + tx * 8 * col_stride;

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
                                data[addr..(addr + 3)].copy_from_slice(&color);
                                data[addr + 3] = 255;
                                addr += 4;
                            }
                            tile_addr += row_stride;
                        }
                    }
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
                        let palette_id = self.tile_block.palettes[ty][tx];
                        if let Some(&palette_idx) = self.palettes_id_idx_map.get(&palette_id) {
                            let tile = if self.palette_only_brush {
                                let x1 = base_x + tx as TileCoord;
                                let y1 = base_y + ty as TileCoord;
                                let tile_idx = self.area.get_tile(x1, y1).unwrap();
                                let flip = self.area.get_flip(x1, y1).unwrap();
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
                                let tile_idx = self.tile_block.tiles[ty][tx];
                                let flip = self.tile_block.flips[ty][tx];
                                let t = self.palettes[palette_idx].tiles[tile_idx as usize];
                                flip.apply_to_tile(t)
                            };
                            let cb = &color_bytes[palette_idx];
                            let mut tile_addr =
                                base_addr + ty * 8 * row_stride + tx * 8 * col_stride;
                            for py in 0..8 {
                                let mut addr = tile_addr;
                                for px in 0..8 {
                                    let color_idx = tile.pixels[py][px];
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
    show_grid: bool,
    grid_alpha: f32,
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
        if !self.selecting_active && !self.show_grid {
            return vec![];
        }

        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let pixel_size_x =
            self.pixel_size * bounds.size().width / (bounds.size().width - self.pixel_size);
        let pixel_size_y =
            self.pixel_size * bounds.size().height / (bounds.size().height - self.pixel_size);
        if self.show_grid {
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
        }
        vec![frame.into_geometry()]
    }
}

pub fn area_grid_view(state: &EditorState, position: AreaPosition) -> Element<Message> {
    let area = state.area(position);
    let num_cols = area.size.1 * 32;
    let num_rows = area.size.0 * 32;
    let pixel_size = state.global_config.pixel_size;

    let mut left = 0;
    let mut right = 0;
    let mut top = 0;
    let mut bottom = 0;

    match (state.start_coords, state.end_coords) {
        (Some(p0), Some(p1)) => {
            left = p0.0.min(p1.0);
            right = p0.0.max(p1.0);
            top = p0.1.min(p1.1);
            bottom = p0.1.max(p1.1);
        }
        _ => {}
    }

    Scrollable::with_direction(
        column![stack![
            canvas(AreaGrid {
                position,
                area_id: state.area_id(position).clone(),
                area: &state.area(position),
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
                show_grid: state.show_grid,
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

pub fn main_area_controls(state: &EditorState) -> Element<Message> {
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
    ]
    .spacing(10)
    .clip(true)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

pub fn side_area_controls(state: &EditorState) -> Element<Message> {
    row![
        pick_list(
            state.area_names.clone(),
            Some(state.side_area().name.clone()),
            |x| Message::SelectArea(AreaPosition::Side, x)
        )
        .on_open(Message::Focus(Focus::PickArea(AreaPosition::Side)))
        .width(200),
        pick_list(
            state.theme_names.clone(),
            Some(state.main_area().theme.clone()),
            |x| Message::SelectTheme(AreaPosition::Side, x)
        )
        .on_open(Message::Focus(Focus::PickTheme(AreaPosition::Side)))
        .width(200),
    ]
    .spacing(10)
    .clip(true)
    .align_y(iced::alignment::Vertical::Center)
    .into()
}

pub fn add_area_view(name: &String, size: (u8, u8)) -> Element<Message> {
    let add_area_msg = Message::AddArea {
        name: name.clone(),
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

pub fn edit_area_view(state: &EditorState, name: &String) -> Element<'static, Message> {
    let old_name = state.main_area().name.clone();
    let rgb_width = 80;
    let edit_area_msg = Message::EditArea {
        old_name: old_name.clone(),
        new_name: name.clone(),
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
                    &state.main_area().bg_color[0],
                    0..=31,
                    Message::EditAreaBGRed
                )
                .width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Green"),
                number_input(
                    &state.main_area().bg_color[1],
                    0..=31,
                    Message::EditAreaBGGreen
                )
                .width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Blue"),
                number_input(
                    &state.main_area().bg_color[2],
                    0..=31,
                    Message::EditAreaBGBlue
                )
                .width(rgb_width),
            ]
            .spacing(5)
            .align_y(iced::alignment::Vertical::Center),
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

pub fn delete_area_view(state: &EditorState) -> Element<Message> {
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

pub fn add_theme_view(name: &String) -> Element<Message> {
    container(
        column![
            text("Add a new theme."),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("AddTheme")
                    .on_input(Message::SetAddThemeName)
                    .on_submit(Message::AddTheme(name.clone()))
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            button(text("Add theme"))
                .style(button::success)
                .on_press(Message::AddTheme(name.clone())),
        ]
        .spacing(10),
    )
    .width(450)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn rename_theme_view(state: &EditorState, name: &String) -> Element<'static, Message> {
    let old_name = state.main_area().theme.clone();
    let rename_msg = Message::RenameTheme {
        old_name: old_name.clone(),
        new_name: name.clone(),
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

pub fn delete_theme_view(state: &EditorState) -> Element<Message> {
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
