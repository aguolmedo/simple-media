use std::collections::{BTreeSet, HashMap};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use anyhow::Result;
use reqwest::StatusCode;
use serde::{Deserialize, Serialize};
use tokio::io::AsyncWriteExt;
use tokio::sync::mpsc::UnboundedSender;
use tokio::task::JoinHandle;

use crate::app::Msg;
use crate::archive;
use crate::catalog::{Episode, Title};
use crate::engine::{Engine, TorrentFile};
use crate::http;
use crate::library;
use crate::storage::{load_json, save_json};
use crate::streams::Torrent;

const FILE: &str = "downloads.json";
const SAMPLE_EVERY: Duration = Duration::from_secs(1);

#[derive(Clone, Serialize, Deserialize)]
pub struct Download {
    pub id: String,
    pub title: Title,
    pub episode: Option<Episode>,
    pub torrent: Torrent,
    pub dir: PathBuf,
    #[serde(default)]
    pub files: Vec<TorrentFile>,
    pub size: u64,
    pub done: bool,
}

impl Download {
    pub fn new(title: Title, episode: Option<Episode>, torrent: Torrent) -> Download {
        let source = torrent.archive.clone().unwrap_or_else(|| torrent.info_hash.clone());
        let file = torrent.file_idx.map(|i| i.to_string()).unwrap_or_default();
        Download {
            id: format!("{source}:{file}"),
            dir: library::folder_for(&title),
            size: torrent.size,
            title,
            episode,
            torrent,
            files: vec![],
            done: false,
        }
    }

    fn episode_key(&self) -> Option<(u32, u32)> {
        self.episode.as_ref().map(|e| (e.season, e.episode))
    }

    pub fn video(&self) -> Option<PathBuf> {
        self.files.first().map(|f| self.dir.join(&f.path))
    }

    fn paths(&self) -> Vec<PathBuf> {
        let partial = self.video().map(|video| partial(&video));
        self.files.iter().map(|f| self.dir.join(&f.path)).chain(partial).collect()
    }

    fn located(&mut self, file_idx: usize, files: Vec<TorrentFile>) {
        if let Some(video) = files.first() {
            self.size = if video.length > 0 { video.length } else { self.size };
            self.torrent.file_name = Some(video.name().to_string());
        }
        self.torrent.file_idx = Some(file_idx);
        self.files = files;
    }

    pub fn to_play(&self) -> Torrent {
        Torrent { local: self.video().filter(|_| self.done), ..self.torrent.clone() }
    }

    fn is_archive(&self) -> bool {
        self.torrent.archive.is_some()
    }
}

pub enum Event {
    Located(usize, Vec<TorrentFile>),
    Failed(String),
}

#[derive(Clone)]
pub enum State {
    Starting,
    Saving { ratio: f64, speed: f64 },
    Done,
    Failed(String),
}

#[derive(Clone)]
pub struct Item {
    pub download: Download,
    pub state: State,
}

#[derive(Default)]
struct Meter {
    saved: u64,
    sampled: u64,
    at: Option<Instant>,
    speed: f64,
}

impl Meter {
    fn update(&mut self, saved: u64, now: Instant) {
        self.saved = saved;
        let Some(at) = self.at else {
            (self.sampled, self.at) = (saved, Some(now));
            return;
        };
        let elapsed = now.duration_since(at);
        if elapsed >= SAMPLE_EVERY {
            self.speed = saved.saturating_sub(self.sampled) as f64 / elapsed.as_secs_f64();
            (self.sampled, self.at) = (saved, Some(now));
        }
    }
}

pub struct Downloads {
    entries: Vec<Download>,
    tasks: HashMap<String, JoinHandle<()>>,
    meters: HashMap<String, Meter>,
    errors: HashMap<String, String>,
    engine: Engine,
    tx: UnboundedSender<Msg>,
}

impl Downloads {
    pub fn load(engine: Engine, tx: UnboundedSender<Msg>) -> Downloads {
        let entries: Vec<Download> = load_json(FILE);
        let mut downloads =
            Downloads { entries: vec![], tasks: HashMap::new(), meters: HashMap::new(), errors: HashMap::new(), engine, tx };
        for download in entries.iter().filter(|d| !d.done) {
            downloads.start(download);
        }
        downloads.entries = entries;
        downloads
    }

    fn save(&self) {
        save_json(FILE, &self.entries);
    }

    pub fn add(&mut self, download: Download) -> bool {
        if self.entries.iter().any(|d| d.id == download.id) {
            return false;
        }
        self.start(&download);
        self.entries.insert(0, download);
        self.save();
        true
    }

