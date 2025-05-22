// Module for displaying and editing the 16 colors of palettes
use iced::{
    alignment::Vertical,
    mouse,
    widget::{
        button, canvas, column, container, pick_list, row, text, text_input, Column, Row, Space,
    },
    Element, Length, Size,
};
use iced_aw::number_input;

use crate::{
    message::Message,
    state::{ColorIdx, ColorRGB, EditorState, Focus, PaletteId, PaletteIdx, Tool},
};

use super::modal_background_style;

#[derive(Debug)]
struct ColorBox {
    r: f32,
    g: f32,
    b: f32,
    thickness: f32,
    selected: bool,
    color_idx: ColorIdx,
    palette_id: PaletteId,
    palette_idx: PaletteIdx,
    selected_color: ColorRGB,
    tool: Tool,
}

impl canvas::Program<Message> for ColorBox {
    // No internal state
    type State = ();

    fn update(
        &self,
        _interaction: &mut Self::State,
        event: canvas::Event,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> (canvas::event::Status, Option<Message>) {
        if cursor.position_in(bounds).is_none() {
            return (canvas::event::Status::Ignored, None);
        };

        match event {
            canvas::Event::Mouse(mouse_event) => match mouse_event {
                mouse::Event::ButtonPressed(button) => {
                    let message = match button {
                        mouse::Button::Left => {
                            if self.tool == Tool::Brush {
                                Some(Message::BrushColor {
                                    palette_id: self.palette_id,
                                    color_idx: self.color_idx,
                                    color: self.selected_color,
                                })
                            } else {
                                Some(Message::SelectColor(self.palette_idx, self.color_idx))
                            }
                        }
                        mouse::Button::Right => None,
                        _ => None,
                    };

                    (canvas::event::Status::Captured, message)
                }
                _ => (canvas::event::Status::Ignored, None),
            },
            _ => (canvas::event::Status::Ignored, None),
        }
    }

    fn draw(
        &self,
        _state: &(),
        renderer: &iced::Renderer,
        theme: &iced::Theme,
        bounds: iced::Rectangle,
        _cursor: mouse::Cursor,
    ) -> Vec<canvas::Geometry> {
        let mut frame = canvas::Frame::new(renderer, bounds.size());

        let thickness = self.thickness;
        let size = Size {
            width: frame.size().width - 2.0 * thickness,
            height: frame.size().height - 2.0 * thickness - 1.0,
        };
        frame.fill_rectangle(
            iced::Point {
                x: thickness,
                y: thickness,
            },
            size,
            iced::Color::from_rgb(self.r, self.g, self.b),
        );

        if self.selected {
            let border_color = if theme.extended_palette().is_dark {
                iced::Color::WHITE
            } else {
                iced::Color::BLACK
            };
            let size = Size {
                width: frame.size().width - thickness,
                height: frame.size().height - thickness - 1.0,
            };
            frame.stroke_rectangle(
                iced::Point {
                    x: thickness / 2.0,
                    y: thickness / 2.0,
                },
                size,
                canvas::Stroke {
                    width: thickness,
                    style: border_color.into(),
                    ..Default::default()
                },
            );
        }

        vec![frame.into_geometry()]
    }

    fn mouse_interaction(
        &self,
        _interaction: &Self::State,
        bounds: iced::Rectangle,
        cursor: mouse::Cursor,
    ) -> mouse::Interaction {
        if self.tool == Tool::Brush && cursor.is_over(bounds) {
            mouse::Interaction::Crosshair
        } else {
            mouse::Interaction::default()
        }
    }
}

pub fn selected_palette_view(state: &EditorState) -> Element<Message> {
    let palette_names: Vec<String> = state
        .palettes
        .iter()
        .map(|x| format!("{}: {}", x.id, x.name))
        .collect();
    let pal = &state.palettes[state.palette_idx];
    let selected_palette_name = format!("{}: {}", pal.id, pal.name);

    let mut colors_row = iced::widget::Row::new();
    let pal = &state.palettes[state.palette_idx];
    let size = 25.0;
    for i in 0..16 {
        colors_row = colors_row.push(
            canvas(ColorBox {
                r: pal.colors[i][0] as f32 / 31.0,
                g: pal.colors[i][1] as f32 / 31.0,
                b: pal.colors[i][2] as f32 / 31.0,
                thickness: 2.0,
                selected: Some(i as ColorIdx) == state.color_idx,
                color_idx: i as ColorIdx,
                palette_id: state.palettes[state.palette_idx].id,
                palette_idx: state.palette_idx,
                selected_color: state.selected_color,
                tool: state.tool,
            })
            .width(size)
            .height(size),
        );
    }

    let rgb_width = 80;
    let mut col = column![
        row![
            text("Palette"),
            pick_list(
                palette_names,
                Some(selected_palette_name),
                Message::SelectPalette
            )
            .on_open(Message::Focus(Focus::PickPalette))
            .width(Length::Fill),
            button(text("\u{F64D}").font(iced_fonts::BOOTSTRAP_FONT))
                .style(button::success)
                .on_press(Message::AddPaletteDialogue),
            button(text("\u{F4CB}").font(iced_fonts::BOOTSTRAP_FONT))
                .on_press(Message::RenamePaletteDialogue),
        ]
        .spacing(10)
        .align_y(iced::alignment::Vertical::Center),
        colors_row,
    ]
    .spacing(5);

    if state.color_idx.is_some() {
        col = col.push(
            row![
                text("Red"),
                number_input(&state.selected_color[0], 0..=31, Message::ChangeRed).width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Green"),
                number_input(&state.selected_color[1], 0..=31, Message::ChangeGreen)
                    .width(rgb_width),
                iced::widget::Space::with_width(10),
                text("Blue"),
                number_input(&state.selected_color[2], 0..=31, Message::ChangeBlue)
                    .width(rgb_width),
            ]
            .spacing(5)
            .align_y(iced::alignment::Vertical::Center),
        );
    }

    row![col].padding(10).into()
}

pub fn add_palette_view(name: &String, id: PaletteId) -> Element<Message> {
    container(
        column![
            text("Select a name and ID for the new palette"),
            row![
                text("Name: ").width(70),
                text_input("", name)
                    .id("AddPalette")
                    .on_input(Message::SetAddPaletteName)
                    .on_submit(Message::AddPalette {
                        name: name.clone(),
                        id
                    })
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("ID: ").width(70),
                number_input(&id, 0..=255, Message::SetAddPaletteID)
                    .width(50)
                    .on_submit(Message::AddPalette {
                        name: name.clone(),
                        id
                    }),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            button(text("Add palette"))
                .style(button::success)
                .on_press(Message::AddPalette {
                    name: name.clone(),
                    id
                }),
        ]
        .spacing(10),
    )
    .width(350)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn rename_palette_view(state: &EditorState, name: &String) -> Element<'static, Message> {
    let idx = state.palette_idx;
    let old_name = &state.palettes[idx].name;
    let rename_msg = Message::RenamePalette {
        id: state.palettes[idx].id,
        name: name.clone(),
    };
    container(
        column![
            text(format!(
                "Rename palette {}: \"{}\"",
                state.palettes[idx].id, old_name
            )),
            text_input("", name)
                .id("RenamePalette")
                .on_input(Message::SetRenamePaletteName)
                .on_submit(rename_msg.clone())
                .padding(5),
            row![
                button(text("Rename palette")).on_press(rename_msg.clone()),
                Space::with_width(Length::Fill),
                button(text("Delete palette"))
                    .style(button::danger)
                    .on_press(Message::DeletePaletteDialogue),
            ],
        ]
        .spacing(10),
    )
    .width(350)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn delete_palette_view(state: &EditorState) -> Element<Message> {
    let idx = state.palette_idx;
    let name = &state.palettes[idx].name;
    container(
        column![
            text(format!(
                "Delete palette {}: \"{}\"?",
                state.palettes[idx].id, name
            )),
            text("This will also delete all 8x8 tiles associated to this palette."),
            button(text("Delete palette"))
                .style(button::danger)
                .on_press(Message::DeletePalette(state.palettes[idx].id)),
        ]
        .spacing(10),
    )
    .width(350)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn used_palettes_view(state: &EditorState) -> Element<Message> {
    let mut col: Column<Message> = Column::new();
    let palette_ids = state.main_area().get_unique_palettes();
    for pal_id in palette_ids {
        let Some(&palette_idx) = state.palettes_id_idx_map.get(&pal_id) else {
            col = col.push(row![text(format!("{} (does not exist)", pal_id))]);
            continue;
        };
        let mut row: Row<Message> = Row::new();
        let pal = &state.palettes[palette_idx as usize];
        row = row.push(text(pal.name.clone()).width(125));
        row = row.push(Space::with_width(5));

        let size = 18.0;
        for i in 0..16 {
            row = row.push(
                canvas(ColorBox {
                    r: pal.colors[i][0] as f32 / 31.0,
                    g: pal.colors[i][1] as f32 / 31.0,
                    b: pal.colors[i][2] as f32 / 31.0,
                    thickness: 1.0,
                    selected: state.palette_idx == palette_idx
                        && Some(i as ColorIdx) == state.color_idx,
                    color_idx: i as ColorIdx,
                    palette_id: state.palettes[palette_idx].id,
                    palette_idx,
                    selected_color: state.selected_color,
                    tool: state.tool,
                })
                .width(size)
                .height(size),
            );
        }
        col = col.push(row);
    }
    row![col].padding(10).into()
}
