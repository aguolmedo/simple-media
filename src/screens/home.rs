use std::collections::{HashMap, HashSet};
use std::time::Duration;

use crossterm::event::{KeyCode, KeyEvent, MouseButton, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::Stylize;
use ratatui::text::{Line, Span};
use ratatui::widgets::{ListItem, ListState, Paragraph};

use super::widgets::{self, ACCENT, DIM, DONE, GOLD, Hit};
use super::{Action, Screen, playing::Playing, series::Series, streams::Streams};
use crate::app::{Ctx, Msg, Pending, is_current};
use crate::catalog::{self, Details, Kind, Title};
use crate::downloads::{Item, State};
use crate::history::Entry;
use crate::library;
use crate::poster::{Art, Picture};
use crate::release::episode_tag;

const DEBOUNCE: Duration = Duration::from_millis(300);

#[derive(Clone, Copy, PartialEq)]
enum Tab {
    Continue,
    Watchlist,
    Downloads,
    Recent,
}

enum Row<'a> {
    Continue(&'a Entry),
    Title(&'a Title),
    Download(&'a Item),
    OnDisk(&'a Title),
}

impl Row<'_> {
    fn title(&self) -> &Title {
        match self {
            Row::Continue(entry) => &entry.title,
            Row::Title(title) => title,
            Row::Download(item) => &item.download.title,
            Row::OnDisk(title) => title,
        }
    }

    fn resumable_episode(&self) -> Option<String> {
        let Row::Continue(entry) = self else { return None };
        let episode = entry.episode.as_ref().filter(|_| !entry.finished())?;
        Some(episode_tag(episode.season, episode.episode))
    }
}

#[derive(Clone, Copy)]
enum Choice {
    Resume,
    StartOver,
    Open,
    Watchlist,
    RemoveDownload,
}

struct Menu {
    choices: Vec<(Choice, String)>,
    state: ListState,
    anchor: Position,
    area: Rect,
}

#[derive(Default)]
pub struct Home {
    query: String,
    tab: usize,
    continuing: Vec<Entry>,
    watchlist: Vec<Title>,
    recent: Vec<Title>,
    downloads: Vec<Item>,
    on_disk: Vec<Title>,
    on_disk_request: Option<Pending>,
    results: Vec<Title>,
    search_request: Option<Pending>,
    details: HashMap<String, Details>,
    details_requested: HashSet<String>,
    posters: HashMap<String, Option<Picture>>,
    selection: ListState,
    error: Option<String>,
    menu: Option<Menu>,
    tabs_area: Rect,
    list_area: Rect,
}

impl Home {
    pub fn start(&mut self, ctx: &mut Ctx) {
        self.load_local(ctx);
        self.resume(ctx);
    }

    fn load_local(&mut self, ctx: &Ctx) {
        self.continuing = ctx.history.continue_watching();
        self.watchlist = ctx.watchlist.titles().to_vec();
        self.recent = ctx.history.recent_titles();
        self.downloads = ctx.downloads.items();
    }

    pub fn resume(&mut self, ctx: &mut Ctx) {
        self.posters.retain(|_, poster| poster.is_some());
        self.keeping_tab(|home| home.load_local(ctx));
        self.on_disk_request = Some(ctx.request(library::titles(), Msg::OnDisk));
        self.ensure_details(ctx);
    }

    fn keeping_tab(&mut self, update: impl FnOnce(&mut Home)) {
        let current = self.tabs().get(self.tab).copied();
        update(self);
        self.tab = current.and_then(|tab| self.tabs().iter().position(|t| *t == tab)).unwrap_or(0);
        self.clamp_selection();
    }

    pub fn on_tick(&mut self, ctx: &mut Ctx) {
        if self.current_tab() != Some(Tab::Downloads) {
            return;
        }
        let finished_before = finished_count(&self.downloads);
        self.downloads = ctx.downloads.items();
        if finished_count(&self.downloads) > finished_before {
            self.on_disk_request = Some(ctx.request(library::titles(), Msg::OnDisk));
        }
    }

    fn is_on_disk(&self, title_id: &str) -> bool {
        self.on_disk.iter().any(|title| title.id == title_id)
    }

    fn pending_downloads(&self) -> impl Iterator<Item = &Item> {
        self.downloads.iter().filter(|item| !matches!(item.state, State::Done) || !self.is_on_disk(&item.download.title.id))
    }

    fn on_disk_titles(&self) -> impl Iterator<Item = &Title> {
        let downloading = |title: &Title| {
            title.kind == Kind::Movie
                && self.downloads.iter().any(|item| item.download.title.id == title.id && !matches!(item.state, State::Done))
        };
        self.on_disk.iter().filter(move |title| !downloading(title))
    }

    fn tab_label(&self, tab: Tab) -> &str {
        match tab {
            Tab::Continue => "Continue watching",
            Tab::Watchlist => "Watchlist",
            Tab::Downloads => "Downloads",
            Tab::Recent => "Recently watched",
        }
    }

    fn tabs(&self) -> Vec<Tab> {
        let continuing = (!self.continuing.is_empty()).then_some(Tab::Continue);
        let downloads = (!self.downloads.is_empty() || self.on_disk_titles().next().is_some()).then_some(Tab::Downloads);
        let recent = (!self.recent.is_empty()).then_some(Tab::Recent);
        continuing.into_iter().chain([Tab::Watchlist]).chain(downloads).chain(recent).collect()
    }

    fn tab_labels(&self) -> Vec<&str> {
        self.tabs().into_iter().map(|t| self.tab_label(t)).collect()
    }

    fn current_tab(&self) -> Option<Tab> {
        (!self.searching()).then(|| self.tabs().get(self.tab).copied()).flatten()
    }

    fn searching(&self) -> bool {
        !self.query.trim().is_empty()
    }

    fn rows(&self) -> Vec<Row<'_>> {
        if self.searching() {
            return self.results.iter().map(Row::Title).collect();
        }
        match self.current_tab() {
            Some(Tab::Continue) => self.continuing.iter().map(Row::Continue).collect(),
            Some(Tab::Watchlist) => self.watchlist.iter().map(Row::Title).collect(),
            Some(Tab::Recent) => self.recent.iter().map(Row::Title).collect(),
            Some(Tab::Downloads) => {
                self.pending_downloads().map(Row::Download).chain(self.on_disk_titles().map(Row::OnDisk)).collect()
            }
            None => vec![],
        }
    }

    pub fn selected_title(&self) -> Option<Title> {
        let rows = self.rows();
        let title = rows.get(self.selection.selected()?)?.title();
        Some(self.with_details(title))
    }

    fn with_details(&self, title: &Title) -> Title {
        let mut title = title.clone();
        if let Some(details) = self.details.get(&title.id) {
            title.details = details.clone();
        }
        title
    }

    fn in_watchlist(&self, title_id: &str) -> bool {
        self.watchlist.iter().any(|t| t.id == title_id)
    }

    fn clamp_selection(&mut self) {
        let len = self.rows().len();
        let selected = self.selection.selected().unwrap_or(0).min(len.saturating_sub(1));
        self.selection.select(Some(selected));
    }

    fn ensure_details(&mut self, ctx: &mut Ctx) {
        let Some(title) = self.selected_title() else { return };
        self.ensure_poster(&title.id, ctx);
        if !title.details.is_empty() || !self.details_requested.insert(title.id.clone()) {
            return;
        }
        let work = async move {
            let details = catalog::meta(title.kind, &title.id).await.ok().map(|m| Box::new(m.details));
            (title.id, details)
        };
        ctx.request_detached(work, |_, (title_id, details)| Msg::Details(title_id, details));
    }

    fn ensure_poster(&mut self, title_id: &str, ctx: &mut Ctx) {
        if self.posters.contains_key(title_id) {
            return;
        }
        self.posters.insert(title_id.to_string(), None);
        ctx.request_art(Art::Poster, title_id);
    }

    fn switch_tab(&mut self, forward: bool) {
        let count = self.tabs().len();
        self.select_tab(if forward { (self.tab + 1) % count } else { (self.tab + count - 1) % count });
    }

    fn select_tab(&mut self, tab: usize) {
        self.tab = tab;
        self.selection.select(Some(0));
    }

    pub fn on_key(&mut self, key: KeyEvent, ctx: &mut Ctx) -> Action {
        if self.menu.is_some() {
            return self.on_menu_key(key.code, ctx);
        }
        let len = self.rows().len();
        if widgets::navigate(&mut self.selection, len, key.code) {
            self.ensure_details(ctx);
            return Action::None;
        }
        match key.code {
            KeyCode::Tab | KeyCode::Right if !self.searching() => self.switch_tab(true),
            KeyCode::BackTab | KeyCode::Left if !self.searching() => self.switch_tab(false),
            KeyCode::Esc if self.query.is_empty() => return Action::Quit,
            KeyCode::Esc => self.edit_query(ctx, |q| q.clear()),
            KeyCode::Enter => return self.open(false),
            KeyCode::Backspace => self.edit_query(ctx, |q| {
                q.pop();
            }),
            KeyCode::Char(c) => self.edit_query(ctx, |q| q.push(c)),
            _ => return Action::None,
        }
        self.ensure_details(ctx);
        Action::None
    }

    fn on_menu_key(&mut self, code: KeyCode, ctx: &mut Ctx) -> Action {
        let Some(menu) = &mut self.menu else { return Action::None };
        if widgets::navigate(&mut menu.state, menu.choices.len(), code) {
            return Action::None;
        }
        match code {
            KeyCode::Enter => {
                let index = menu.state.selected().unwrap_or(0);
                self.choose(index, ctx)
            }
            KeyCode::Esc => {
                self.menu = None;
                Action::None
            }
            _ => Action::None,
        }
    }

    pub fn on_mouse(&mut self, mouse: MouseEvent, ctx: &mut Ctx) -> Action {
        if self.menu.is_some() {
            return self.on_menu_mouse(mouse, ctx);
        }
        let len = self.rows().len();
        if widgets::scroll(&mut self.selection, len, mouse.kind) {
            self.ensure_details(ctx);
            return Action::None;
        }
        let at = widgets::position(&mouse);
        let action = match mouse.kind {
            MouseEventKind::Down(MouseButton::Left) => self.left_click(at, len),
            MouseEventKind::Down(MouseButton::Right) => {
                if !matches!(widgets::click_row(&mut self.selection, self.list_area, (0..len).map(|_| 1), at), Hit::Miss) {
                    self.open_menu(at);
                }
                Action::None
            }
            _ => return Action::None,
        };
        self.ensure_details(ctx);
        action
    }

    fn left_click(&mut self, at: Position, len: usize) -> Action {
        if let Some(tab) = widgets::tab_at(&self.tab_labels(), self.tabs_area, at) {
            self.select_tab(tab);
            return Action::None;
        }
        match widgets::click_row(&mut self.selection, self.list_area, (0..len).map(|_| 1), at) {
            Hit::Activated => self.open(false),
            Hit::Selected | Hit::Miss => Action::None,
        }
    }

    fn on_menu_mouse(&mut self, mouse: MouseEvent, ctx: &mut Ctx) -> Action {
        let Some(menu) = &mut self.menu else { return Action::None };
        if !matches!(mouse.kind, MouseEventKind::Down(_)) {
            return Action::None;
        }
        let inner = menu.area.inner(ratatui::layout::Margin { horizontal: 1, vertical: 1 });
        let heights = menu.choices.iter().map(|_| 1);
        match widgets::click_row(&mut menu.state, inner, heights, widgets::position(&mouse)) {
            Hit::Miss => {
                self.menu = None;
                Action::None
            }
            Hit::Selected | Hit::Activated => {
                let index = menu.state.selected().unwrap_or(0);
                self.choose(index, ctx)
            }
        }
    }

    fn open_menu(&mut self, anchor: Position) {
        let rows = self.rows();
        let Some(row) = self.selection.selected().and_then(|i| rows.get(i)) else { return };
        let title = row.title();
        let mut choices = Vec::new();
        if let Some(tag) = row.resumable_episode() {
            choices.push((Choice::Resume, format!("Resume {tag}")));
        }
        let open = match title.kind {
            _ if matches!(row, Row::Download(_)) => "Play",
            Kind::Series => "Episodes",
            Kind::Movie if matches!(row, Row::Continue(_)) => "Resume",
            Kind::Movie => "Open",
        };
        choices.push((Choice::Open, open.to_string()));
        if let Row::Continue(entry) = row
            && entry.resume_at().is_some()
        {
            choices.push((Choice::StartOver, "Start over".to_string()));
        }
        if matches!(row, Row::Download(_)) {
            choices.push((Choice::RemoveDownload, "Remove download".to_string()));
        }
        let watchlist = if self.in_watchlist(&title.id) { "Remove from watchlist" } else { "Add to watchlist" };
        choices.push((Choice::Watchlist, watchlist.to_string()));
        self.menu = Some(Menu { choices, state: ListState::default().with_selected(Some(0)), anchor, area: Rect::default() });
    }

    fn choose(&mut self, index: usize, ctx: &mut Ctx) -> Action {
        let Some(menu) = self.menu.take() else { return Action::None };
        let Some((choice, _)) = menu.choices.get(index) else { return Action::None };
        match choice {
            Choice::Resume => self.open(true),
            Choice::Open => self.open(false),
            Choice::StartOver => self.start_over(),
            Choice::Watchlist => {
                if let Some(title) = self.selected_title() {
                    ctx.watchlist.toggle(title);
                    self.resume(ctx);
                }
                Action::None
            }
            Choice::RemoveDownload => {
                self.remove_download(ctx);
                Action::None
            }
        }
    }

    fn remove_download(&mut self, ctx: &mut Ctx) {
        let rows = self.rows();
        let Some(Row::Download(item)) = self.selection.selected().and_then(|i| rows.get(i)) else { return };
        if let Some(work) = ctx.downloads.remove(&item.download.id) {
            ctx.clean_up(work);
        }
        self.resume(ctx);
    }

    fn edit_query(&mut self, ctx: &mut Ctx, edit: impl FnOnce(&mut String)) {
        edit(&mut self.query);
        self.selection.select(Some(0));
        let query = self.query.trim().to_string();
        if query.is_empty() {
            self.results.clear();
            self.search_request = None;
            return;
        }
        let work = async move {
            tokio::time::sleep(DEBOUNCE).await;
            catalog::search(&query).await.map_err(|e| e.to_string())
        };
        self.search_request = Some(ctx.request(work, Msg::Titles));
    }

    fn open(&self, resume: bool) -> Action {
        let rows = self.rows();
        let Some(row) = self.selection.selected().and_then(|i| rows.get(i)) else {
            return Action::None;
        };
        let title = self.with_details(row.title());
        let screen = match (row, title.kind) {
            (Row::Download(item), _) => {
                Screen::Playing(Box::new(Playing::new(title, item.download.episode.clone(), item.download.to_play())))
            }
            (Row::Continue(entry), _) if entry.episode.is_none() || (resume && !entry.finished()) => {
                Screen::Streams(Streams::new(title, entry.episode.clone()))
            }
            (_, Kind::Series) => Screen::Series(Series::new(title)),
            (_, Kind::Movie) => Screen::Streams(Streams::new(title, None)),
        };
        Action::Push(Box::new(screen))
    }

    fn start_over(&self) -> Action {
        let rows = self.rows();
        let Some(Row::Continue(entry)) = self.selection.selected().and_then(|i| rows.get(i)) else {
            return Action::None;
        };
        let streams = Streams::new(self.with_details(&entry.title), entry.episode.clone()).starting_over(true);
        Action::Push(Box::new(Screen::Streams(streams)))
    }

    pub fn on_msg(&mut self, msg: Msg, ctx: &mut Ctx) -> Result<Action, Msg> {
        match msg {
            Msg::Titles(id, result) => {
                if is_current(&self.search_request, id) {
                    self.search_request = None;
                    match result {
                        Ok(titles) => (self.results, self.error) = (titles, None),
                        Err(err) => self.error = Some(err),
                    }
                    self.selection.select(Some(0));
                }
            }
            Msg::Details(title_id, details) => {
                self.details.insert(title_id, details.map(|d| *d).unwrap_or_default());
            }
            Msg::Art(Art::Poster, title_id, poster) => {
                self.posters.insert(title_id, poster);
            }
            Msg::OnDisk(id, titles) if is_current(&self.on_disk_request, id) => {
                self.on_disk_request = None;
                self.keeping_tab(|home| home.on_disk = titles);
            }
            Msg::OnDisk(..) => {}
            other => return Err(other),
        }
        self.ensure_details(ctx);
        Ok(Action::None)
    }

    pub fn render(&mut self, frame: &mut Frame) {
        let sections = match self.searching() {
            true => vec![Line::from(Span::raw("Results").fg(ACCENT).bold().underlined())],
            false => widgets::tabs(&self.tab_labels(), self.tab, widgets::content_width(frame)),
        };
        let areas = widgets::areas(frame, "home", 2 + sections.len() as u16);
        self.tabs_area = if self.searching() { Rect::default() } else { widgets::below(areas.header, 2) };
        let prompt = Line::from(vec![
            Span::raw("🔍 "),
            Span::raw(self.query.clone()).bold(),
            Span::raw("▏").fg(ACCENT),
            Span::raw(if self.query.is_empty() { " search for a movie or series" } else { "" }).fg(DIM),
        ]);
        let header: Vec<Line> = [prompt, Line::default()].into_iter().chain(sections).collect();
        frame.render_widget(Paragraph::new(header), areas.header);

        let (list_area, details_area) = widgets::split_details(areas.body);
        self.list_area = list_area;
        let items: Vec<ListItem> =
            self.rows().iter().map(|row| row_item(row, self.in_watchlist(&row.title().id))).collect();
        if let Some(message) = self.empty_message(items.is_empty()) {
            widgets::message(frame, list_area, &message);
        } else {
            widgets::list(frame, list_area, items, &mut self.selection);
        }
        if let (Some(area), Some(title)) = (details_area, self.selected_title()) {
            let loading = title.details.is_empty() && !self.details.contains_key(&title.id);
            let poster = self.posters.get(&title.id).cloned().flatten();
            widgets::panel(frame, area, &title.name, widgets::title_details(&title, loading), poster.as_deref());
        }

        let hints: &[(&str, &str)] = match (self.menu.is_some(), self.searching()) {
            (true, _) => &[("↑↓", "move"), ("⏎", "choose"), ("esc", "close")],
            (false, true) => &[("↑↓", "move"), ("⏎", "open"), ("ctrl+s", "watchlist"), ("esc", "clear")],
            (false, false) => {
                &[("tab ←→", "list"), ("⏎", "open"), ("ctrl+s", "watchlist"), ("right click", "more"), ("esc", "quit")]
            }
        };
        widgets::footer(frame, areas.footer, hints);

        if let Some(menu) = &mut self.menu {
            let items = menu.choices.iter().map(|(_, label)| ListItem::new(label.clone())).collect();
            menu.area = widgets::popup(frame, menu.anchor, items, &mut menu.state);
        }
    }

    fn empty_message(&self, empty: bool) -> Option<String> {
        if self.searching() {
            if let Some(error) = &self.error {
                return Some(format!("Search failed: {error}"));
            }
            return empty.then(|| if self.search_request.is_some() { "Searching…" } else { "No results." }.into());
        }
        if !empty {
            return None;
        }
        Some(match self.current_tab()? {
            Tab::Watchlist => "Your watchlist is empty. Press ctrl+s on any movie or series to add it.".into(),
            Tab::Continue | Tab::Downloads | Tab::Recent => "Nothing here yet.".into(),
        })
    }
}

fn row_item(row: &Row, starred: bool) -> ListItem<'static> {
    let mut spans = vec![Span::raw(row.title().name.clone())];
    match row {
        Row::Title(_) | Row::OnDisk(_) => {
            let title = row.title();
            spans.push(Span::raw(format!("  {}", title.year())).fg(DIM));
            if let Some(rating) = title.details.rating() {
                spans.push(Span::raw(format!("  ★ {rating}")).fg(GOLD));
            }
            spans.push(Span::raw(format!("  {}", title.kind.label())).fg(DIM).italic());
            if matches!(row, Row::OnDisk(_)) {
                spans.push(Span::raw("  💾 on disk").fg(DIM));
            }
        }
        Row::Download(item) => {
            if let Some(e) = &item.download.episode {
                spans.push(Span::raw(format!("  {}", episode_tag(e.season, e.episode))).fg(ACCENT));
            }
            spans.push(Span::raw("  "));
            spans.extend(download_state(&item.state));
        }
        Row::Continue(entry) => {
            if let Some(e) = &entry.episode {
                spans.push(Span::raw(format!("  {}", episode_tag(e.season, e.episode))).fg(ACCENT));
            }
            spans.push(Span::raw("  "));
            spans.extend(widgets::progress(entry.ratio()));
        }
    }
    spans.extend(widgets::starred(starred));
    ListItem::new(Line::from(spans))
}

fn finished_count(items: &[Item]) -> usize {
    items.iter().filter(|item| matches!(item.state, State::Done)).count()
}

fn download_state(state: &State) -> Vec<Span<'static>> {
    match state {
        State::Done => vec![Span::raw("✓ downloaded").fg(DONE)],
        State::Starting => vec![Span::raw("connecting…").fg(DIM)],
        State::Failed(error) => vec![Span::raw(format!("✗ {error}")).red()],
        State::Saving { ratio, speed } => vec![
            Span::raw(widgets::bar(*ratio, 10)).fg(ACCENT),
            Span::raw(format!(" {:>3.0}%", ratio * 100.0)).fg(DIM),
            Span::raw(format!("  ⬇ {:.1} MB/s", speed / 1_000_000.0)).fg(DIM),
        ],
    }
}