    fn start(&mut self, download: &Download) {
        let key = download.id.clone();
        let (id, download, engine, tx) = (download.id.clone(), download.clone(), self.engine.clone(), self.tx.clone());
        let task = tokio::spawn(async move {
            if let Err(err) = fetch(download, engine, &tx, &id).await {
                let _ = tx.send(Msg::Download(id, Event::Failed(format!("{err:#}"))));
            }
        });
        self.tasks.insert(key, task);
    }

    pub fn on_event(&mut self, id: &str, event: Event) {
        match event {
            Event::Located(file_idx, files) => {
                if let Some(download) = self.entries.iter_mut().find(|d| d.id == id) {
                    download.located(file_idx, files);
                    self.save();
                }
            }
            Event::Failed(error) => {
                self.errors.insert(id.to_string(), error);
            }
        }
    }

    pub fn refresh(&mut self) {
        let now = Instant::now();
        let mut finished = Vec::new();
        for download in self.entries.iter_mut().filter(|d| !d.done) {
            let Some(progress) = progress(download, &self.engine) else { continue };
            let meter = self.meters.entry(download.id.clone()).or_default();
            meter.update(progress.saved, now);
            meter.speed = progress.speed.unwrap_or(meter.speed);
            if progress.complete {
                download.done = true;
                finished.push(download.id.clone());
            }
        }
        for id in &finished {
            self.meters.remove(id);
            self.tasks.remove(id);
        }
        if !finished.is_empty() {
            self.save();
        }
    }

    fn state_of(&self, download: &Download) -> State {
        if download.done {
            return State::Done;
        }
        if let Some(error) = self.errors.get(&download.id) {
            return State::Failed(error.clone());
        }
        match self.meters.get(&download.id) {
            Some(meter) => State::Saving { ratio: ratio(meter.saved, download.size), speed: meter.speed },
            None => State::Starting,
        }
    }

    pub fn items(&self) -> Vec<Item> {
        self.entries.iter().map(|d| Item { download: d.clone(), state: self.state_of(d) }).collect()
    }

    pub fn unfinished(&self) -> Vec<PathBuf> {
        self.entries.iter().filter(|d| !d.done).filter_map(Download::video).collect()
    }

    pub fn remove(&mut self, id: &str) -> Option<impl Future<Output = ()> + Send + 'static> {
        let index = self.entries.iter().position(|d| d.id == id)?;
        let download = self.entries.remove(index);
        self.save();
        if let Some(task) = self.tasks.remove(id) {
            task.abort();
        }
        self.meters.remove(id);
        self.errors.remove(id);
        let others: Vec<PathBuf> = self.entries.iter().flat_map(Download::paths).collect();
        let engine = self.engine.clone();
        Some(async move {
            if let (false, Some(file_idx)) = (download.is_archive(), download.torrent.file_idx) {
                engine.discard(&download.torrent.info_hash, file_idx).await;
            }
            delete_files(&download, &others);
        })
    }
}

fn ratio(saved: u64, size: u64) -> f64 {
    if size == 0 { 0.0 } else { (saved as f64 / size as f64).clamp(0.0, 1.0) }
}

fn partial(path: &Path) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(".part");
    PathBuf::from(name)
}

fn file_size(path: &Path) -> Option<u64> {
    std::fs::metadata(path).ok().map(|m| m.len())
}

struct Progress {
    saved: u64,
    complete: bool,
    speed: Option<f64>,
}

fn progress(download: &Download, engine: &Engine) -> Option<Progress> {
    let video = download.video()?;
    if download.is_archive() {
        return match file_size(&partial(&video)) {
            Some(saved) => Some(Progress { saved, complete: false, speed: None }),
            None => file_size(&video).map(|saved| Progress { saved, complete: true, speed: None }),
        };
    }
    let (saved, speed) = engine.saving(&download.torrent.info_hash, download.torrent.file_idx?)?;
    Some(Progress { saved, complete: download.size > 0 && saved >= download.size, speed: Some(speed) })
}

fn delete_files(download: &Download, others: &[PathBuf]) {
    let mine: Vec<PathBuf> = download.paths().into_iter().filter(|p| !others.contains(p)).collect();
    for path in &mine {
        let _ = std::fs::remove_file(path);
    }
    let folders: BTreeSet<PathBuf> = mine
        .iter()
        .flat_map(|p| p.ancestors().skip(1).take_while(|a| a.starts_with(&download.dir)).map(Path::to_path_buf).collect::<Vec<_>>())
        .collect();
    for folder in folders.iter().rev() {
        let _ = std::fs::remove_dir(folder);
    }
}

