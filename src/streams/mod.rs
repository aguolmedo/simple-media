mod providers;

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::Duration;

use futures::future::join_all;
use serde::{Deserialize, Serialize};

use crate::archive;
use crate::engine::{Engine, TorrentFile};
use crate::release::{detect_languages, is_not_a_video, is_video, matches_episode, parse_quality, quality_rank};

pub const ARCHIVE: &str = "Archive.org";
const MAX_PACKS: usize = 8;
const PACK_WAIT: Duration = Duration::from_secs(8);

pub struct Target {
    pub imdb_id: String,
    pub name: String,
    pub year: Option<u32>,
    pub episode: Option<(u32, u32)>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Torrent {
    pub info_hash: String,
    pub name: String,
    pub seeds: u64,
    pub leechers: Option<u64>,
    pub size: u64,
    pub source: String,
    pub quality: Option<String>,
    pub pack: bool,
    pub archive: Option<String>,
    pub file_idx: Option<usize>,
    pub file_name: Option<String>,
    #[serde(default)]
    pub local: Option<PathBuf>,
}

impl Torrent {
    pub fn quality(&self) -> String {
        self.quality.clone().unwrap_or_else(|| parse_quality(&self.name))
    }

    pub fn peers(&self) -> String {
        if self.archive.is_some() {
            return "archive".to_string();
        }
        if self.local.is_some() {
            return "on disk".to_string();
        }
        let leechers = self.leechers.map(|l| l.to_string()).unwrap_or_else(|| "?".into());
        format!("{:>4} / {leechers:<4}", self.seeds)
    }

    pub fn full_name(&self) -> String {
        format!("{} {}", self.name, self.file_name.as_deref().unwrap_or(""))
    }

    pub fn languages(&self) -> String {
        let langs = detect_languages(&self.full_name());
        let audio = match (langs.audio.is_empty(), langs.multi) {
            (false, _) => format!("🔊 {}", langs.audio.join(" ")),
            (true, true) => "🔊 Multi".to_string(),
            (true, false) => String::new(),
        };
        let subs = match langs.subs.is_empty() {
            false => format!("💬 {}", langs.subs.join(" ")),
            true => String::new(),
        };
        [audio, subs].into_iter().filter(|s| !s.is_empty()).collect::<Vec<_>>().join("  ")
    }
}

pub struct Found {
    pub torrents: Vec<Torrent>,
    pub failures: Vec<String>,
}

pub async fn find(target: &Target, engine: &Engine) -> Found {
    let results = join_all(providers::ALL.iter().map(|p| p.find(target))).await;
    let mut torrents = Vec::new();
    let mut failures = Vec::new();
    for (provider, result) in providers::ALL.iter().zip(results) {
        match result {
            Ok(found) => torrents.extend(found),
            Err(err) => failures.push(format!("{}: {err}", provider.name())),
        }
    }

    torrents.retain(|t| !is_not_a_video(&t.name));
    let mut torrents = dedupe(torrents);
    if let Some((season, episode)) = target.episode {
        let (packs, singles): (Vec<_>, Vec<_>) = torrents.into_iter().partition(|t| t.pack);
        torrents = singles;
        torrents.extend(resolve_packs(packs, season, episode, engine).await);
    }

    torrents.sort_by_cached_key(|t| (quality_rank(&t.quality()), std::cmp::Reverse(t.seeds)));
    Found { torrents, failures }
}

fn dedupe(torrents: Vec<Torrent>) -> Vec<Torrent> {
    let mut by_hash: HashMap<String, Torrent> = HashMap::new();
    for t in torrents {
        match by_hash.get(&t.info_hash) {
            Some(prev) if prev.seeds >= t.seeds => {}
            _ => {
                by_hash.insert(t.info_hash.clone(), t);
            }
        }
    }
    by_hash.into_values().collect()
}

pub fn episode_file_index(files: &[TorrentFile], season: u32, episode: u32) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| is_video(&f.path) && matches_episode(&f.path, season, episode))
        .max_by_key(|(_, f)| f.length)
        .map(|(i, _)| i)
}

async fn resolve_pack(pack: Torrent, season: u32, episode: u32, engine: &Engine) -> Option<Torrent> {
    let listing = async {
        match &pack.archive {
            Some(identifier) => archive::list_files(identifier).await,
            None => engine.list_files(&pack.info_hash).await,
        }
    };
    let files = tokio::time::timeout(PACK_WAIT, listing).await.ok()?.ok()?;
    let idx = episode_file_index(&files, season, episode)?;
    let file = &files[idx];
    Some(Torrent {
        file_idx: Some(idx),
        file_name: Some(file.name().to_string()),
        size: file.length,
        ..pack
    })
}

async fn resolve_packs(mut packs: Vec<Torrent>, season: u32, episode: u32, engine: &Engine) -> Vec<Torrent> {
    packs.sort_by_key(|p| std::cmp::Reverse(p.seeds));
    packs.truncate(MAX_PACKS);
    let resolved = join_all(packs.into_iter().map(|pack| resolve_pack(pack, season, episode, engine))).await;
    resolved.into_iter().flatten().collect()
}
