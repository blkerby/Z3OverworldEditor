mod area;
mod graphics;
mod palette;
mod settings;
mod tiles;

use std::path::PathBuf;

use area::{
    add_area_view, add_theme_view, area_grid_view, delete_area_view, delete_theme_view,
    edit_area_view, main_area_controls, rename_theme_view, side_area_controls,
};
use graphics::graphics_view;
use iced::{
    alignment::Vertical,
    widget::{
        button, center, column, container, horizontal_space, mouse_area, opaque, responsive, row,
        stack, text, Column, Space,
    },
    Element, Font, Length, Theme,
};
use iced_aw::quad;
use palette::{
    add_palette_view, delete_palette_view, rename_palette_view, selected_palette_view,
    used_palettes_view,
};
use settings::{import_rom_confirm_view, import_rom_progress_view, settings_view};
use tiles::{move_tiles_view, moving_tiles_progress_view, tile_view};

use crate::{
    message::Message,
    state::{AreaPosition, Dialogue, EditorState, SidePanelView},
};

pub async fn open_project() -> Option<PathBuf> {
    let picked_dir = rfd::AsyncFileDialog::new()
        .set_title("Select new or existing project folder ...")
        .pick_folder()
        .await;
    picked_dir.map(|x| x.path().to_owned())
}

pub async fn open_rom() -> Option<PathBuf> {
    let picked_dir = rfd::AsyncFileDialog::new()
        .set_title("Select a ROM ...")
        .add_filter("SNES ROM", &["sfc", "smc"])
        .pick_file()
        .await;
    picked_dir.map(|x| x.path().to_owned())
}

fn modal<'a, Message>(
    base: impl Into<Element<'a, Message>>,
    content: impl Into<Element<'a, Message>>,
    on_blur: Message,
) -> Element<'a, Message>
where
    Message: Clone + 'a,
{
    stack![
        base.into(),
        opaque(
            mouse_area(center(opaque(content)).style(|_theme| {
                container::Style {
                    background: Some(
                        iced::Color {
                            a: 0.5,
                            ..iced::Color::BLACK
                        }
                        .into(),
                    ),
                    ..container::Style::default()
                }
            }))
            .on_press(on_blur)
        )
    ]
    .into()
}

pub fn modal_background_style(theme: &Theme) -> container::Style {
    let palette = theme.extended_palette();
    container::Style {
        background: Some(palette.background.base.color.into()),
        border: iced::border::rounded(4)
            .color(palette.background.weak.color)
            .width(1.0),
        ..container::Style::default()
    }
}

fn vertical_separator() -> quad::Quad {
    quad::Quad {
        quad_color: iced::Color::from([0.5; 3]).into(),
        quad_border: iced::Border {
            radius: iced::border::Radius::new(1.0),
            ..Default::default()
        },
        inner_bounds: iced_aw::widget::InnerBounds::Ratio(1.0, 1.0),
        width: Length::Fixed(1.0),
        ..Default::default()
    }
}

pub fn help_view(_state: &EditorState) -> Element<Message> {
    let controls = vec![
        ("s", "Select tool", "copy tiles, colors, pixels"),
        ("b", "Brush tool", "paste tiles, colors, pixels"),
        (
            "m",
            "Move tool",
            "move tiles across palettes (replacing all uses)",
        ),
        ("g", "Grid view toggle", "show/hide 16x16 tile grid"),
        ("p", "Grid snap toggle", "snapping to 16x16 tile grid"),
        ("h", "Horizontal flip", "flip selection horizontally"),
        ("v", "Vertical flip", "flip selection horizontally"),
        ("t", "Tileset view", "show palettes/tilesets in side panel"),
        ("a", "Area view", "show secondary area in side panel"),
        ("-", "Zoom out", "zoom out area views"),
        ("=", "Zoom in", "zoom in area views"),
        ("Ctrl", "Identify", "highlight uses of selected tile/color"),
    ];
    let mut col = Column::new();
    col = col.push(text("Essential keyboard controls:"));
    for (key, name, desc) in controls {
        col = col.push(
            row![
                text(key).width(50).font(Font {
                    weight: iced::font::Weight::ExtraBold,
                    ..Default::default()
                }),
                text(format!("{}: {}", name, desc)).width(Length::Fill),
            ]
            .align_y(Vertical::Center),
        );
    }

    container(col.spacing(10))
        .width(550)
        .padding(25)
        .style(modal_background_style)
        .into()
}

