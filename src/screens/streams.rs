use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, ListState, Paragraph};

use super::widgets::{self, ACCENT, DIM, Hit};
use super::{Action, Screen, playing::Playing};
use crate::app::{Ctx, Msg, Pending, is_current};
use crate::catalog::{Episode, Title};
use crate::downloads::Download;
use crate::history::Entry;
use crate::library;
use crate::poster::{Art, Picture};
use crate::release::format_size;
use crate::streams::{self, Found, Target, Torrent};

struct Row {
    torrent: Torrent,
    quality: String,
    languages: String,
    searchable: String,
}

impl Row {
    fn new(torrent: Torrent) -> Row {
        let (quality, languages) = (torrent.quality(), torrent.languages());
        let searchable = format!("{quality} {} {} {languages}", torrent.full_name(), torrent.source).to_lowercase();
        Row { torrent, quality, languages, searchable }
    }
}

pub struct Streams {
    title: Title,
    episode: Option<Episode>,
    rows: Option<Vec<Row>>,
    failures: Vec<String>,
    visible: Vec<usize>,
    filter: String,
    editing_filter: bool,
    list: ListState,
    request: Option<Pending>,
    list_area: Rect,
    starred: bool,
    backdrop: Option<Picture>,
    archive: bool,
    tabs_area: Rect,
    resume_at: Option<f64>,
    from_start: bool,
    on_disk: Vec<Torrent>,
    notice: Option<&'static str>,
}

impl Streams {
    pub fn new(title: Title, episode: Option<Episode>) -> Streams {
        Streams {
            title,
            episode,
            rows: None,
            failures: vec![],
            visible: vec![],
            filter: String::new(),
            editing_filter: false,
            list: ListState::default(),
            request: None,
            list_area: Rect::default(),
            starred: false,
            backdrop: None,
            archive: false,
            tabs_area: Rect::default(),
            resume_at: None,
            from_start: false,
            on_disk: vec![],
            notice: None,
        }
    }

    fn heading(&self) -> String {
        widgets::heading(&self.title, self.episode.as_ref())
    }

    pub fn title(&self) -> &Title {
        &self.title
    }

    pub fn starting_over(self, from_start: bool) -> Streams {
        Streams { from_start, ..self }
    }

    pub fn resume(&mut self, ctx: &mut Ctx) {
        self.starred = ctx.watchlist.contains(&self.title.id);
        let episode = self.episode.as_ref().map(|e| (e.season, e.episode));
        self.resume_at = ctx.history.get(&self.title.id, episode).and_then(Entry::resume_at);
    }

    pub fn start(&mut self, ctx: &mut Ctx) {
        self.resume(ctx);
        ctx.request_art(Art::Backdrop, &self.title.id);
        let target = Target {
            imdb_id: self.title.id.clone(),
            name: self.title.name.clone(),
            year: self.title.first_year(),
            episode: self.episode.as_ref().map(|e| (e.season, e.episode)),
        };
        self.on_disk = library::find(&target, &ctx.downloads.unfinished());
        if !self.on_disk.is_empty() {
            self.set_rows(vec![]);
        }
        let engine = ctx.engine.clone();
        let work = async move { streams::find(&target, &engine).await };
        self.request = Some(ctx.request(work, Msg::Streams));
    }

    fn set_rows(&mut self, torrents: Vec<Torrent>) {
        self.rows = Some(self.on_disk.iter().cloned().chain(torrents).map(Row::new).collect());
        self.apply_filter();
    }

    fn rows(&self) -> &[Row] {
        self.rows.as_deref().unwrap_or_default()
    }

    fn apply_filter(&mut self) {
        let filter = self.filter.to_lowercase();
        let words: Vec<&str> = filter.split_whitespace().collect();
        let wanted = |row: &Row| row.torrent.archive.is_some() == self.archive && words.iter().all(|w| row.searchable.contains(w));
        self.visible = self.rows().iter().enumerate().filter(|(_, row)| wanted(row)).map(|(i, _)| i).collect();
        self.list.select(Some(0));
    }

    fn count(&self, archive: bool) -> usize {
        self.rows().iter().filter(|row| row.torrent.archive.is_some() == archive).count()
    }

