use std::sync::Arc;

use crossterm::event::{KeyCode, KeyEvent};
use ratatui::Frame;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::Paragraph;

use super::widgets::{self, ACCENT, DIM, DONE};
use super::{Action, Screen, streams::Streams};
use crate::app::{Ctx, Msg, Pending, is_current};
use crate::archive;
use crate::catalog::{self, Episode, Kind, Title};
use crate::engine::{Playback, Status};
use crate::history::{self, Entry, History};
use crate::library;
use crate::player::{self, Launch, Player, PlayerEvent, Track};
use crate::release::{episode_tag, format_size};
use crate::streams::{Torrent, episode_file_index};
use crate::subtitles::{self, Measured, Subtitle};
use tokio::sync::oneshot;

const MIN_RUNTIME_RATIO: f64 = 0.5;

enum Phase {
    Connecting,
    Opening,
    Playing,
    Finished { next: Option<Option<Episode>> },
}

pub struct Playing {
    title: Title,
    episode: Option<Episode>,
    torrent: Torrent,
    reuse: Option<Arc<Playback>>,
    from_start: bool,
    request: Option<u64>,
    playback: Option<Arc<Playback>>,
    status: Status,
    player: Option<Player>,
    player_id: u64,
    phase: Phase,
    resume_at: Option<f64>,
    position: f64,
    duration: f64,
    subtitles: Option<Vec<Subtitle>>,
    subtitles_request: Option<Pending>,
    measured: Option<oneshot::Sender<Measured>>,
    next_request: Option<Pending>,
    error: Option<String>,
}

impl Playing {
    pub fn new(title: Title, episode: Option<Episode>, torrent: Torrent) -> Playing {
        Playing {
            title,
            episode,
            torrent,
            reuse: None,
            from_start: false,
            request: None,
            playback: None,
            status: Status::default(),
            player: None,
            player_id: 0,
            phase: Phase::Connecting,
            resume_at: None,
            position: 0.0,
            duration: 0.0,
            subtitles: None,
            subtitles_request: None,
            measured: None,
            next_request: None,
            error: None,
        }
    }

    pub fn title(&self) -> &Title {
        &self.title
    }

    pub fn starting_over(mut self, from_start: bool) -> Playing {
        self.from_start = from_start;
        self
    }

    fn is_the_real_thing(&self) -> bool {
        let runtime = self.title.details.runtime_seconds().filter(|_| self.episode.is_none());
        runtime.is_none_or(|runtime| self.duration >= runtime * MIN_RUNTIME_RATIO)
    }

    fn episode_key(&self) -> Option<(u32, u32)> {
        self.episode.as_ref().map(|e| (e.season, e.episode))
    }

    pub fn start(&mut self, ctx: &mut Ctx) {
        let engine = ctx.engine.clone();
        let saved = ctx.history.get(&self.title.id, self.episode_key()).and_then(Entry::resume_at);
        self.resume_at = saved.filter(|_| !self.from_start);
        let (reuse, torrent) = (self.reuse.take(), self.torrent.clone());
        let work = async move {
            let playback = match (torrent.local, reuse, torrent.file_idx, torrent.archive) {
                (Some(path), ..) => library::playback(&path),
                (_, Some(playback), Some(file_idx), _) => engine.switch(&playback, file_idx).await,
                (_, _, file_idx, Some(identifier)) => archive::play(&identifier, file_idx).await,
                (_, _, file_idx, None) => engine.play(&torrent.info_hash, file_idx).await,
            };
            playback.map(Arc::new).map_err(|e| format!("{e:#}"))
        };
        self.request = Some(ctx.request_detached(work, Msg::Ready));
    }

    pub fn on_key(&mut self, key: KeyEvent) -> Action {
        match (key.code, &self.phase) {
            (KeyCode::Enter, Phase::Finished { next: Some(Some(next)) }) => self.play_next(next.clone()),
            (KeyCode::Esc | KeyCode::Char('q'), _) => Action::Pop,
            _ => Action::None,
        }
    }

