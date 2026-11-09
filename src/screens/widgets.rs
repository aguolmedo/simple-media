use crossterm::event::{KeyCode, MouseEvent, MouseEventKind};
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Margin, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear, List, ListItem, ListState, Padding, Paragraph, Wrap};

use crate::catalog::{Episode, Title};
use crate::history::watched;
use crate::poster::{Backdrop, Image, Mosaic};
use crate::release::episode_tag;

pub const ACCENT: Color = Color::Cyan;
pub const DIM: Color = Color::DarkGray;
pub const GOLD: Color = Color::Yellow;
pub const DONE: Color = Color::Green;

const DETAILS_MIN_WIDTH: u16 = 70;
const POSTER_MAX_HEIGHT: u16 = 20;
const POSTER_MIN_HEIGHT: u16 = 6;
const POSTER_MIN_TEXT: u16 = 24;

pub struct Areas {
    pub header: Rect,
    pub body: Rect,
    pub footer: Rect,
}

pub fn areas(frame: &mut Frame, section: &str, header_height: u16) -> Areas {
    let area = frame.area();
    frame.render_widget(Clear, area);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Line::from(vec![
            Span::raw(" simple-media ").fg(ACCENT).bold(),
            Span::raw(format!("· {section} ")).fg(DIM),
        ]));
    let inner = block.inner(area).inner(Margin { horizontal: 1, vertical: 0 });
    frame.render_widget(block, area);
    let [header, body, footer] =
        Layout::vertical([Constraint::Length(header_height), Constraint::Fill(1), Constraint::Length(1)])
            .spacing(1)
            .areas(inner);
    Areas { header, body, footer }
}

pub fn backdrop(frame: &mut Frame, image: Option<&Image>) {
    if let Some(image) = image {
        frame.render_widget(Backdrop(image), frame.area().inner(Margin { horizontal: 1, vertical: 1 }));
    }
}

pub fn split_details(body: Rect) -> (Rect, Option<Rect>) {
    if body.width < DETAILS_MIN_WIDTH {
        return (body, None);
    }
    let [list, details] = Layout::horizontal([Constraint::Percentage(58), Constraint::Percentage(42)])
        .spacing(2)
        .areas(body);
    (list, Some(details))
}

pub fn panel(frame: &mut Frame, area: Rect, title: &str, lines: Vec<Line>, poster: Option<&Image>) {
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(DIM))
        .title(Span::raw(format!(" {title} ")).bold())
        .padding(Padding::horizontal(1));
    let inner = block.inner(area);
    frame.render_widget(block, area);
    let text = match poster.and_then(|p| place_poster(inner, p).map(|areas| (p, areas))) {
        Some((poster, (image, rest))) => {
            frame.render_widget(Mosaic(poster), image);
            rest
        }
        None => inner,
    };
    frame.render_widget(Paragraph::new(lines).wrap(Wrap { trim: true }), text);
}

fn place_poster(area: Rect, poster: &Image) -> Option<(Rect, Rect)> {
    poster_beside(area, poster).or_else(|| poster_above(area, poster))
}

fn poster_beside(area: Rect, poster: &Image) -> Option<(Rect, Rect)> {
    let fits = |height: &u16| poster.width_for(*height) + 2 + POSTER_MIN_TEXT <= area.width;
    let height = (POSTER_MIN_HEIGHT..=area.height.min(POSTER_MAX_HEIGHT)).rev().find(fits)?;
    let width = poster.width_for(height);
    let [image, _, rest] =
        Layout::horizontal([Constraint::Length(width), Constraint::Length(2), Constraint::Fill(1)]).areas(area);
    Some((Rect { height, ..image }, rest))
}

fn poster_above(area: Rect, poster: &Image) -> Option<(Rect, Rect)> {
    let fits = |height: &u16| poster.width_for(*height) <= area.width;
    let height = (POSTER_MIN_HEIGHT..=(area.height / 2).min(POSTER_MAX_HEIGHT)).rev().find(fits)?;
    let width = poster.width_for(height);
    let [image, _, rest] =
        Layout::vertical([Constraint::Length(height), Constraint::Length(1), Constraint::Fill(1)]).areas(area);
    Some((Rect { width, ..image }, rest))
}

