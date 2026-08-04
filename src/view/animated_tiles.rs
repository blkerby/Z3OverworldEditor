use iced::{
    mouse,
    widget::{
        button, canvas, column, container, horizontal_space, pick_list, row,
        scrollable::{Direction, Scrollbar},
        text, Scrollable,
    },
    Element, Point, Rectangle, Size,
};
use iced_aw::number_input;

use crate::{
    helpers::{alpha_blend, scale_color},
    message::Message,
    state::{
        AnimatedTileGroup, EditorState, Palette, PixelTarget, Tile, TileIdx, TilePixels,
    },
};

use super::{
    graphics::pixel_editor, modal_background_style, palette::palette_colors_view,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct GroupChoice(TileIdx);

impl std::fmt::Display for GroupChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "${:02X}–${:02X}", self.0, self.0 + 15)
    }
}

fn animated_pixels(
    palette: &Palette,
    group: &AnimatedTileGroup,
    frame: usize,
    tile: usize,
) -> TilePixels {
    if frame == 0 {
        palette.tiles[group.base_tile as usize + tile].pixels
    } else {
        group.frames[frame - 1][tile]
    }
}

fn grid_size(frame_count: usize, pixel_size: f32) -> Size {
    Size::new(16.0 * 8.0 * pixel_size, frame_count as f32 * 8.0 * pixel_size)
}

struct AnimatedTileGrid<'a> {
    palette: &'a Palette,
    group: &'a AnimatedTileGroup,
    selected_frame: usize,
    selected_tile: usize,
    pixel_size: f32,
}

impl canvas::Program<Message> for AnimatedTileGrid<'_> {
    type State = ();

    fn update(
        &self,
        _state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(mouse::Event::ButtonPressed(mouse::Button::Left)) = event {
            if let Some(point) = cursor.position_in(bounds) {
                let tile = (point.x / (8.0 * self.pixel_size)) as usize;
                let frame = (point.y / (8.0 * self.pixel_size)) as usize;
                if tile < 16 && frame <= self.group.frames.len() {
                    return (
                        canvas::event::Status::Captured,
                        Some(Message::SelectAnimatedTile {
                            base_tile: self.group.base_tile,
                            frame,
                            tile,
                        }),
                    );
                }
            }
        }
        (canvas::event::Status::Ignored, None)
    }

    fn draw(
        &self,
        _state: &Self::State,
        renderer: &iced::Renderer,
        _theme: &iced::Theme,
        bounds: Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut canvas_frame = canvas::Frame::new(renderer, bounds.size());
        let colors: Vec<[u8; 3]> = self
            .palette
            .colors
            .iter()
            .map(|&[r, g, b]| [scale_color(r), scale_color(g), scale_color(b)])
            .collect();
        let frame_count = self.group.frames.len() + 1;
        let image_size = grid_size(frame_count, self.pixel_size);
        let mut data = Vec::with_capacity(frame_count * 16 * 64 * 4);
        for y in 0..frame_count * 8 {
            for x in 0..16 * 8 {
                let frame = y / 8;
                let tile = x / 8;
                let pixels = animated_pixels(self.palette, self.group, frame, tile);
                let mut color = colors[pixels[y % 8][x % 8] as usize];
                if frame == self.selected_frame && tile == self.selected_tile {
                    color = alpha_blend(color, [255, 105, 180], 0.25);
                }
                data.extend(color);
                data.push(255);
            }
        }
        let image = iced::advanced::image::Image::new(
            iced::advanced::image::Handle::from_rgba(
                16 * 8,
                (frame_count * 8) as u32,
                data,
            ),
        )
        .filter_method(iced::widget::image::FilterMethod::Nearest)
        .snap(true);
        canvas_frame.draw_image(
            Rectangle::new(Point::ORIGIN, image_size),
            image,
        );
        vec![canvas_frame.into_geometry()]
    }
}

