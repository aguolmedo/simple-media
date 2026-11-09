pub mod home;
pub mod playing;
pub mod series;
pub mod streams;
mod widgets;

use crossterm::event::{KeyEvent, MouseEvent};
use ratatui::Frame;

use crate::app::{Ctx, Msg};
use crate::catalog::Title;

pub enum Action {
    None,
    Push(Box<Screen>),
    Replace(Box<Screen>),
    Pop,
    Quit,
}

pub enum Screen {
    Home(home::Home),
    Series(series::Series),
    Streams(streams::Streams),
    Playing(Box<playing::Playing>),
}

impl Screen {
    pub fn start(&mut self, ctx: &mut Ctx) {
        match self {
            Screen::Home(s) => s.start(ctx),
            Screen::Series(s) => s.start(ctx),
            Screen::Streams(s) => s.start(ctx),
            Screen::Playing(s) => s.start(ctx),
        }
    }

    pub fn stop(&mut self, ctx: &mut Ctx) {
        if let Screen::Playing(s) = self {
            s.stop(ctx);
        }
    }

    pub fn resume(&mut self, ctx: &mut Ctx) {
        match self {
            Screen::Home(s) => s.resume(ctx),
            Screen::Series(s) => s.resume(ctx),
            Screen::Streams(s) => s.resume(ctx),
            Screen::Playing(_) => {}
        }
    }

    pub fn title(&self) -> Option<Title> {
        match self {
            Screen::Home(s) => s.selected_title(),
            Screen::Series(s) => Some(s.title().clone()),
            Screen::Streams(s) => Some(s.title().clone()),
            Screen::Playing(s) => Some(s.title().clone()),
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        match self {
            Screen::Home(s) => s.on_key(key, ctx),
            Screen::Series(s) => s.on_key(key),
            Screen::Streams(s) => s.on_key(key, ctx),
            Screen::Playing(s) => s.on_key(key),
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent, ctx: &mut Ctx) -> Action {
        match self {
            Screen::Home(s) => s.on_mouse(mouse, ctx),
            Screen::Series(s) => s.on_mouse(mouse),
            Screen::Streams(s) => s.on_mouse(mouse),
            Screen::Playing(s) => s.on_mouse(mouse),
        }
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Result<Action, Msg> {
        match self {
            Screen::Home(s) => s.on_msg(msg, ctx),
            Screen::Series(s) => s.on_msg(msg),
            Screen::Streams(s) => s.on_msg(msg),
            Screen::Playing(s) => s.on_msg(msg, ctx),
        }
    }

    pub fn on_tick(&mut self, ctx: &mut Ctx) {
        match self {
            Screen::Home(s) => s.on_tick(ctx),
            Screen::Playing(s) => s.on_tick(),
            Screen::Series(_) | Screen::Streams(_) => {}
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        match self {
            Screen::Home(s) => s.render(frame),
            Screen::Series(s) => s.render(frame),
            Screen::Streams(s) => s.render(frame),
            Screen::Playing(s) => s.render(frame),
        }
    }
}