pub fn title_details(title: &Title, loading: bool) -> Vec<Line<'static>> {
    let details = &title.details;
    let mut facts = vec![Span::raw(title.year()), Span::raw(" · ").fg(DIM), Span::raw(title.kind.label())];
    if let Some(runtime) = &details.runtime {
        facts.extend([Span::raw(" · ").fg(DIM), Span::raw(runtime.clone())]);
    }
    if let Some(rating) = details.rating() {
        facts.extend([Span::raw(" · ").fg(DIM), Span::raw(format!("★ {rating}")).fg(GOLD)]);
    }
    let mut lines = vec![Line::from(facts)];
    if !details.genres.is_empty() {
        lines.push(Line::from(details.genres.join(", ")).fg(ACCENT));
    }
    lines.push(Line::default());
    match &details.description {
        Some(description) => lines.push(Line::from(description.clone())),
        None if loading => lines.push(Line::from("Loading details…").fg(DIM)),
        None => lines.push(Line::from("No synopsis.").fg(DIM)),
    }
    if !details.cast.is_empty() {
        lines.push(Line::default());
        let cast: Vec<String> = details.cast.iter().take(4).cloned().collect();
        lines.push(Line::from(vec![Span::raw("Cast: ").fg(DIM), Span::raw(cast.join(", "))]));
    }
    lines
}

pub fn starred(starred: bool) -> Option<Span<'static>> {
    starred.then(|| Span::raw("  🔖").fg(GOLD))
}

pub fn content_width(frame: &Frame) -> usize {
    frame.area().width.saturating_sub(4).into()
}

const TAB_GAP: usize = 3;

fn tab_spots(labels: &[impl AsRef<str>], width: usize) -> Vec<(usize, usize)> {
    let mut spots = Vec::new();
    let (mut line, mut used) = (0, 0);
    for label in labels {
        let len = label.as_ref().chars().count();
        if used > 0 && used + TAB_GAP + len > width {
            (line, used) = (line + 1, 0);
        }
        if used > 0 {
            used += TAB_GAP;
        }
        spots.push((line, used));
        used += len;
    }
    spots
}

pub fn tabs(labels: &[impl AsRef<str>], selected: usize, width: usize) -> Vec<Line<'static>> {
    let mut lines: Vec<Vec<Span<'static>>> = vec![vec![]];
    for (i, (label, (line, _))) in labels.iter().zip(tab_spots(labels, width)).enumerate() {
        if line == lines.len() {
            lines.push(vec![]);
        }
        let spans = lines.last_mut().expect("there is always a line");
        if !spans.is_empty() {
            spans.push(Span::raw(" ".repeat(TAB_GAP)));
        }
        spans.push(match i == selected {
            true => Span::raw(label.as_ref().to_string()).fg(ACCENT).bold().underlined(),
            false => Span::raw(label.as_ref().to_string()).fg(DIM),
        });
    }
    lines.into_iter().map(Line::from).collect()
}

pub fn tab_at(labels: &[impl AsRef<str>], area: Rect, at: Position) -> Option<usize> {
    if !area.contains(at) {
        return None;
    }
    let (row, column) = (usize::from(at.y - area.y), usize::from(at.x - area.x));
    tab_spots(labels, area.width.into())
        .into_iter()
        .zip(labels)
        .position(|((line, start), label)| line == row && (start..start + label.as_ref().chars().count()).contains(&column))
}

pub fn below(area: Rect, skip: u16) -> Rect {
    let skip = skip.min(area.height);
    Rect { y: area.y + skip, height: area.height - skip, ..area }
}

pub enum Hit {
    Miss,
    Selected,
    Activated,
}

pub fn click_row(state: &mut ListState, area: Rect, heights: impl IntoIterator<Item = u16>, at: Position) -> Hit {
    if !area.contains(at) {
        return Hit::Miss;
    }
    let mut top = area.y;
    for (index, height) in heights.into_iter().enumerate().skip(state.offset()) {
        if (top..top + height).contains(&at.y) {
            if state.selected() == Some(index) {
                return Hit::Activated;
            }
            state.select(Some(index));
            return Hit::Selected;
        }
        top += height;
    }
    Hit::Miss
}

pub fn scroll(state: &mut ListState, len: usize, kind: MouseEventKind) -> bool {
    match kind {
        MouseEventKind::ScrollUp => navigate(state, len, KeyCode::Up),
        MouseEventKind::ScrollDown => navigate(state, len, KeyCode::Down),
        _ => false,
    }
}

pub fn position(mouse: &MouseEvent) -> Position {
    Position::new(mouse.column, mouse.row)
}

const HINT_GAP: u16 = 3;

pub fn footer(frame: &mut Frame, area: Rect, hints: &[(&str, &str)]) {
    let mut spans = Vec::new();
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            spans.push(Span::raw(" · ").fg(DIM));
        }
        spans.push(Span::raw(*key).fg(ACCENT));
        spans.push(Span::raw(format!(" {label}")).fg(DIM));
    }
    frame.render_widget(Paragraph::new(Line::from(spans)), area);
}

