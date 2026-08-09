use hashbrown::HashMap;
use iced::{
    mouse,
    widget::{
        button, canvas, column, container, horizontal_space, pick_list, row, scrollable, text,
        tooltip,
    },
    Element, Length, Point, Rectangle, Size,
};

use crate::{
    helpers::scale_color,
    message::{Message, SelectionSource},
    state::{
        DynamicTileGrid, DynamicTileTarget, DynamicTileType, EditorState, Focus, Palette,
        PaletteId, TileBlock, TileCoord, Tool,
    },
};

const PIXEL_SIZE: f32 = 3.0;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
struct FrameChoice(usize);

impl std::fmt::Display for FrameChoice {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Frame {}", self.0 + 1)
    }
}

#[derive(Default)]
struct WarningCounts {
    empty: usize,
    broken: usize,
    property: usize,
}

fn get_warning_counts(
    kind: DynamicTileType,
    target: DynamicTileTarget,
    grid: &DynamicTileGrid,
    palettes: &[Palette],
    palette_indices: &HashMap<PaletteId, usize>,
) -> WarningCounts {
    let mut counts = WarningCounts::default();
    for (y, row) in grid.tiles.iter().enumerate() {
        for (x, placement) in row.iter().enumerate() {
            let Some(placement) = placement else {
                counts.empty += 1;
                continue;
            };
            let Some(&palette_idx) = palette_indices.get(&placement.palette) else {
                counts.broken += 1;
                continue;
            };
            let Some(tile) = palettes[palette_idx].tiles.get(placement.tile as usize) else {
                counts.broken += 1;
                continue;
            };
            if target == DynamicTileTarget::Before {
                if let Some(expected) = kind.expected_property() {
                    if kind.expects_property_at(x, y, row.len(), grid.tiles.len())
                        && tile.collision != expected
                    {
                        counts.property += 1;
                    }
                }
            }
        }
    }
    counts
}

#[derive(Clone, Copy, Default, PartialEq, Eq)]
enum Action {
    #[default]
    None,
    Selecting,
    Brushing,
}

#[derive(Clone, Copy, Default)]
struct GridState {
    action: Action,
}

struct DynamicGrid<'a> {
    kind: DynamicTileType,
    variant: usize,
    target: DynamicTileTarget,
    grid: &'a DynamicTileGrid,
    palettes: &'a [Palette],
    palette_indices: &'a HashMap<PaletteId, usize>,
    selected: &'a TileBlock,
    selection_source: SelectionSource,
    start_coords: Option<(TileCoord, TileCoord)>,
    end_coords: Option<(TileCoord, TileCoord)>,
    tool: Tool,
}

impl DynamicGrid<'_> {
    fn get_coords(&self, point: Point, bounds: Rectangle) -> Point<TileCoord> {
        let width = self.grid.tiles[0].len() as TileCoord;
        let height = self.grid.tiles.len() as TileCoord;
        let x = ((point.x - bounds.x).max(0.0) / (8.0 * PIXEL_SIZE)) as TileCoord;
        let y = ((point.y - bounds.y).max(0.0) / (8.0 * PIXEL_SIZE)) as TileCoord;
        Point::new(x.min(width - 1), y.min(height - 1))
    }

    fn get_brush_message(&self, coords: Point<TileCoord>) -> Message {
        Message::DynamicTileBrush {
            kind: self.kind,
            variant: self.variant,
            target: self.target,
            coords,
            selection: self.selected.clone(),
        }
    }
}

