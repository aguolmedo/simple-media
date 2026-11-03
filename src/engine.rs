use std::collections::{HashMap, HashSet};
use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use anyhow::{Context, Result, bail};
use librqbit::api::{Api, TorrentIdOrHash};
use librqbit::http_api::HttpApi;
use librqbit::{
    AddTorrent, AddTorrentOptions, AddTorrentResponse, ListOnlyResponse, ListenerOptions, ManagedTorrent, Session,
    SessionOptions,
};
use librqbit_dualstack_sockets::TcpListener;
use serde::{Deserialize, Serialize};

use crate::release::{is_video, matches_episode};
use crate::storage::cache_dir;

pub const TRACKERS: [&str; 5] = [
    "udp://tracker.opentrackr.org:1337/announce",
    "udp://open.stealth.si:80/announce",
    "udp://tracker.torrent.eu.org:451/announce",
    "udp://open.demonii.com:1337/announce",
    "udp://exodus.desync.com:6969/announce",
];

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct TorrentFile {
    pub path: String,
    pub length: u64,
}

impl TorrentFile {
    pub fn name(&self) -> &str {
        self.path.rsplit('/').next().unwrap_or(&self.path)
    }
}

#[derive(Clone)]
pub struct Engine {
    session: Arc<Session>,
    port: u16,
    listed: Arc<Mutex<HashMap<String, Arc<Listed>>>>,
    kept: Arc<Mutex<HashMap<String, Kept>>>,
}

struct Kept {
    torrent: Arc<ManagedTorrent>,
    videos: HashSet<usize>,
    subtitles: HashSet<usize>,
}

impl Kept {
    fn files(&self) -> HashSet<usize> {
        self.videos.union(&self.subtitles).copied().collect()
    }
}

struct Listed {
    files: Vec<TorrentFile>,
    torrent_bytes: Vec<u8>,
    seen_peers: Vec<SocketAddr>,
}

#[derive(Clone)]
enum Source {
    Torrent { torrent: Arc<ManagedTorrent>, base_url: String },
    Direct { base_url: String },
    Local { dir: PathBuf },
}

pub struct Playback {
    source: Source,
    pub id: String,
    pub file_idx: usize,
    pub file: TorrentFile,
    pub files: Vec<TorrentFile>,
}

#[derive(Default, Clone, Copy)]
pub struct Status {
    pub peers: u32,
    pub speed_mbps: f64,
    pub downloaded: u64,
}

fn magnet(info_hash: &str) -> String {
    let trackers: String = TRACKERS.iter().map(|t| format!("&tr={}", urlencoding::encode(t))).collect();
    format!("magnet:?xt=urn:btih:{info_hash}{trackers}")
}

fn add_options() -> AddTorrentOptions {
    AddTorrentOptions {
        overwrite: true,
        trackers: Some(TRACKERS.iter().map(|t| t.to_string()).collect()),
        ..Default::default()
    }
}

impl Listed {
    fn from_response(response: ListOnlyResponse) -> Listed {
        let files = response
            .info
            .iter_file_details()
            .map(|f| TorrentFile { path: f.filename.to_string(), length: f.len })
            .collect();
        Listed { files, torrent_bytes: response.torrent_bytes.to_vec(), seen_peers: response.seen_peers }
    }
}

fn is_subtitle(file: &TorrentFile) -> bool {
    file.path.to_lowercase().ends_with(".srt")
}

fn subtitle_indices(files: &[TorrentFile]) -> HashSet<usize> {
    (0..files.len()).filter(|&i| is_subtitle(&files[i])).collect()
}

fn episode_subtitles(files: &[TorrentFile], episode: Option<(u32, u32)>) -> HashSet<usize> {
    let videos = files.iter().filter(|f| is_video(&f.path)).count();
    let belongs = |file: &TorrentFile| match episode {
        Some((season, number)) if videos > 1 => matches_episode(&file.path, season, number),
        _ => true,
    };
    subtitle_indices(files).into_iter().filter(|&i| belongs(&files[i])).collect()
}

fn main_video(files: &[TorrentFile]) -> Option<usize> {
    files.iter().enumerate().filter(|(_, f)| is_video(&f.path)).max_by_key(|(_, f)| f.length).map(|(i, _)| i)
}

impl Engine {
    pub async fn start() -> Result<Engine> {
        let options = SessionOptions {
            listen: Some(ListenerOptions::default()),
            ..Default::default()
        };
        let session = Session::new_with_opts(cache_dir().join("torrents"), options).await?;
        let listener = TcpListener::bind_tcp(([127, 0, 0, 1], 0).into(), Default::default())?;
        let port = listener.bind_addr().port();
        let api = HttpApi::new(Api::new(session.clone(), None, None), None);
        tokio::spawn(api.make_http_api_and_run(listener, None));
        Ok(Engine { session, port, listed: Default::default(), kept: Default::default() })
    }

