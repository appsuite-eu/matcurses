// matrix-sdk 0.14's deeply-monomorphized async sync stack overflows
// rustc's default 128-deep type-resolution limit. Bump it crate-wide.
#![recursion_limit = "256"]

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