impl canvas::Program<Message> for DynamicGrid<'_> {
    type State = GridState;

    fn update(
        &self,
        state: &mut Self::State,
        event: canvas::Event,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if let canvas::Event::Mouse(event) = event {
            match event {
                mouse::Event::ButtonPressed(button @ (mouse::Button::Left | mouse::Button::Right)) => {
                    let Some(point) = cursor.position_over(bounds) else {
                        return (canvas::event::Status::Ignored, None);
                    };
                    let coords = self.get_coords(point, bounds);
                    match (self.tool, button) {
                        (Tool::Brush, mouse::Button::Left) => {
                            state.action = Action::Brushing;
                            return (
                                canvas::event::Status::Captured,
                                Some(self.get_brush_message(coords)),
                            );
                        }
                        (Tool::Select, mouse::Button::Left | mouse::Button::Right)
                        | (Tool::Brush, mouse::Button::Right) => {
                            state.action = Action::Selecting;
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::StartTileSelection(
                                    coords,
                                    SelectionSource::DynamicTiles {
                                        kind: self.kind,
                                        variant: self.variant,
                                        target: self.target,
                                    },
                                )),
                            );
                        }
                        _ => {}
                    }
                }
                mouse::Event::ButtonReleased(mouse::Button::Left | mouse::Button::Right) => {
                    let action = state.action;
                    state.action = Action::None;
                    if action == Action::Selecting {
                        if let Some(point) = cursor.position() {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::EndTileSelection(self.get_coords(point, bounds))),
                            );
                        }
                    }
                }
                mouse::Event::CursorMoved { .. } => {
                    let Some(point) = cursor.position() else {
                        return (canvas::event::Status::Ignored, None);
                    };
                    let coords = self.get_coords(point, bounds);
                    match state.action {
                        Action::Selecting => {
                            return (
                                canvas::event::Status::Captured,
                                Some(Message::ProgressTileSelection(coords)),
                            );
                        }
                        Action::Brushing => {
                            return (
                                canvas::event::Status::Captured,
                                Some(self.get_brush_message(coords)),
                            );
                        }
                        Action::None => {}
                    }
                }
                _ => {}
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
        let mut frame = canvas::Frame::new(renderer, bounds.size());
        let tile_size = 8.0 * PIXEL_SIZE;
        let expected_property = self.kind.expected_property();

        for (y, row) in self.grid.tiles.iter().enumerate() {
            for (x, placement) in row.iter().enumerate() {
                let origin = Point::new(x as f32 * tile_size, y as f32 * tile_size);
                let mut broken = false;
                let mut property_mismatch = false;
                if let Some(placement) = placement {
                    if let Some(&palette_idx) = self.palette_indices.get(&placement.palette) {
                        if let Some(tile) = self.palettes[palette_idx].tiles.get(placement.tile as usize) {
                            let tile = placement.flip.apply_to_tile(*tile);
                            for py in 0..8 {
                                for px in 0..8 {
                                    let color = self.palettes[palette_idx].colors
                                        [tile.pixels[py][px] as usize];
                                    frame.fill_rectangle(
                                        Point::new(
                                            origin.x + px as f32 * PIXEL_SIZE,
                                            origin.y + py as f32 * PIXEL_SIZE,
                                        ),
                                        Size::new(PIXEL_SIZE, PIXEL_SIZE),
                                        iced::Color::from_rgb8(
                                            scale_color(color[0]),
                                            scale_color(color[1]),
                                            scale_color(color[2]),
                                        ),
                                    );
                                }
                            }
                            if self.target == DynamicTileTarget::Before {
                                if let Some(expected) = expected_property {
                                    property_mismatch = self.kind.expects_property_at(
                                        x,
                                        y,
                                        row.len(),
                                        self.grid.tiles.len(),
                                    )
                                        && tile.collision != expected;
                                }
                            }
                        } else {
                            broken = true;
                        }
                    } else {
                        broken = true;
                    }
                } else {
                    for cy in 0..4 {
                        for cx in 0..4 {
                            let color = if (cx + cy) % 2 == 0 {
                                iced::Color::from_rgb8(75, 75, 75)
                            } else {
                                iced::Color::from_rgb8(105, 105, 105)
                            };
                            frame.fill_rectangle(
                                Point::new(
                                    origin.x + cx as f32 * tile_size / 4.0,
                                    origin.y + cy as f32 * tile_size / 4.0,
                                ),
                                Size::new(tile_size / 4.0, tile_size / 4.0),
                                color,
                            );
                        }
                    }
                }

                if broken || property_mismatch {
                    let outline = if broken {
                        iced::Color::from_rgb8(255, 0, 0)
                    } else {
                        iced::Color::from_rgb8(255, 170, 0)
                    };
                    frame.stroke(
                        &canvas::Path::rectangle(origin, Size::new(tile_size, tile_size)),
                        canvas::Stroke {
                            style: canvas::stroke::Style::Solid(outline),
                            width: 2.0,
                            ..Default::default()
                        },
                    );
                }
            }
        }

        if self.selection_source
            == (SelectionSource::DynamicTiles {
            kind: self.kind,
            variant: self.variant,
            target: self.target,
        }) {
            if let (Some(start), Some(end)) = (self.start_coords, self.end_coords) {
                let left = start.0.min(end.0) as f32 * tile_size;
                let top = start.1.min(end.1) as f32 * tile_size;
                let width = (start.0.max(end.0) - start.0.min(end.0) + 1) as f32 * tile_size;
                let height = (start.1.max(end.1) - start.1.min(end.1) + 1) as f32 * tile_size;
                frame.stroke(
                    &canvas::Path::rectangle(Point::new(left, top), Size::new(width, height)),
                    canvas::Stroke {
                        style: canvas::stroke::Style::Solid(iced::Color::from_rgb8(0, 255, 0)),
                        width: 2.0,
                        ..Default::default()
                    },
                );
            }
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _state: &Self::State,
        bounds: Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if cursor.is_over(bounds) && self.tool == Tool::Brush {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

fn grid_view<'a>(
    state: &'a EditorState,
    variant: usize,
    grid: &'a DynamicTileGrid,
    target: DynamicTileTarget,
) -> Element<'a, Message> {
    let kind = state.dynamic_tile_type;
    let (width, height) = kind.size();
    canvas(DynamicGrid {
        kind,
        variant,
        target,
        grid,
        palettes: &state.palettes,
        palette_indices: &state.palettes_id_idx_map,
        selected: &state.selected_tile_block,
        selection_source: state.selection_source,
        start_coords: state.start_coords,
        end_coords: state.end_coords,
        tool: state.tool,
    })
    .width(width as f32 * 8.0 * PIXEL_SIZE)
    .height(height as f32 * 8.0 * PIXEL_SIZE)
    .into()
}