    async fn fetch_listing(&self, info_hash: &str) -> Result<Arc<Listed>> {
        let options = AddTorrentOptions { list_only: true, ..add_options() };
        let response = self.session.add_torrent(AddTorrent::from_url(magnet(info_hash)), Some(options)).await?;
        let AddTorrentResponse::ListOnly(response) = response else { bail!("unexpected response from the engine") };
        let listed = Arc::new(Listed::from_response(response));
        self.listed.lock().unwrap().insert(info_hash.to_string(), listed.clone());
        Ok(listed)
    }

    async fn listing(&self, info_hash: &str) -> Result<Arc<Listed>> {
        if let Some(listed) = self.listed.lock().unwrap().get(info_hash) {
            return Ok(listed.clone());
        }
        let engine = self.clone();
        let hash = info_hash.to_string();
        tokio::spawn(async move { engine.fetch_listing(&hash).await }).await?
    }

    pub async fn list_files(&self, info_hash: &str) -> Result<Vec<TorrentFile>> {
        Ok(self.listing(info_hash).await?.files.clone())
    }

    pub async fn play(&self, info_hash: &str, file_idx: Option<usize>) -> Result<Playback> {
        let listed = self.listing(info_hash).await?;
        let files = listed.files.clone();
        let file_idx = file_idx.or_else(|| main_video(&files)).context("the torrent has no video")?;
        let file = files.get(file_idx).context("the file is not in the torrent")?.clone();

        let mut wanted = self.with_kept(info_hash, subtitle_indices(&files));
        wanted.insert(file_idx);
        let torrent = self.add(&listed, &wanted, None).await?;
        torrent.wait_until_initialized().await?;

        let base_url = format!("http://127.0.0.1:{}/torrents/{}/stream", self.port, torrent.id());
        let source = Source::Torrent { torrent, base_url };
        Ok(Playback { source, id: info_hash.to_string(), file_idx, file, files })
    }

    async fn add(&self, listed: &Listed, wanted: &HashSet<usize>, dir: Option<&Path>) -> Result<Arc<ManagedTorrent>> {
        let options = AddTorrentOptions {
            only_files: Some(wanted.iter().copied().collect()),
            initial_peers: Some(listed.seen_peers.clone()),
            output_folder: dir.map(|dir| dir.to_string_lossy().into_owned()),
            ..add_options()
        };
        let response =
            self.session.add_torrent(AddTorrent::from_bytes(listed.torrent_bytes.clone()), Some(options)).await?;
        let already_managed = matches!(response, AddTorrentResponse::AlreadyManaged(..));
        let torrent = response.into_handle().context("the engine did not return the torrent")?;
        if already_managed {
            self.session.update_only_files(&torrent, wanted).await?;
        }
        Ok(torrent)
    }

    fn with_kept(&self, info_hash: &str, mut wanted: HashSet<usize>) -> HashSet<usize> {
        if let Some(kept) = self.kept.lock().unwrap().get(info_hash) {
            wanted.extend(kept.files());
        }
        wanted
    }

    fn kept_files(&self, info_hash: &str) -> Option<HashSet<usize>> {
        self.kept.lock().unwrap().get(info_hash).map(Kept::files)
    }

    fn kept_subtitles(&self, info_hash: &str) -> HashSet<usize> {
        self.kept.lock().unwrap().get(info_hash).map(|kept| kept.subtitles.clone()).unwrap_or_default()
    }

    fn kept_videos(&self, info_hash: &str) -> HashSet<usize> {
        self.kept.lock().unwrap().get(info_hash).map(|kept| kept.videos.clone()).unwrap_or_default()
    }

    pub async fn download(
        &self,
        info_hash: &str,
        file_idx: Option<usize>,
        episode: Option<(u32, u32)>,
        dir: &Path,
    ) -> Result<Vec<(usize, TorrentFile)>> {
        let listed = self.listing(info_hash).await?;
        let files = listed.files.clone();
        let file_idx = file_idx.or_else(|| main_video(&files)).context("the torrent has no video")?;
        let mut videos = self.kept_videos(info_hash);
        videos.insert(file_idx);
        let subtitles = self.kept_subtitles(info_hash).union(&episode_subtitles(&files, episode)).copied().collect();
        let wanted = videos.union(&subtitles).copied().collect();
        let torrent = self.add_into(&listed, &wanted, dir).await?;
        let mut saved: Vec<usize> = episode_subtitles(&files, episode).into_iter().collect();
        saved.sort();
        saved.insert(0, file_idx);
        self.kept.lock().unwrap().insert(info_hash.to_string(), Kept { torrent, videos, subtitles });
        Ok(saved.into_iter().map(|i| (i, files[i].clone())).collect())
    }

    async fn add_into(&self, listed: &Listed, wanted: &HashSet<usize>, dir: &Path) -> Result<Arc<ManagedTorrent>> {
        let torrent = self.add(listed, wanted, Some(dir)).await?;
        if torrent.output_folder() == dir {
            return Ok(torrent);
        }
        self.session.delete(TorrentIdOrHash::Id(torrent.id()), true).await?;
        self.add(listed, wanted, Some(dir)).await
    }