pub fn rebuild_project_view(_state: &EditorState) -> Element<Message> {
    container(text(
        "Please wait while the project PNG files are exported.",
    ))
    .width(500)
    .padding(25)
    .style(modal_background_style)
    .into()
}

pub fn modified_reload_view(_state: &EditorState) -> Element<Message> {
    container(
        column![
            text("File changes detected on disk."),
            text("Please click 'Accept' to load these changes, to keep the editor in sync with the project data."),
            row![
                button(text("Accept"))
                    .style(button::success)
                    .on_press(Message::ModifiedReload),
                horizontal_space(),
                button(text("Reject"))
                    .style(button::danger)
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

pub fn view_dialogue<'a>(
    state: &'a EditorState,
    main_view: Element<'a, Message>,
) -> Element<'a, Message> {
    if let Some(dialogue) = &state.dialogue {
        match dialogue {
            Dialogue::Settings => modal(main_view, settings_view(state), Message::HideModal),
            Dialogue::AddPalette { name, id } => {
                modal(main_view, add_palette_view(name, *id), Message::HideModal)
            }
            Dialogue::DeletePalette => {
                modal(main_view, delete_palette_view(state), Message::HideModal)
            }
            Dialogue::RenamePalette { name } => modal(
                main_view,
                rename_palette_view(&state, name),
                Message::HideModal,
            ),
            Dialogue::AddArea { name, size } => {
                modal(main_view, add_area_view(name, *size), Message::HideModal)
            }
            Dialogue::EditArea { name } => {
                modal(main_view, edit_area_view(state, name), Message::HideModal)
            }
            Dialogue::DeleteArea => modal(main_view, delete_area_view(state), Message::HideModal),
            Dialogue::AddTheme { name } => {
                modal(main_view, add_theme_view(name), Message::HideModal)
            }
            Dialogue::RenameTheme { name } => modal(
                main_view,
                rename_theme_view(state, name),
                Message::HideModal,
            ),
            Dialogue::DeleteTheme => modal(main_view, delete_theme_view(state), Message::HideModal),
            Dialogue::ImportROMConfirm => modal(
                main_view,
                import_rom_confirm_view(state),
                Message::HideModal,
            ),
            Dialogue::ImportROMProgress => {
                modal(main_view, import_rom_progress_view(state), Message::Nothing)
            }
            Dialogue::Help => modal(main_view, help_view(state), Message::HideModal),
            Dialogue::RebuildProject => {
                modal(main_view, rebuild_project_view(state), Message::Nothing)
            }
            Dialogue::ModifiedReload => {
                modal(main_view, modified_reload_view(state), Message::Nothing)
            }
            Dialogue::MovingTilesProgress => modal(
                main_view,
                moving_tiles_progress_view(state),
                Message::Nothing,
            ),
            Dialogue::MoveTiles {
                src_selection,
                dst_selection,
            } => modal(
                main_view,
                move_tiles_view(state, src_selection, dst_selection),
                Message::Nothing,
            ),
        }
    } else {
        main_view
    }
}

pub fn view(state: &EditorState) -> Element<Message> {
    if state.global_config.project_dir.is_none() {
        return Space::new(Length::Fill, Length::Fill).into();
    }

    let main_panel: Element<Message> = column![
        row![
            button(text("\u{F3E2}").font(iced_fonts::BOOTSTRAP_FONT))
                .style(button::secondary)
                .on_press(Message::SettingsDialogue),
            main_area_controls(state),
            horizontal_space(),
            button(text("\u{F505}").font(iced_fonts::BOOTSTRAP_FONT))
                .style(button::secondary)
                .on_press(Message::HelpDialogue),
        ]
        .spacing(10),
        area_grid_view(state, AreaPosition::Main),
    ]
    .padding(10)
    .spacing(10)
    .into();

    let side_panel: Element<Message> = match state.side_panel_view {
        SidePanelView::Tileset => column![
            used_palettes_view(state),
            selected_palette_view(state),
            responsive(
                move |size| column![tile_view(state, size, 260.0), graphics_view(state)].into()
            )
        ]
        .width(420)
        .into(),
        SidePanelView::Area => column![
            side_area_controls(state),
            area_grid_view(state, AreaPosition::Side),
        ]
        .padding(10)
        .spacing(10)
        .width(420)
        .into(),
    };

    let mut main_view: Element<Message> = row![main_panel, vertical_separator(), side_panel,]
        .spacing(0)
        .width(Length::Fill)
        .height(Length::Fill)
        .into();

    main_view = view_dialogue(state, main_view);
    main_view
}