async fn fetch(download: Download, engine: Engine, tx: &UnboundedSender<Msg>, id: &str) -> Result<()> {
    let located = |file_idx, files| {
        let _ = tx.send(Msg::Download(id.to_string(), Event::Located(file_idx, files)));
    };
    let Some(identifier) = &download.torrent.archive else {
        let (hash, file_idx) = (&download.torrent.info_hash, download.torrent.file_idx);
        let saved = engine.download(hash, file_idx, download.episode_key(), &download.dir).await?;
        let file_idx = saved.first().map(|(i, _)| *i).unwrap_or_default();
        located(file_idx, saved.into_iter().map(|(_, f)| f).collect());
        return Ok(());
    };
    let playback = archive::play(identifier, download.torrent.file_idx).await?;
    let file = TorrentFile { path: playback.file.name().to_string(), length: playback.file.length };
    let target = download.dir.join(&file.path);
    located(playback.file_idx, vec![file]);
    save_to(&playback.url(), &target).await
}

async fn save_to(url: &str, path: &Path) -> Result<()> {
    if let Some(dir) = path.parent() {
        tokio::fs::create_dir_all(dir).await?;
    }
    let part = partial(path);
    let have = file_size(&part).unwrap_or(0);
    let response = http::download(url).header("range", format!("bytes={have}-")).send().await?;
    if response.status() == StatusCode::RANGE_NOT_SATISFIABLE && have > 0 {
        return Ok(tokio::fs::rename(&part, path).await?);
    }
    let mut response = response.error_for_status()?;
    let resumed = response.status() == StatusCode::PARTIAL_CONTENT;
    let mut file =
        tokio::fs::OpenOptions::new().create(true).write(true).append(resumed).truncate(!resumed).open(&part).await?;
    while let Some(chunk) = response.chunk().await? {
        file.write_all(&chunk).await?;
    }
    file.flush().await?;
    drop(file);
    Ok(tokio::fs::rename(&part, path).await?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Details, Kind};

    fn download(dir: &Path, files: &[&str]) -> Download {
        let title = Title { id: "tt1".into(), kind: Kind::Movie, name: "Movie".into(), release_info: None, details: Details::default() };
        let torrent: Torrent = serde_json::from_value(serde_json::json!({
            "info_hash": "abc", "name": "Movie", "seeds": 1, "leechers": null, "size": 10, "source": "YTS",
            "quality": null, "pack": false, "archive": null, "file_idx": 0, "file_name": null
        }))
        .unwrap();
        let mut download = Download::new(title, None, torrent);
        download.dir = dir.to_path_buf();
        download.files = files.iter().map(|path| TorrentFile { path: path.to_string(), length: 4 }).collect();
        download
    }

    #[test]
    fn measures_speed_once_per_second() {
        let start = Instant::now();
        let mut meter = Meter::default();
        meter.update(100, start);
        meter.update(600, start + Duration::from_millis(500));
        assert_eq!((meter.saved, meter.speed), (600, 0.0));
        meter.update(2100, start + Duration::from_secs(2));
        assert_eq!(meter.speed, 1000.0);
        assert_eq!(ratio(5, 10), 0.5);
        assert_eq!(ratio(5, 0), 0.0);
    }

    #[test]
    fn located_downloads_know_their_file() {
        let mut download = download(Path::new("/m/Movie"), &[]);
        assert_eq!(download.id, "abc:0");
        assert!(download.video().is_none());
        download.located(3, vec![TorrentFile { path: "Sub/Movie.mkv".into(), length: 42 }]);
        assert_eq!((download.size, download.torrent.file_idx), (42, Some(3)));
        assert_eq!(download.video(), Some(PathBuf::from("/m/Movie/Sub/Movie.mkv")));
        assert_eq!(download.torrent.file_name.as_deref(), Some("Movie.mkv"));
        assert!(download.to_play().local.is_none());
        download.done = true;
        assert_eq!(download.to_play().local, download.video());
    }

    #[test]
    fn removing_keeps_files_of_others_and_of_the_user() {
        let dir = std::env::temp_dir().join(format!("simple-media-downloads-{}", std::process::id()));
        std::fs::create_dir_all(dir.join("Subs")).unwrap();
        for name in ["E01.mkv", "E02.mkv", "Subs/E01.srt", "Subs/shared.srt", "mine.mkv"] {
            std::fs::write(dir.join(name), "data").unwrap();
        }
        let first = download(&dir, &["E01.mkv", "Subs/E01.srt", "Subs/shared.srt"]);
        let second = download(&dir, &["E02.mkv", "Subs/shared.srt"]);
        delete_files(&first, &second.paths());
        assert!(!dir.join("E01.mkv").exists() && !dir.join("Subs/E01.srt").exists());
        assert!(dir.join("Subs/shared.srt").exists() && dir.join("mine.mkv").exists());
        delete_files(&second, &[]);
        assert!(!dir.join("Subs").exists() && dir.join("mine.mkv").exists());
        std::fs::remove_dir_all(dir).unwrap();
    }
}