    pub fn saving(&self, info_hash: &str, file_idx: usize) -> Option<(u64, f64)> {
        let torrent = self.kept.lock().unwrap().get(info_hash)?.torrent.clone();
        let stats = torrent.stats();
        let speed = stats.live.map(|live| live.download_speed.as_bytes() as f64).unwrap_or(0.0);
        Some((*stats.file_progress.get(file_idx)?, speed))
    }

    pub async fn discard(&self, info_hash: &str, file_idx: usize) {
        let Some((torrent, remaining)) = self.release(info_hash, file_idx) else { return };
        let _ = match remaining {
            Some(files) => self.session.update_only_files(&torrent, &files).await,
            None => self.session.delete(TorrentIdOrHash::Id(torrent.id()), true).await,
        };
    }

    fn release(&self, info_hash: &str, file_idx: usize) -> Option<(Arc<ManagedTorrent>, Option<HashSet<usize>>)> {
        let mut kept = self.kept.lock().unwrap();
        let entry = kept.get_mut(info_hash)?;
        entry.videos.remove(&file_idx);
        if entry.videos.is_empty() {
            return kept.remove(info_hash).map(|entry| (entry.torrent, None));
        }
        Some((entry.torrent.clone(), Some(entry.files())))
    }

    pub async fn switch(&self, playback: &Playback, file_idx: usize) -> Result<Playback> {
        let file = playback.files.get(file_idx).context("the file is not in the torrent")?.clone();
        if let Source::Torrent { torrent, .. } = &playback.source {
            let wanted = self.with_kept(&playback.id, std::iter::once(file_idx).chain(playback.subtitle_files()).collect());
            self.session.update_only_files(torrent, &wanted).await?;
        }
        Ok(Playback {
            source: playback.source.clone(),
            id: playback.id.clone(),
            file_idx,
            file,
            files: playback.files.clone(),
        })
    }

    pub async fn stop(&self, playback: &Playback) {
        let Source::Torrent { torrent, .. } = &playback.source else { return };
        let _ = match self.kept_files(&playback.id) {
            Some(files) => self.session.update_only_files(torrent, &files).await,
            None => self.session.delete(TorrentIdOrHash::Id(torrent.id()), true).await,
        };
    }
}

impl Playback {
    pub fn direct(id: &str, base_url: String, files: Vec<TorrentFile>, file_idx: usize) -> Result<Playback> {
        let file = files.get(file_idx).context("the file is not in the item")?.clone();
        Ok(Playback { source: Source::Direct { base_url }, id: id.to_string(), file_idx, file, files })
    }

    pub fn local(id: &str, dir: PathBuf, files: Vec<TorrentFile>) -> Result<Playback> {
        let file = files.first().context("the download has no video")?.clone();
        Ok(Playback { source: Source::Local { dir }, id: id.to_string(), file_idx: 0, file, files })
    }

    pub fn is_direct(&self) -> bool {
        matches!(self.source, Source::Direct { .. })
    }

    pub fn is_local(&self) -> bool {
        matches!(self.source, Source::Local { .. })
    }

    pub fn url_for(&self, idx: usize) -> String {
        let Some(file) = self.files.get(idx) else { return String::new() };
        match &self.source {
            Source::Torrent { base_url, .. } => format!("{base_url}/{idx}/{}", urlencoding::encode(file.name())),
            Source::Direct { base_url } => {
                let path: Vec<String> = file.path.split('/').map(|part| urlencoding::encode(part).into_owned()).collect();
                format!("{base_url}/{}", path.join("/"))
            }
            Source::Local { dir } => dir.join(&file.path).to_string_lossy().into_owned(),
        }
    }

    pub fn url(&self) -> String {
        self.url_for(self.file_idx)
    }

    pub fn subtitle_files(&self) -> Vec<usize> {
        (0..self.files.len()).filter(|&i| is_subtitle(&self.files[i])).collect()
    }

    pub fn status(&self) -> Status {
        let Source::Torrent { torrent, .. } = &self.source else { return Status::default() };
        let stats = torrent.stats();
        let downloaded = stats.file_progress.get(self.file_idx).copied().unwrap_or(0);
        match stats.live {
            Some(live) => Status {
                peers: live.snapshot.peer_stats.live,
                speed_mbps: live.download_speed.mbps,
                downloaded,
            },
            None => Status { downloaded, ..Default::default() },
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(path: &str) -> TorrentFile {
        TorrentFile { path: path.into(), length: 1 }
    }

    #[test]
    fn keeps_the_subtitles_of_the_episode() {
        let pack = [file("S01E01.mkv"), file("S01E02.mkv"), file("Subs/S01E01.srt"), file("Subs/S01E02.srt")];
        assert_eq!(episode_subtitles(&pack, Some((1, 2))), HashSet::from([3]));
        let movie = [file("Movie.mp4"), file("English.srt"), file("Spanish.srt")];
        assert_eq!(episode_subtitles(&movie, None), HashSet::from([1, 2]));
    }
}
