use std::collections::HashMap;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, ListState, Paragraph};

use super::widgets::{self, DIM, Hit};
use super::{Action, Screen, streams::Streams};
use crate::app::{Ctx, Msg, Pending, is_current};
use crate::catalog::{self, Episode, Kind, Meta, Title};
use crate::history::watched;
use crate::poster::{Art, Picture};

pub struct Series {
    title: Title,
    meta: Option<Meta>,
    seasons: Vec<u32>,
    season: usize,
    last_watched: Option<((u32, u32), bool)>,
    starred: bool,
    poster: Option<Picture>,
    backdrop: Option<Picture>,
    episodes: Vec<Episode>,
    progress: HashMap<(u32, u32), f64>,
    list: ListState,
    request: Option<Pending>,
    error: Option<String>,
    tabs_area: Rect,
    list_area: Rect,
}

impl Series {
    pub fn new(title: Title) -> Series {
        Series {
            title,
            meta: None,
            seasons: vec![],
            season: 0,
            last_watched: None,
            starred: false,
            poster: None,
            backdrop: None,
            episodes: vec![],
            progress: HashMap::new(),
            list: ListState::default(),
            request: None,
            error: None,
            tabs_area: Rect::default(),
            list_area: Rect::default(),
        }
    }

    pub fn start(&mut self, ctx: &mut Ctx) {
        let id = self.title.id.clone();
        let work = async move { catalog::meta(Kind::Series, &id).await.map(Box::new).map_err(|e| e.to_string()) };
        self.request = Some(ctx.request(work, Msg::Meta));
        ctx.request_art(Art::Poster, &self.title.id);
        ctx.request_art(Art::Backdrop, &self.title.id);
        self.last_watched = ctx
            .history
            .latest_episode(&self.title.id)
            .and_then(|entry| entry.episode.as_ref().map(|e| ((e.season, e.episode), entry.finished())));
        self.resume(ctx);
    }

    pub fn resume(&mut self, ctx: &mut Ctx) {
        self.progress = ctx.history.progress_of(&self.title.id);
        self.starred = ctx.watchlist.contains(&self.title.id);
    }

    pub fn title(&self) -> &Title {
        &self.title
    }

    fn focus(&self, meta: &Meta) -> Option<(u32, u32)> {
        let ((season, episode), finished) = self.last_watched?;
        let next = finished.then(|| meta.next_episode(season, episode)).flatten();
        Some(next.map(|e| (e.season, e.episode)).unwrap_or((season, episode)))
    }

    fn show_focus(&mut self) {
        let Some((season, episode)) = self.meta.as_ref().and_then(|meta| self.focus(meta)) else {
            self.show_season(0);
            return;
        };
        self.show_season(self.seasons.iter().position(|s| *s == season).unwrap_or(0));
        if let Some(index) = self.episodes.iter().position(|e| (e.season, e.episode) == (season, episode)) {
            self.list.select(Some(index));
        }
    }

    fn show_season(&mut self, index: usize) {
        let Some(meta) = &self.meta else { return };
        let Some(&season) = self.seasons.get(index) else { return };
        self.season = index;
        self.episodes = meta.episodes(season);
        let first_unwatched = self.episodes.iter().position(|e| !watched(self.ratio(e))).unwrap_or(0);
        self.list.select(Some(first_unwatched));
    }