fn warning_view<'a>(
    state: &'a EditorState,
    grid: &'a DynamicTileGrid,
    target: DynamicTileTarget,
) -> Element<'a, Message> {
    let counts = get_warning_counts(
        state.dynamic_tile_type,
        target,
        grid,
        &state.palettes,
        &state.palettes_id_idx_map,
    );
    let mut warnings = vec![];
    if counts.empty > 0 {
        warnings.push(format!("{} unset", counts.empty));
    }
    if counts.broken > 0 {
        warnings.push(format!("{} missing", counts.broken));
    }
    if counts.property > 0 {
        let expected = state.dynamic_tile_type.expected_property().unwrap();
        warnings.push(format!(
            "{} expected property ${:02X}",
            counts.property, expected
        ));
    }
    text(warnings.join(", ")).size(12).into()
}

pub fn dynamic_tiles_view(state: &EditorState) -> Element<'_, Message> {
    let kind = state.dynamic_tile_type;
    let group = state
        .dynamic_tiles
        .groups
        .iter()
        .find(|group| group.kind == kind);

    let close_button = tooltip(
        button(text("×").size(22)).on_press(Message::CloseDynamicTiles),
        "Close dynamic tile editor",
        tooltip::Position::Bottom,
    );
    let mut content = column![
        row![text("Dynamic tiles").size(20), horizontal_space(), close_button]
            .align_y(iced::alignment::Vertical::Center),
        row![
            text("Type"),
            pick_list(
                DynamicTileType::ALL.to_vec(),
                Some(kind),
                Message::SelectDynamicTileType
            )
            .on_open(Message::Focus(Focus::PickDynamicTileType))
            .width(Length::Fill),
        ]
        .spacing(8)
        .align_y(iced::alignment::Vertical::Center),
        row![
            text("Variants"),
            horizontal_space(),
            button("+")
                .style(button::success)
                .on_press(Message::AddDynamicTileVariant),
        ]
        .align_y(iced::alignment::Vertical::Center),
    ]
    .spacing(10);

    let mut variants = column![].spacing(8);
    if let Some(group) = group {
        let grid_width = kind.size().0 as f32 * 8.0 * PIXEL_SIZE;
        let column_width = grid_width.max(64.0);
        if !group.variants.is_empty() {
            variants = variants.push(
                row![
                    row![
                        container(text("Before"))
                            .width(column_width)
                            .align_x(iced::alignment::Horizontal::Center),
                        container(text("After"))
                            .width(column_width)
                            .align_x(iced::alignment::Horizontal::Center),
                    ]
                    .spacing(16),
                    horizontal_space(),
                ]
                .spacing(8)
            );
        }
        for (variant_idx, variant) in group.variants.iter().enumerate() {
            let frame = state
                .dynamic_tile_frames
                .get(variant_idx)
                .copied()
                .unwrap_or(0)
                .min(variant.after_frames.len() - 1);
            let frame_choices: Vec<FrameChoice> =
                (0..variant.after_frames.len()).map(FrameChoice).collect();
            let before = column![
                grid_view(
                    state,
                    variant_idx,
                    &variant.before,
                    DynamicTileTarget::Before
                ),
                warning_view(state, &variant.before, DynamicTileTarget::Before),
            ]
            .spacing(5)
            .width(column_width)
            .align_x(iced::alignment::Horizontal::Center);

            let after = column![
                grid_view(
                    state,
                    variant_idx,
                    &variant.after_frames[frame],
                    DynamicTileTarget::After(frame)
                ),
                warning_view(
                    state,
                    &variant.after_frames[frame],
                    DynamicTileTarget::After(frame)
                ),
            ]
            .spacing(5)
            .width(column_width)
            .align_x(iced::alignment::Horizontal::Center);

            let mut variant_row =
                row![row![before, after].spacing(16), horizontal_space()].spacing(8);
            if frame_choices.len() > 1 {
                variant_row = variant_row.push(
                    pick_list(frame_choices, Some(FrameChoice(frame)), move |choice| {
                        Message::SelectDynamicTileFrame {
                            variant: variant_idx,
                            frame: choice.0,
                        }
                    })
                    .width(90),
                );
            }
            variant_row = variant_row.push(
                button(text("\u{F63B}").font(iced_fonts::BOOTSTRAP_FONT))
                    .style(button::danger)
                    .on_press(Message::DeleteDynamicTileVariant(variant_idx)),
            );
            variants = variants.push(variant_row.align_y(iced::alignment::Vertical::Center));
        }
    }
    if group.is_none_or(|group| group.variants.is_empty()) {
        variants = variants.push(text("Add a variant to begin."));
    }
    content = content.push(scrollable(variants).spacing(8).height(Length::Fill));

    container(content)
        .padding(12)
        .width(480)
        .height(Length::Fill)
        .into()
}
