use iced::{
    alignment::Vertical,
    widget::{button, column, container, horizontal_space, row, slider, text, text_input},
    Element, Length,
};
use iced_aw::number_input;
use iced_fonts::BOOTSTRAP_FONT;

use crate::{
    message::Message,
    state::{EditorState, MAX_PIXEL_SIZE, MIN_PIXEL_SIZE},
};

use super::modal_background_style;

pub fn settings_view(state: &EditorState) -> Element<'_, Message> {
    let project_dir = state.global_config.project_dir.as_ref().unwrap();
    let zoom_range = MIN_PIXEL_SIZE..=MAX_PIXEL_SIZE;
    container(
        column![
            row![
                text("Project path").width(100),
                text_input("", project_dir.to_str().unwrap()).width(Length::Fill),
                button(text("\u{F3D7}").font(BOOTSTRAP_FONT))
                    .style(button::secondary)
                    .on_press(Message::OpenProject),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Zoom").width(100),
                slider(
                    zoom_range.clone(),
                    state.global_config.pixel_size,
                    Message::SetPixelSize
                )
                .width(Length::Fill),
                number_input(
                    &state.global_config.pixel_size,
                    zoom_range.clone(),
                    Message::SetPixelSize
                )
                .width(60),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                text("Grid alpha").width(100),
                slider(
                    0.01..=1.0,
                    state.global_config.grid_alpha,
                    Message::SetGridAlpha
                )
                .step(0.01_f32)
                .width(Length::Fill),
                number_input(
                    &((state.global_config.grid_alpha * 100.0).round() as u8),
                    1..=100,
                    |x| { Message::SetGridAlpha(x as f32 / 100.0) }
                )
                .width(60),
            ]
            .spacing(10)
            .align_y(Vertical::Center),
            row![
                button("Close")
                    .style(button::secondary)
                    .on_press(Message::CloseDialogue),
                horizontal_space(),
                button("Import from ROM")
                    .style(button::danger)
                    .on_press(Message::ImportDialogue)
            ]
        ]
        .spacing(20),
    )
    .width(600)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn import_rom_confirm_view(_state: &EditorState) -> Element<'_, Message> {
    container(
        column![
            text("Import project from ROM?"),
            text("This may update existing palettes, tilesets, and areas."),
            text("This action cannot be undone."),
            row![
                button(text("Import from ROM"))
                    .style(button::danger)
                    .on_press(Message::ImportROMProgress),
                horizontal_space(),
                button(text("Cancel"))
                    .style(button::secondary)
                    .on_press(Message::CloseDialogue),
            ]
        ]
        .spacing(15),
    )
    .width(500)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn import_rom_progress_view(_state: &EditorState) -> Element<'_, Message> {
    container(text("Please wait while ROM is importing."))
        .width(350)
        .padding(25)
        .style(modal_background_style)
        .into()
}
