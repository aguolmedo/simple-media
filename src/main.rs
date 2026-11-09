mod app;
mod archive;
mod blocks;
mod catalog;
mod downloads;
mod engine;
mod history;
mod http;
mod ipc;
mod library;
mod player;
mod poster;
mod release;
mod screens;
mod storage;
mod streams;
mod subtitles;
mod watchlist;

use std::time::Duration;

use anyhow::Result;
use crossterm::event::{DisableMouseCapture, EnableMouseCapture, EventStream};
use crossterm::terminal::{Clear, ClearType};
use futures::StreamExt;
use tokio::sync::mpsc;

use app::App;
use engine::Engine;

const TICK: Duration = Duration::from_millis(250);

async fn run(app: &mut App, rx: &mut mpsc::UnboundedReceiver<app::Msg>) -> Result<()> {
    let mut terminal = ratatui::init();
    crossterm::execute!(std::io::stdout(), Clear(ClearType::All), EnableMouseCapture)?;
    let mut events = EventStream::new();
    let mut tick = tokio::time::interval(TICK);
    while !app.quit {
        terminal.draw(|frame| app.render(frame))?;
        tokio::select! {
            Some(Ok(event)) = events.next() => app.on_event(event),
            Some(msg) = rx.recv() => app.on_msg(msg),
            _ = tick.tick() => app.on_tick(),
        }
    }
    Ok(())
}

#[tokio::main]
async fn main() -> Result<()> {
    let engine = Engine::start().await?;
    let (tx, mut rx) = mpsc::unbounded_channel();
    let mut app = App::new(tx, engine);
    let result = run(&mut app, &mut rx).await;
    let _ = crossterm::execute!(std::io::stdout(), DisableMouseCapture);
    ratatui::restore();
    app.shutdown().await;
    if let Err(err) = result {
        eprintln!("{err:#}");
        std::process::exit(1);
    }
    std::process::exit(0)
}