    fn play_next(&mut self, next: Episode) -> Action {
        let in_same_pack = self
            .playback
            .as_ref()
            .and_then(|p| episode_file_index(&p.files, next.season, next.episode).map(|idx| (p, idx)));
        let Some((playback, idx)) = in_same_pack else {
            return Action::Replace(Box::new(Screen::Streams(Streams::new(self.title.clone(), Some(next)))));
        };
        let file = &playback.files[idx];
        let torrent = Torrent {
            file_idx: Some(idx),
            file_name: Some(file.name().to_string()),
            size: file.length,
            ..self.torrent.clone()
        };
        let mut following = Playing::new(self.title.clone(), Some(next), torrent);
        following.reuse = self.playback.take();
        Action::Replace(Box::new(Screen::Playing(Box::new(following))))
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Result<Action, Msg> {
        match msg {
            Msg::Ready(id, result) if Some(id) == self.request => {
                self.request = None;
                match result {
                    Ok(playback) => self.open(playback, ctx),
                    Err(err) => self.error = Some(err),
                }
                Ok(Action::None)
            }
            Msg::Player(id, event) if id == self.player_id => Ok(self.on_player(event, ctx)),
            Msg::Subtitles(id, found) if is_current(&self.subtitles_request, id) => {
                self.subtitles_request = None;
                self.load_subtitles(found);
                Ok(Action::None)
            }
            Msg::Next(id, next) if is_current(&self.next_request, id) => {
                self.phase = Phase::Finished { next: Some(next) };
                Ok(Action::None)
            }
            Msg::Player(..) | Msg::Subtitles(..) | Msg::Next(..) => Ok(Action::None),
            other => Err(other),
        }
    }

    fn open(&mut self, playback: Arc<Playback>, ctx: &mut Ctx) {
        let title = widgets::heading(&self.title, self.episode.as_ref());
        let launch = Launch { url: playback.url(), title, start: self.resume_at };
        let id = ctx.new_id();
        let tx = ctx.tx.clone();
        self.player = Some(player::launch(launch, move |event| {
            let _ = tx.send(Msg::Player(id, event));
        }));
        self.player_id = id;
        self.phase = Phase::Opening;

        let request = subtitles::Request {
            kind: self.title.kind,
            imdb_id: self.title.id.clone(),
            episode: self.episode_key(),
        };
        let for_subtitles = playback.clone();
        let (measured, measurement) = oneshot::channel();
        self.measured = Some(measured);
        let work = async move { subtitles::prepare(request, &for_subtitles, measurement).await };
        self.subtitles_request = Some(ctx.request(work, Msg::Subtitles));
        self.playback = Some(playback);
    }

    fn load_subtitles(&mut self, found: Vec<Subtitle>) {
        if let Some(player) = &self.player {
            let tracks: Vec<Track> = found
                .iter()
                .map(|s| Track { path: &s.path, title: s.label(), lang: &s.lang })
                .collect();
            player.add_subtitles(&tracks);
        }
        self.subtitles = Some(found);
    }

    fn finished(&self) -> bool {
        history::watched(history::ratio(self.position, self.duration))
    }

    fn on_player(&mut self, event: PlayerEvent, ctx: &mut Ctx) -> Action {
        match event {
            PlayerEvent::Started => Action::None,
            PlayerEvent::Measured { duration, fps } => {
                if let Some(measured) = self.measured.take() {
                    let _ = measured.send(Measured { duration, fps });
                }
                Action::None
            }
            PlayerEvent::Position { position, duration } => {
                (self.position, self.duration) = (position, duration);
                self.phase = Phase::Playing;
                Action::None
            }
            PlayerEvent::Exited => match self.episode_key() {
                Some((season, episode)) if self.finished() => {
                    self.ask_for_next(season, episode, ctx);
                    Action::None
                }
                _ => Action::Pop,
            },
        }
    }

    fn ask_for_next(&mut self, season: u32, episode: u32, ctx: &mut Ctx) {
        self.phase = Phase::Finished { next: None };
        let id = self.title.id.clone();
        let work = async move {
            let meta = catalog::meta(Kind::Series, &id).await.ok()?;
            meta.next_episode(season, episode)
        };
        self.next_request = Some(ctx.request(work, Msg::Next));
    }

    pub fn stop(&mut self, ctx: &mut Ctx) {
        self.player = None;
        self.save_progress(&mut ctx.history);
        let subtitles = self.subtitles_request.take();
        if let Some(playback) = self.playback.take() {
            let engine = ctx.engine.clone();
            ctx.clean_up(async move {
                if let Some(subtitles) = subtitles {
                    subtitles.cancel().await;
                }
                subtitles::forget(&playback);
                engine.stop(&playback).await;
            });
        }
    }

    fn save_progress(&self, history: &mut History) {
        if self.position > 0.0 && self.is_the_real_thing() {
            history.record(Entry::new(self.title.clone(), self.episode.clone(), self.position, self.duration));
        }
    }

    pub fn on_tick(&mut self) {
        if let Some(playback) = &self.playback {
            self.status = playback.status();
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let areas = widgets::areas(frame, "player", 1);
        let heading = widgets::heading(&self.title, self.episode.as_ref());
        frame.render_widget(Paragraph::new(Span::raw(heading).bold()), areas.header);

        let mut lines = vec![Line::from(Span::raw(self.torrent.name.clone()).fg(DIM))];
        if let Some(playback) = &self.playback {
            lines.push(Line::from(Span::raw(format!("📄 {}", playback.file.name())).fg(DIM)));
        }
        lines.push(Line::default());
        match &self.error {
            Some(error) => lines.push(Line::from(format!("Could not open the torrent: {error}")).red()),
            None => lines.extend(self.phase_lines()),
        }
        if self.playback.is_some() {
            lines.push(Line::default());
            lines.push(self.status_line());
            lines.push(self.subtitles_line());
        }
        frame.render_widget(Paragraph::new(lines), areas.body);

        let hints: &[(&str, &str)] = match self.phase {
            Phase::Finished { next: Some(Some(_)) } => &[("⏎", "watch next"), ("esc", "back")],
            _ => &[("esc", "stop and go back"), ("j in mpv", "switch subtitle")],
        };
        widgets::footer(frame, areas.footer, hints);
    }

    fn phase_lines(&self) -> Vec<Line<'static>> {
        let resume = self.resume_at.map(|at| format!(" (resuming at {})", widgets::clock(at))).unwrap_or_default();
        match &self.phase {
            Phase::Connecting => {
                let line = match (&self.torrent.local, &self.torrent.archive) {
                    (Some(_), _) => format!("Opening the file…{resume}"),
                    (None, Some(_)) => format!("Connecting to Archive.org…{resume}"),
                    (None, None) => format!("Connecting to peers…{resume}"),
                };
                vec![Line::from(line).fg(ACCENT)]
            }
            Phase::Opening => vec![Line::from(format!("Opening mpv…{resume}")).fg(ACCENT)],
            Phase::Playing => {
                let ratio = history::ratio(self.position, self.duration);
                vec![Line::from(vec![
                    Span::raw("▶ Playing in mpv  ").fg(ACCENT),
                    Span::raw(format!("{} / {}  ", widgets::clock(self.position), widgets::clock(self.duration))),
                    Span::raw(widgets::bar(ratio, 24)).fg(ACCENT),
                ])]
            }
            Phase::Finished { next } => {
                let mut lines = vec![Line::from("✓ Finished").fg(DONE)];
                lines.push(match next {
                    None => Line::from("Looking for the next episode…").fg(DIM),
                    Some(None) => Line::from("No more episodes.").fg(DIM),
                    Some(Some(e)) => Line::from(vec![
                        Span::raw("Next: ").fg(DIM),
                        Span::raw(episode_tag(e.season, e.episode)).fg(ACCENT).bold(),
                        Span::raw(format!("  {}", e.name.clone().unwrap_or_default())),
                    ]),
                });
                lines
            }
        }
    }

    fn status_line(&self) -> Line<'static> {
        let size = self.playback.as_ref().map(|p| p.file.length).unwrap_or(0);
        if self.playback.as_ref().is_some_and(|p| p.is_local()) {
            return Line::from(format!("💾 Playing from disk ({})", format_size(size))).fg(DONE);
        }
        if self.playback.as_ref().is_some_and(|p| p.is_direct()) {
            return Line::from(format!("🌐 Streaming from Archive.org · 💾 {}", format_size(size)));
        }
        if size > 0 && self.status.downloaded >= size {
            return Line::from(format!("💾 ✓ Fully downloaded ({})", format_size(size))).fg(DONE);
        }
        let downloaded = if self.status.downloaded == 0 { "0 MB".to_string() } else { format_size(self.status.downloaded) };
        Line::from(format!(
            "👤 {} peers · ⬇ {:.1} MB/s · 💾 {downloaded} of {}",
            self.status.peers,
            self.status.speed_mbps,
            format_size(size)
        ))
    }

    fn subtitles_line(&self) -> Line<'static> {
        let Some(found) = &self.subtitles else {
            return Line::from("💬 Looking for subtitles…").fg(DIM);
        };
        let Some(best) = found.first() else {
            return Line::from("💬 No subtitles").fg(DIM);
        };
        let reasons = if best.reasons.is_empty() { "popularity".to_string() } else { best.reasons.join(", ") };
        Line::from(vec![
            Span::raw(format!("💬 {}", best.lang.to_uppercase())).fg(ACCENT),
            Span::raw(format!(" · {reasons}")),
            Span::raw(format!("  (+{} tracks)", found.len() - 1)).fg(DIM),
        ])
    }
}
