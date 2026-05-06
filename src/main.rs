mod app;
mod audio;
mod event;
mod matrix;
mod secrets;
mod sounds;
mod message;
mod modal;
mod ui;
mod view;

use std::io;

fn main() -> io::Result<()> {
    let mut terminal = ratatui::init();
    let mut app = app::App::new();
    let result = app.run(&mut terminal);
    ratatui::restore();
    result
}
