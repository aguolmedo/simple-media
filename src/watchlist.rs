use crate::catalog::Title;
use crate::storage::{load_json, save_json};

const FILE: &str = "watchlist.json";

pub struct Watchlist {
    titles: Vec<Title>,
}

impl Watchlist {
    pub fn load() -> Watchlist {
        Watchlist { titles: load_json(FILE) }
    }

    pub fn titles(&self) -> &[Title] {
        &self.titles
    }

    pub fn contains(&self, title_id: &str) -> bool {
        self.titles.iter().any(|t| t.id == title_id)
    }

    pub fn toggle(&mut self, title: Title) {
        if self.contains(&title.id) {
            self.titles.retain(|t| t.id != title.id);
        } else {
            self.titles.insert(0, title);
        }
        save_json(FILE, &self.titles);
    }
}
