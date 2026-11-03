use std::sync::Arc;
use std::time::Duration;

use crossterm::event::{Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::Frame;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::catalog::{Details, Episode, Meta, Title};
use crate::downloads::{self, Downloads};
use crate::engine::{Engine, Playback};
use crate::history::History;
use crate::player::PlayerEvent;
use crate::poster::{self, Art, Picture};
use crate::screens::{Action, Screen, home::Home};
use crate::streams::Found;
use crate::subtitles::Subtitle;
use crate::watchlist::Watchlist;

const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

pub enum Msg {
    Titles(u64, Result<Vec<Title>, String>),
    Details(String, Option<Box<Details>>),
    Art(Art, String, Option<Picture>),
    Meta(u64, Result<Box<Meta>, String>),
    Streams(u64, Found),
    Ready(u64, Result<Arc<Playback>, String>),
    Player(u64, PlayerEvent),
    Subtitles(u64, Vec<Subtitle>),
    Next(u64, Option<Episode>),
    Download(String, downloads::Event),
    OnDisk(u64, Vec<Title>),
}

pub struct Pending {
    id: u64,
    handle: Option<JoinHandle<()>>,
}

impl Pending {
    pub fn is(&self, id: u64) -> bool {
        self.id == id
    }

    pub async fn cancel(mut self) {
        if let Some(handle) = self.handle.take() {
            handle.abort();
            let _ = handle.await;
        }
    }
}

impl Drop for Pending {
    fn drop(&mut self) {
        if let Some(handle) = &self.handle {
            handle.abort();
        }
    }
}

pub fn is_current(pending: &Option<Pending>, id: u64) -> bool {
    pending.as_ref().is_some_and(|p| p.is(id))
}

pub struct Ctx {
    pub tx: UnboundedSender<Msg>,
    pub engine: Engine,
    pub history: History,
    pub watchlist: Watchlist,
    pub downloads: Downloads,
    next_id: u64,
    cleanups: Vec<JoinHandle<()>>,
}

impl Ctx {
    pub fn new_id(&mut self) -> u64 {
        self.next_id += 1;
        self.next_id
    }

    pub fn request_art(&mut self, art: Art, title_id: &str) {
        let (id, tx) = (title_id.to_string(), self.tx.clone());
        tokio::spawn(async move {
            let picture = poster::fetch(art, &id).await.ok();
            let _ = tx.send(Msg::Art(art, id, picture));
        });
    }

    fn send_when_done<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
        wrap: fn(u64, T) -> Msg,
    ) -> (u64, JoinHandle<()>) {
        let id = self.new_id();
        let tx = self.tx.clone();
        let handle = tokio::spawn(async move {
            let _ = tx.send(wrap(id, work.await));
        });
        (id, handle)
    }

    pub fn request<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
        wrap: fn(u64, T) -> Msg,
    ) -> Pending {
        let (id, handle) = self.send_when_done(work, wrap);
        Pending { id, handle: Some(handle) }
    }

    pub fn request_detached<T: Send + 'static>(
        &mut self,
        work: impl Future<Output = T> + Send + 'static,
        wrap: fn(u64, T) -> Msg,
    ) -> u64 {
        self.send_when_done(work, wrap).0
    }

    pub fn clean_up(&mut self, work: impl Future<Output = ()> + Send + 'static) {
        self.cleanups.retain(|handle| !handle.is_finished());
        self.cleanups.push(tokio::spawn(work));
    }

    async fn finish_cleanups(&mut self) {
        let all = futures::future::join_all(self.cleanups.drain(..));
        let _ = tokio::time::timeout(SHUTDOWN_WAIT, all).await;
    }
}

pub struct App {
    stack: Vec<Screen>,
    ctx: Ctx,
    pub quit: bool,
}