pub fn hint_at(hints: &[(&str, &str)], area: Rect, at: Position) -> Option<usize> {
    if !area.contains(at) {
        return None;
    }
    let mut start = area.x;
    for (i, (key, label)) in hints.iter().enumerate() {
        if i > 0 {
            start += HINT_GAP;
        }
        let width = Line::from(format!("{key} {label}")).width() as u16;
        if (start..start + width).contains(&at.x) {
            return Some(i);
        }
        start += width;
    }
    None
}

pub fn message(frame: &mut Frame, area: Rect, text: &str) {
    frame.render_widget(Paragraph::new(text.to_string()).fg(DIM).wrap(Wrap { trim: true }), area);
}

pub fn list(frame: &mut Frame, area: Rect, items: Vec<ListItem>, state: &mut ListState) {
    let list = List::new(items)
        .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌ ");
    frame.render_stateful_widget(list, area, state);
}

pub fn popup(frame: &mut Frame, anchor: Position, items: Vec<ListItem>, state: &mut ListState) -> Rect {
    let bounds = frame.area();
    let width = (items.iter().map(ListItem::width).max().unwrap_or(0) as u16 + 5).min(bounds.width);
    let height = (items.len() as u16 + 2).min(bounds.height);
    let x = anchor.x.min(bounds.right().saturating_sub(width));
    let y = anchor.y.min(bounds.bottom().saturating_sub(height));
    let area = Rect::new(x, y, width, height);
    frame.render_widget(Clear, area);
    let block = Block::bordered().border_type(BorderType::Rounded).border_style(Style::new().fg(ACCENT));
    let list = List::new(items)
        .block(block)
        .highlight_style(Style::new().fg(ACCENT).add_modifier(Modifier::BOLD))
        .highlight_symbol("▌ ");
    frame.render_stateful_widget(list, area, state);
    area
}

pub fn navigate(state: &mut ListState, len: usize, code: KeyCode) -> bool {
    if len == 0 {
        return matches!(code, KeyCode::Up | KeyCode::Down | KeyCode::PageUp | KeyCode::PageDown);
    }
    let current = state.selected().unwrap_or(0);
    let next = match code {
        KeyCode::Up => current.saturating_sub(1),
        KeyCode::Down => (current + 1).min(len - 1),
        KeyCode::PageUp => current.saturating_sub(10),
        KeyCode::PageDown => (current + 10).min(len - 1),
        KeyCode::Home => 0,
        KeyCode::End => len - 1,
        _ => return false,
    };
    state.select(Some(next));
    true
}

pub fn bar(ratio: f64, width: usize) -> String {
    let filled = ((ratio.clamp(0.0, 1.0) * width as f64).round() as usize).min(width);
    format!("{}{}", "█".repeat(filled), "░".repeat(width - filled))
}

pub fn progress(ratio: f64) -> Vec<Span<'static>> {
    if watched(ratio) {
        return vec![Span::raw("✓ watched").fg(DONE)];
    }
    vec![Span::raw(bar(ratio, 10)).fg(ACCENT), Span::raw(format!(" {:>3.0}%", ratio * 100.0)).fg(DIM)]
}

pub fn clock(seconds: f64) -> String {
    let total = seconds.max(0.0) as u64;
    let (h, m, s) = (total / 3600, total / 60 % 60, total % 60);
    match h {
        0 => format!("{m}:{s:02}"),
        _ => format!("{h}:{m:02}:{s:02}"),
    }
}

pub fn heading(title: &Title, episode: Option<&Episode>) -> String {
    match episode {
        Some(e) => format!("{} · {}", title.name, episode_tag(e.season, e.episode)),
        None if title.year().is_empty() => title.name.clone(),
        None => format!("{} ({})", title.name, title.year()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn tabs_wrap_and_are_clickable() {
        let labels = ["Continue watching", "Watchlist", "Recently watched"];
        assert_eq!(tab_spots(&labels, 40), vec![(0, 0), (0, 20), (1, 0)]);
        let area = Rect::new(10, 5, 40, 2);
        assert_eq!(tab_at(&labels, area, Position::new(31, 5)), Some(1));
        assert_eq!(tab_at(&labels, area, Position::new(28, 5)), None);
        assert_eq!(tab_at(&labels, area, Position::new(12, 6)), Some(2));
        assert_eq!(clock(3725.0), "1:02:05");
        assert_eq!(bar(0.5, 4), "██░░");
        let hints = [("⏎", "watch next"), ("esc", "back")];
        let footer = Rect::new(2, 20, 40, 1);
        assert_eq!(hint_at(&hints, footer, Position::new(3, 20)), Some(0));
        assert_eq!(hint_at(&hints, footer, Position::new(15, 20)), None);
        assert_eq!(hint_at(&hints, footer, Position::new(17, 20)), Some(1));
        assert_eq!(hint_at(&hints, footer, Position::new(17, 19)), None);
    }
}