    fn tab_labels(&self) -> Vec<String> {
        if self.count(true) == 0 {
            return vec![];
        }
        vec![format!("Torrents ({})", self.count(false)), format!("Archive.org ({})", self.count(true))]
    }

    fn show_archive(&mut self, archive: bool) {
        if !self.tab_labels().is_empty() {
            self.archive = archive;
            self.apply_filter();
        }
    }

    pub fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        if self.editing_filter {
            self.edit_filter(key.code);
            return Action::None;
        }
        if widgets::navigate(&mut self.list, self.visible.len(), key.code) {
            return Action::None;
        }
        match key.code {
            KeyCode::Esc | KeyCode::Char('q') => Action::Pop,
            KeyCode::Char('f') | KeyCode::Char('/') => {
                self.editing_filter = true;
                Action::None
            }
            KeyCode::Enter => self.play(self.from_start),
            KeyCode::Char('d') => {
                self.download(ctx);
                Action::None
            }
            KeyCode::Char('s') if self.resume_at.is_some() => self.play(true),
            KeyCode::Left | KeyCode::Right | KeyCode::Tab | KeyCode::BackTab => {
                self.show_archive(!self.archive);
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent) -> Action {
        if self.editing_filter {
            return Action::None;
        }
        if widgets::scroll(&mut self.list, self.visible.len(), mouse.kind) {
            return Action::None;
        }
        match mouse.kind {
            MouseEventKind::Down(MouseButton::Right) => Action::Pop,
            MouseEventKind::Down(MouseButton::Left) => {
                if let Some(tab) = widgets::tab_at(&self.tab_labels(), self.tabs_area, widgets::position(&mouse)) {
                    self.show_archive(tab == 1);
                    return Action::None;
                }
                let heights: Vec<u16> = self.visible.iter().map(|&i| item_height(&self.rows()[i].torrent)).collect();
                match widgets::click_row(&mut self.list, self.list_area, heights, widgets::position(&mouse)) {
                    Hit::Activated => self.play(self.from_start),
                    Hit::Selected | Hit::Miss => Action::None,
                }
            }
            _ => Action::None,
        }
    }

    fn edit_filter(&mut self, code: KeyCode) {
        match code {
            KeyCode::Enter => self.editing_filter = false,
            KeyCode::Esc => {
                self.editing_filter = false;
                self.filter.clear();
            }
            KeyCode::Backspace => {
                self.filter.pop();
            }
            KeyCode::Char(c) => self.filter.push(c),
            _ => return,
        }
        self.apply_filter();
    }

    fn selected(&self) -> Option<&Torrent> {
        let &index = self.list.selected().and_then(|i| self.visible.get(i))?;
        Some(&self.rows()[index].torrent)
    }

    fn download(&mut self, ctx: &mut Ctx) {
        let Some(torrent) = self.selected().cloned() else { return };
        self.notice = Some(match torrent.local.is_some() {
            true => "💾 Already on disk",
            false if ctx.downloads.add(Download::new(self.title.clone(), self.episode.clone(), torrent)) => {
                "💾 Downloading, see Downloads on the home screen"
            }
            false => "💾 Already in Downloads",
        });
    }

    fn play(&self, from_start: bool) -> Action {
        let Some(torrent) = self.selected().cloned() else {
            return Action::None;
        };
        let playing = Playing::new(self.title.clone(), self.episode.clone(), torrent).starting_over(from_start);
        Action::Push(Box::new(Screen::Playing(Box::new(playing))))
    }

    pub fn on_msg(&mut self, msg: Msg) -> Result<Action, Msg> {
        match msg {
            Msg::Streams(id, found) if is_current(&self.request, id) => {
                self.request = None;
                let Found { torrents, failures } = found;
                self.failures = failures;
                self.set_rows(torrents);
                Ok(Action::None)
            }
            Msg::Streams(..) => Ok(Action::None),
            Msg::Art(Art::Backdrop, title_id, picture) if title_id == self.title.id => {
                self.backdrop = picture;
                Ok(Action::None)
            }
            other => Err(other),
        }
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let labels = self.tab_labels();
        let tabs = widgets::tabs(&labels, usize::from(self.archive), widgets::content_width(frame));
        let header_height = if labels.is_empty() { 1 } else { 2 + tabs.len() as u16 };
        let areas = widgets::areas(frame, "streams", header_height);
        self.list_area = areas.body;
        self.tabs_area = if labels.is_empty() { Rect::default() } else { widgets::below(areas.header, 2) };
        let mut header = vec![Span::raw(self.heading()).bold()];
        header.extend(widgets::starred(self.starred));
        match (self.resume_at, self.from_start) {
            (Some(_), true) => header.push(Span::raw("  ⏮ from the beginning").fg(DIM)),
            (Some(at), false) => header.push(Span::raw(format!("  ⏯ resumes at {}", widgets::clock(at))).fg(DIM)),
            (None, _) => {}
        }
        if self.request.is_some() && self.rows.is_some() {
            header.push(Span::raw("  looking for torrents…").fg(DIM));
        }
        if let Some(notice) = self.notice {
            header.push(Span::raw(format!("   {notice}")).fg(ACCENT));
        }
        if self.editing_filter || !self.filter.is_empty() {
            header.push(Span::raw("   filter: ").fg(DIM));
            header.push(Span::raw(self.filter.clone()).fg(ACCENT));
            if self.editing_filter {
                header.push(Span::raw("▏").fg(ACCENT));
            }
        }
        let mut header = vec![Line::from(header)];
        if !labels.is_empty() {
            header.push(Line::default());
            header.extend(tabs);
        }
        frame.render_widget(Paragraph::new(header), areas.header);

        match &self.rows {
            None => {
                let text = match self.episode {
                    Some(_) => "Looking for torrents (season packs take a few seconds)…",
                    None => "Looking for torrents…",
                };
                widgets::message(frame, areas.body, text);
            }
            Some(_) if self.visible.is_empty() => {
                let none_here = self.count(self.archive) == 0;
                let mut text = String::from(if none_here { "No torrents found." } else { "Nothing matches the filter." });
                for failure in &self.failures {
                    text.push_str(&format!("\n{failure}"));
                }
                widgets::message(frame, areas.body, &text);
            }
            Some(rows) => {
                let items = self.visible.iter().map(|&i| torrent_item(&rows[i])).collect();
                widgets::list(frame, areas.body, items, &mut self.list);
            }
        }

        let hints: &[(&str, &str)] = match (self.editing_filter, labels.is_empty()) {
            (true, _) => &[("⏎", "apply"), ("esc", "clear filter")],
            (false, true) => {
                &[("↑↓", "move"), ("⏎", "watch"), ("d", "download"), ("f", "filter"), ("ctrl+s", "watchlist"), ("esc", "back")]
            }
            (false, false) => {
                &[("←→", "source"), ("⏎", "watch"), ("d", "download"), ("f", "filter"), ("ctrl+s", "watchlist"), ("esc", "back")]
            }
        };
        let start_over = [("s", "start over")];
        let hints: Vec<(&str, &str)> = match self.resume_at.is_some() && !self.from_start && !self.editing_filter {
            true => [&hints[..2], &start_over, &hints[2..]].concat(),
            false => hints.to_vec(),
        };
        widgets::footer(frame, areas.footer, &hints);
        widgets::backdrop(frame, self.backdrop.as_deref());
    }
}

fn item_height(t: &Torrent) -> u16 {
    if t.file_name.is_some() { 4 } else { 3 }
}

fn torrent_item(row: &Row) -> ListItem<'static> {
    let t = &row.torrent;
    let summary = Line::from(vec![
        Span::raw(format!("{:<6}", row.quality)).fg(ACCENT).bold(),
        Span::raw(format!(" 👤 {:<11}", t.peers())),
        Span::raw(format!(" 💾 {:>9}", format_size(t.size))),
        Span::raw(format!("  {}", t.source)).fg(DIM),
        Span::raw(format!("  {}", row.languages)),
    ]);
    let mut lines = vec![summary, Line::from(Span::raw(format!("       {}", t.name)).fg(DIM))];
    if let Some(file) = &t.file_name {
        lines.push(Line::from(Span::raw(format!("       📄 {file}")).fg(DIM)));
    }
    lines.push(Line::default());
    ListItem::new(lines)
}
