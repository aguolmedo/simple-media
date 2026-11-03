use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::catalog::{Episode, Title};
use crate::storage::{load_json, save_json};

const FINISHED_RATIO: f64 = 0.9;
const MIN_RESUME_SECONDS: f64 = 30.0;

pub fn watched(ratio: f64) -> bool {
    ratio >= FINISHED_RATIO
}

pub fn ratio(position: f64, duration: f64) -> f64 {
    if duration > 0.0 { (position / duration).clamp(0.0, 1.0) } else { 0.0 }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Entry {
    pub title: Title,
    pub episode: Option<Episode>,
    pub position: f64,
    pub duration: f64,
    pub updated: u64,
}

impl Entry {
    pub fn new(title: Title, episode: Option<Episode>, position: f64, duration: f64) -> Entry {
        let updated = SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        Entry { title, episode, position, duration, updated }
    }

    fn key(&self) -> (String, Option<(u32, u32)>) {
        (self.title.id.clone(), self.episode.as_ref().map(|e| (e.season, e.episode)))
    }

    pub fn ratio(&self) -> f64 {
        ratio(self.position, self.duration)
    }

    pub fn finished(&self) -> bool {
        watched(self.ratio())
    }

    pub fn resume_at(&self) -> Option<f64> {
        (!self.finished() && self.position >= MIN_RESUME_SECONDS).then_some(self.position)
    }
}

pub struct History {
    entries: Vec<Entry>,
}

const FILE: &str = "history.json";

impl History {
    pub fn load() -> History {
        History { entries: load_json(FILE) }
    }

    pub fn record(&mut self, entry: Entry) {
        self.entries.retain(|e| e.key() != entry.key());
        self.entries.push(entry);
        save_json(FILE, &self.entries);
    }

    pub fn get(&self, title_id: &str, episode: Option<(u32, u32)>) -> Option<&Entry> {
        self.entries.iter().find(|e| e.key() == (title_id.to_string(), episode))
    }

    pub fn continue_watching(&self) -> Vec<Entry> {
        let mut latest: HashMap<&str, &Entry> = HashMap::new();
        for entry in &self.entries {
            let newer = latest.get(entry.title.id.as_str()).is_none_or(|e| e.updated < entry.updated);
            if newer {
                latest.insert(&entry.title.id, entry);
            }
        }
        let mut entries: Vec<Entry> = latest
            .into_values()
            .filter(|e| e.episode.is_some() || !e.finished())
            .cloned()
            .collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated));
        entries
    }

    pub fn recent_titles(&self) -> Vec<Title> {
        let mut entries: Vec<&Entry> = self.entries.iter().collect();
        entries.sort_by_key(|e| std::cmp::Reverse(e.updated));
        let mut titles: Vec<Title> = Vec::new();
        for entry in entries {
            if !titles.iter().any(|t| t.id == entry.title.id) {
                titles.push(entry.title.clone());
            }
        }
        titles
    }

    pub fn latest_episode(&self, title_id: &str) -> Option<&Entry> {
        self.entries
            .iter()
            .filter(|e| e.title.id == title_id && e.episode.is_some())
            .max_by_key(|e| e.updated)
    }

    pub fn progress_of(&self, title_id: &str) -> HashMap<(u32, u32), f64> {
        self.entries
            .iter()
            .filter(|e| e.title.id == title_id)
            .filter_map(|e| e.episode.as_ref().map(|ep| ((ep.season, ep.episode), e.ratio())))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Details, Kind};

    fn title(id: &str) -> Title {
        Title { id: id.into(), kind: Kind::Series, name: id.into(), release_info: None, details: Details::default() }
    }

    fn entry(id: &str, episode: Option<(u32, u32)>, position: f64, updated: u64) -> Entry {
        let episode = episode.map(|(season, episode)| Episode { season, episode, name: None, released: None });
        Entry { title: title(id), episode, position, duration: 100.0, updated }
    }

    #[test]
    fn continue_watching_keeps_the_latest_per_title() {
        let history = History {
            entries: vec![
                entry("show", Some((1, 1)), 95.0, 1),
                entry("show", Some((1, 2)), 40.0, 3),
                entry("movie", None, 95.0, 2),
                entry("film", None, 50.0, 4),
            ],
        };
        let continuing = history.continue_watching();
        let shown: Vec<(&str, f64)> = continuing.iter().map(|e| (e.title.id.as_str(), e.position)).collect();
        assert_eq!(shown, vec![("film", 50.0), ("show", 40.0)]);
        assert_eq!(history.latest_episode("show").and_then(|e| e.episode.as_ref()).map(|e| e.episode), Some(2));
        assert_eq!(history.progress_of("show").get(&(1, 1)), Some(&0.95));
    }

    #[test]
    fn resume_rules() {
        assert_eq!(entry("a", None, 10.0, 0).resume_at(), None);
        assert_eq!(entry("a", None, 50.0, 0).resume_at(), Some(50.0));
        assert!(entry("a", None, 90.0, 0).finished());
        assert_eq!(entry("a", None, 90.0, 0).resume_at(), None);
        assert_eq!(ratio(5.0, 0.0), 0.0);
    }
}