impl App {
    pub fn new(tx: UnboundedSender<Msg>, engine: Engine) -> App {
        let mut ctx = Ctx {
            downloads: Downloads::load(engine.clone(), tx.clone()),
            tx,
            engine,
            history: History::load(),
            watchlist: Watchlist::load(),
            next_id: 0,
            cleanups: Vec::new(),
        };
        let mut home = Screen::Home(Home::default());
        home.start(&mut ctx);
        App { stack: vec![home], ctx, quit: false }
    }

    fn top(&mut self) -> &mut Screen {
        self.stack.last_mut().expect("there is always a screen")
    }

    fn push(&mut self, mut screen: Screen) {
        screen.start(&mut self.ctx);
        self.stack.push(screen);
    }

    fn close_top(&mut self) {
        if let Some(mut screen) = self.stack.pop() {
            screen.stop(&mut self.ctx);
        }
    }

    fn pop(&mut self) {
        if self.stack.len() == 1 {
            self.quit = true;
            return;
        }
        self.close_top();
        self.resume();
    }

    fn resume(&mut self) {
        let ctx = &mut self.ctx;
        self.stack.last_mut().expect("there is always a screen").resume(ctx);
    }

    fn apply(&mut self, action: Action) {
        match action {
            Action::None => {}
            Action::Push(screen) => self.push(*screen),
            Action::Replace(screen) => {
                self.close_top();
                self.push(*screen);
            }
            Action::Pop => self.pop(),
            Action::Quit => self.quit = true,
        }
    }

    pub async fn shutdown(mut self) {
        while !self.stack.is_empty() {
            self.close_top();
        }
        self.ctx.finish_cleanups().await;
    }

    pub fn on_event(&mut self, event: Event) {
        let action = match event {
            Event::Key(key) => self.on_key(key),
            Event::Mouse(mouse) => {
                let ctx = &mut self.ctx;
                self.stack.last_mut().expect("there is always a screen").on_mouse(mouse, ctx)
            }
            _ => Action::None,
        };
        self.apply(action);
    }

    fn on_key(&mut self, key: KeyEvent) -> Action {
        if key.kind != KeyEventKind::Press {
            return Action::None;
        }
        if is_ctrl(&key, 'c') {
            return Action::Quit;
        }
        if is_ctrl(&key, 's') {
            self.toggle_watchlist();
            return Action::None;
        }
        let ctx = &mut self.ctx;
        self.stack.last_mut().expect("there is always a screen").on_key(key, ctx)
    }

    fn toggle_watchlist(&mut self) {
        if let Some(title) = self.top().title() {
            self.ctx.watchlist.toggle(title);
            self.resume();
        }
    }

    pub fn on_msg(&mut self, msg: Msg) {
        if let Msg::Download(id, event) = msg {
            return self.ctx.downloads.on_event(&id, event);
        }
        let ctx = &mut self.ctx;
        let mut screens = self.stack.iter_mut().rev();
        let top = screens.next().expect("there is always a screen");
        let mut unclaimed = match top.on_msg(msg, ctx) {
            Ok(action) => return self.apply(action),
            Err(msg) => msg,
        };
        for screen in screens {
            match screen.on_msg(unclaimed, ctx) {
                Ok(_) => return,
                Err(msg) => unclaimed = msg,
            }
        }
        if let Msg::Ready(_, Ok(playback)) = unclaimed {
            let engine = ctx.engine.clone();
            ctx.clean_up(async move { engine.stop(&playback).await });
        }
    }

    pub fn on_tick(&mut self) {
        self.ctx.downloads.refresh();
        let ctx = &mut self.ctx;
        self.stack.last_mut().expect("there is always a screen").on_tick(ctx);
    }

    pub fn render(&mut self, frame: &mut Frame) {
        self.top().render(frame);
    }
}

fn is_ctrl(key: &KeyEvent, letter: char) -> bool {
    key.modifiers.contains(KeyModifiers::CONTROL) && key.code == KeyCode::Char(letter)
}
