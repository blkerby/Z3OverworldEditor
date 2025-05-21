use std::time::Duration;

use z3_overworld_editor::{message, state, update, view};

use anyhow::Result;
use iced::{Size, Subscription, Task, Theme};
use message::Message;
use state::EditorState;

fn theme(_state: &EditorState) -> Theme {
    match dark_light::detect().unwrap_or(dark_light::Mode::Unspecified) {
        dark_light::Mode::Light => Theme::Light,
        dark_light::Mode::Dark | dark_light::Mode::Unspecified => Theme::Dark,
    }
}

fn subscription(_state: &EditorState) -> Subscription<Message> {
    Subscription::batch(vec![
        iced::window::close_requests().map(Message::WindowClose),
        iced::time::every(Duration::from_secs(1)).map(|_| Message::SaveProject),
        iced::event::listen().map(Message::Event),
    ])
}

pub fn main() -> Result<()> {
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("z3_overworld_editor=info"),
    )
    .format_timestamp_millis()
    .init();

    std::env::set_var("RUST_BACKTRACE", "1");

    let editor_state = state::get_initial_state()?;
    let initial_task = match &editor_state.global_config.project_dir {
        None => Task::perform(view::open_project(), Message::ProjectOpened),
        Some(_) => Task::none(),
    };
    iced::application("Z3 Overworld Editor", update::update, view::view)
        .font(iced_fonts::REQUIRED_FONT_BYTES)
        .font(iced_fonts::BOOTSTRAP_FONT_BYTES)
        .theme(theme)
        .exit_on_close_request(false)
        .subscription(subscription)
        .window_size(Size {
            width: 1440.0,
            height: 960.0,
        })
        .run_with(|| (editor_state, initial_task))?;
    Ok(())
}