pub fn animated_tiles_view(
    state: &EditorState,
    selected_base: Option<TileIdx>,
    selected_frame: usize,
    selected_tile: usize,
) -> Element<'_, Message> {
    let palette = &state.palettes[state.palette_idx];
    let palette_id = palette.id;
    let choices: Vec<GroupChoice> = palette
        .animated_tile_groups
        .iter()
        .map(|group| GroupChoice(group.base_tile))
        .collect();
    let selected_choice = selected_base.map(GroupChoice);
    let selected_row = state.tile_idx.map(|tile| tile / 16 * 16);
    let add_message = selected_row.and_then(|base_tile| {
        let start = base_tile as usize;
        if start + 16 > palette.tiles.len()
            || palette
                .animated_tile_groups
                .iter()
                .any(|group| group.base_tile == base_tile)
        {
            return None;
        }
        let frame = std::array::from_fn(|tile| palette.tiles[start + tile].pixels);
        Some(Message::AddAnimatedTileGroup {
            palette_id,
            group: AnimatedTileGroup {
                base_tile,
                frames: vec![frame],
                frame_hold: 1,
                phase_offset: 0,
            },
        })
    });

    let mut content = column![
        row![
            text("Animated tile groups"),
            horizontal_space(),
            button(text("Close")).on_press(Message::CloseDialogue),
        ]
        .align_y(iced::alignment::Vertical::Center),
        row![
            text("Group"),
            pick_list(choices, selected_choice, |choice| {
                Message::SelectAnimatedGroup(choice.0)
            })
            .width(140),
            button(text("Add selected row"))
                .style(button::success)
                .on_press_maybe(add_message),
        ]
        .spacing(10)
        .align_y(iced::alignment::Vertical::Center),
    ]
    .spacing(10);

    if let Some(base_tile) = selected_base {
        if let Some(group) = palette
            .animated_tile_groups
            .iter()
            .find(|group| group.base_tile == base_tile)
        {
            let frame_count = (group.frames.len() + 1) as u16;
            let resize_group = move |new_count: u16| {
                Message::SetAnimatedFrameCount {
                    palette_id,
                    base_tile,
                    frame_count: new_count,
                }
            };
            let set_hold = move |frame_hold| {
                Message::SetAnimatedFrameHold {
                    palette_id,
                    base_tile,
                    frame_hold,
                }
            };
            let set_phase = move |phase_offset| {
                Message::SetAnimatedPhaseOffset {
                    palette_id,
                    base_tile,
                    phase_offset,
                }
            };
            let frame = selected_frame.min(group.frames.len());
            let tile_col = selected_tile.min(15);
            let pixels = animated_pixels(palette, group, frame, tile_col);
            let tile = if frame == 0 {
                palette.tiles[base_tile as usize + tile_col]
            } else {
                Tile {
                    pixels,
                    ..Tile::default()
                }
            };
            let target = if frame == 0 {
                PixelTarget::Regular(base_tile + tile_col as TileIdx)
            } else {
                PixelTarget::Animated {
                    tile_idx: base_tile + tile_col as TileIdx,
                    frame: frame - 1,
                }
            };
            let grid_height = grid_size(group.frames.len() + 1, 3.0).height;

            content = content
                .push(
                    row![
                        text("Frames"),
                        number_input(&frame_count, 2..=255, resize_group).width(70),
                        text("Hold"),
                        number_input(&group.frame_hold, 1..=u16::MAX, set_hold).width(90),
                        text("Phase"),
                        number_input(&group.phase_offset, 0..=u16::MAX, set_phase).width(90),
                        horizontal_space(),
                        button(text("Delete group"))
                            .style(button::danger)
                            .on_press(Message::DeleteAnimatedTileGroup {
                                palette_id,
                                base_tile,
                            }),
                    ]
                    .spacing(8)
                    .align_y(iced::alignment::Vertical::Center),
                )
                .push(palette_colors_view(state))
                .push(text(format!("Frame {}, column {}", frame, tile_col)))
                .push(
                    row![
                        Scrollable::with_direction(
                            canvas(AnimatedTileGrid {
                                palette,
                                group,
                                selected_frame: frame,
                                selected_tile: tile_col,
                                pixel_size: 3.0,
                            })
                            .width(384)
                            .height(grid_height),
                            Direction::Vertical(Scrollbar::default()),
                        )
                        .width(410)
                        .height(360),
                        pixel_editor(state, tile, target, false),
                    ]
                    .spacing(15),
                );
        }
    } else {
        content = content.push(text(
            "Select a tile in the main grid, then add its row as an animated group.",
        ));
    }

    container(content)
        .width(850)
        .padding(20)
        .style(modal_background_style)
        .into()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn animated_grid_keeps_tiles_square() {
        assert_eq!(grid_size(2, 3.0), Size::new(384.0, 48.0));
    }
}