    fn ratio(&self, episode: &Episode) -> f64 {
        self.progress.get(&(episode.season, episode.episode)).copied().unwrap_or(0.0)
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        if widgets::navigate(&mut self.list, self.episodes.len(), key.code) {
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Pop,
            KeyCode::Left => {
                self.show_season(self.season.saturating_sub(1));
                Action::None
            }
            KeyCode::Right => {
                self.show_season(self.season + 1);
                Action::None
            }
            KeyCode::Enter => self.open(),
            _ => Action::None,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> Action {
        if widgets::scroll(&mut self.list, self.episodes.len(), mouse.kind) {
            return Action::None;
        }
        let at = widgets::position(&mouse);
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => Action::Pop,
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(index) = widgets::tab_at(&self.season_labels(), self.tabs_area, at) {
                    self.show_season(index);
                    return Action::None;
                }
                match widgets::click_row(&mut self.list, self.list_area, self.episodes.iter().map(|_| 1), at) {
                    Hit::Activated => self.open(),
                    Hit::Selected | Hit::Miss => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    fn season_labels(&self) -> Vec<String> {
        self.seasons.iter().map(|s| if *s == 0 { "Specials".to_string() } else { format!("S{s}") }).collect()
    }

    fn open(&self) -> Action {
        let Some(episode) = self.list.selected().and_then(|i| self.episodes.get(i)) else {
            return Action::None;
        };
        Action::Push(Box::new(Screen::Streams(Streams::new(self.title.clone(), Some(episode.clone())))))
    }

    pub fn on_msg(&mut self, msg: Msg) -> Result<Action, Msg> {
        match msg {
            Msg::Meta(id, result) if is_current(&self.request, id) => {
                self.request = None;
                match result {
                    Ok(meta) => {
                        self.seasons = meta.seasons();
                        self.title.details = meta.details.clone();
                        self.meta = Some(*meta);
                        self.show_focus();
                    }
                    Err(err) => self.error = Some(err),
                }
                Ok(Action::None)
            }
            Msg::Meta(..) => Ok(Action::None),
            Msg::Art(art, title_id, picture) if title_id == self.title.id => {
                match art {
                    Art::Poster => self.poster = picture,
                    Art::Backdrop => self.backdrop = picture,
                }
                Ok(Action::None)
            }
            other => Err(other),
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let labels = self.season_labels();
        let tabs = widgets::tabs(&labels, self.season, widgets::content_width(frame));
        let areas = widgets::areas(frame, "series", 2 + tabs.len() as u16);
        self.tabs_area = widgets::below(areas.header, 2);
        let mut heading = vec![
            Span::raw(self.title.name.clone()).bold(),
            Span::raw(format!("  {}", self.title.year())).fg(DIM),
        ];
        heading.extend(widgets::starred(self.starred));
        let heading = Line::from(heading);
        let header: Vec<Line> = [heading, Line::default()].into_iter().chain(tabs).collect();
        frame.render_widget(Paragraph::new(header), areas.header);

        let (list_area, details_area) = widgets::split_details(areas.body);
        self.list_area = list_area;
        if let Some(error) = &self.error {
            widgets::message(frame, list_area, &format!("Could not load the series: {error}"));
        } else if self.meta.is_none() {
            widgets::message(frame, list_area, "Loading episodes…");
        } else {
            let items = self.episodes.iter().map(|e| episode_item(e, self.ratio(e))).collect();
            widgets::list(frame, list_area, items, &mut self.list);
        }
        if let Some(area) = details_area {
            widgets::panel(frame, area, &self.title.name, widgets::title_details(&self.title, self.meta.is_none()), self.poster.as_deref());
        }

        widgets::footer(frame, areas.footer, &[("←→", "season"), ("↑↓", "episode"), ("⏎", "watch"), ("ctrl+s", "watchlist"), ("esc", "back")]);
        widgets::backdrop(frame, self.backdrop.as_deref());
    }
}

fn episode_item(episode: &Episode, ratio: f64) -> ListItem<'static> {
    let date = episode.released.as_deref().and_then(|d| d.get(..10)).unwrap_or("");
    let mut spans = vec![
        Span::raw(format!("{:>3}  ", episode.episode)).fg(DIM),
        Span::raw(episode.name.clone().unwrap_or_default()),
        Span::raw(format!("  {date}")).fg(DIM),
    ];
    if ratio > 0.0 {
        spans.push(Span::raw("  "));
        spans.extend(widgets::progress(ratio));
    }
    let item = ListItem::new(Line::from(spans));
    if watched(ratio) { item.fg(DIM) } else { item }
}
