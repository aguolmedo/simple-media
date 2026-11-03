use std::collections::HashMap;
use std::path::{Path, PathBuf};

use anyhow::{Context, Result};

use crate::catalog::{self, Title};
use crate::engine::{Playback, TorrentFile};
use crate::release::{is_video, matches_episode, normalize};
use crate::storage::{library_dir, load_json, save_json};
use crate::streams::{Target, Torrent};

const FILE: &str = "library.json";
const MAX_DEPTH: usize = 3;
pub const ON_DISK: &str = "On disk";

pub fn folder_for(title: &Title) -> PathBuf {
    let name: String = title.name.chars().filter(|c| !matches!(c, '/' | '\\' | ':' | '\0')).collect();
    let name = name.trim();
    let folder = match title.first_year() {
        Some(year) => format!("{name} ({year})"),
        None => name.to_string(),
    };
    library_dir().join(folder)
}

fn parse_folder(folder: &str) -> (String, Option<u32>) {
    let folder = folder.trim();
    let dated = folder.strip_suffix(')').and_then(|rest| rest.rsplit_once('('));
    match dated.and_then(|(name, year)| Some((name, year.trim().parse().ok()?))) {
        Some((name, year)) => (name.trim().to_string(), Some(year)),
        None => (folder.to_string(), None),
    }
}

fn file_name(path: &Path) -> String {
    path.file_name().map(|name| name.to_string_lossy().into_owned()).unwrap_or_default()
}

fn folders() -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(library_dir()) else { return vec![] };
    let mut folders: Vec<PathBuf> = entries.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect();
    folders.sort();
    folders
}

fn collect_videos(dir: &Path, depth: usize, found: &mut Vec<PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else { return };
    for path in entries.flatten().map(|e| e.path()) {
        if path.is_dir() && depth > 0 {
            collect_videos(&path, depth - 1, found);
        } else if path.is_file() && is_video(&file_name(&path)) {
            found.push(path);
        }
    }
}

fn videos_in(dir: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();
    collect_videos(dir, MAX_DEPTH, &mut found);
    found
}

fn is_folder_of(dir: &Path, name: &str, year: Option<u32>) -> bool {
    let (folder_name, folder_year) = parse_folder(&file_name(dir));
    let same_year = folder_year.is_none() || year.is_none() || folder_year == year;
    normalize(&folder_name) == normalize(name) && same_year
}

fn on_disk(path: PathBuf) -> Torrent {
    Torrent {
        info_hash: String::new(),
        name: file_name(&path),
        seeds: 0,
        leechers: None,
        size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
        source: ON_DISK.to_string(),
        quality: None,
        pack: false,
        archive: None,
        file_idx: None,
        file_name: None,
        local: Some(path),
    }
}

pub fn find(target: &Target, unfinished: &[PathBuf]) -> Vec<Torrent> {
    let wanted = |path: &PathBuf| {
        let episode = target.episode.is_none_or(|(season, episode)| matches_episode(&file_name(path), season, episode));
        episode && !unfinished.contains(path)
    };
    let mut found: Vec<Torrent> = folders()
        .into_iter()
        .filter(|dir| is_folder_of(dir, &target.name, target.year))
        .flat_map(|dir| videos_in(&dir))
        .filter(wanted)
        .map(on_disk)
        .collect();
    found.sort_by_key(|t| std::cmp::Reverse(t.size));
    found
}

fn stem(name: &str) -> &str {
    name.rsplit_once('.').map_or(name, |(stem, _)| stem)
}

fn subtitles_for(video: &str, names: &[String]) -> Vec<String> {
    let videos = names.iter().filter(|name| is_video(name)).count();
    let belongs = |name: &&String| videos <= 1 || name.starts_with(stem(video));
    names.iter().filter(|name| name.to_lowercase().ends_with(".srt")).filter(belongs).cloned().collect()
}

pub fn playback(video: &Path) -> Result<Playback> {
    let dir = video.parent().context("the file has no folder")?.to_path_buf();
    let name = file_name(video);
    let length = std::fs::metadata(video).context("the file is not on disk")?.len();
    let names: Vec<String> = std::fs::read_dir(&dir)?.flatten().map(|e| file_name(&e.path())).collect();
    let subtitles = subtitles_for(&name, &names).into_iter().map(|path| {
        let length = std::fs::metadata(dir.join(&path)).map(|m| m.len()).unwrap_or(0);
        TorrentFile { path, length }
    });
    let files = std::iter::once(TorrentFile { path: name.clone(), length }).chain(subtitles).collect();
    Playback::local(&name, dir, files)
}

fn pick(titles: Vec<Title>, name: &str, year: Option<u32>) -> Option<Title> {
    let same_year = |t: &&Title| year.is_none() || t.first_year() == year;
    let same_name = |t: &&Title| normalize(&t.name) == normalize(name);
    let exact = titles.iter().filter(same_year).find(same_name);
    exact.or_else(|| titles.iter().find(same_year)).cloned()
}

async fn identify(folder: &str) -> Result<Option<Title>> {
    let (name, year) = parse_folder(folder);
    Ok(pick(catalog::search(&name).await?, &name, year))
}

pub async fn titles() -> Vec<Title> {
    let mut known: HashMap<String, Option<Title>> = load_json(FILE);
    let before = known.len();
    let mut titles = Vec::new();
    for dir in folders().into_iter().filter(|dir| !videos_in(dir).is_empty()) {
        let folder = file_name(&dir);
        if !known.contains_key(&folder) {
            let Ok(found) = identify(&folder).await else { continue };
            known.insert(folder.clone(), found);
        }
        if let Some(Some(title)) = known.get(&folder) {
            titles.push(title.clone());
        }
    }
    if known.len() != before {
        save_json(FILE, &known);
    }
    titles
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::catalog::{Details, Kind};

    fn title(name: &str, year: &str) -> Title {
        Title { id: name.into(), kind: Kind::Movie, name: name.into(), release_info: Some(year.into()), details: Details::default() }
    }

    #[test]
    fn reads_folder_names() {
        assert_eq!(parse_folder("Marty Supreme (2025)"), ("Marty Supreme".to_string(), Some(2025)));
        assert_eq!(parse_folder("Serial Experiments Lain (1998) "), ("Serial Experiments Lain".to_string(), Some(1998)));
        assert_eq!(parse_folder("Some Movie"), ("Some Movie".to_string(), None));
        assert_eq!(parse_folder("Movie (Director's cut)"), ("Movie (Director's cut)".to_string(), None));
        assert!(is_folder_of(Path::new("/m/La estanquera de vallecas (1987)"), "La Estanquera de Vallecas", Some(1987)));
        assert!(!is_folder_of(Path::new("/m/Dune (1984)"), "Dune", Some(2021)));
        assert_eq!(folder_for(&title("Face/Off", "1997")).file_name().unwrap(), "FaceOff (1997)");
    }

    #[test]
    fn picks_the_title_with_the_same_name_and_year() {
        let titles = vec![title("Dune Part Two", "2024"), title("Dune", "2021"), title("Dune", "1984")];
        assert_eq!(pick(titles.clone(), "Dune", Some(1984)).map(|t| t.first_year()), Some(Some(1984)));
        assert_eq!(pick(titles.clone(), "Dune", None).map(|t| t.name), Some("Dune".to_string()));
        assert!(pick(titles, "Dune", Some(1999)).is_none());
    }

    #[test]
    fn subtitles_next_to_the_video() {
        let names = |list: &[&str]| list.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let single = names(&["Movie.mp4", "Movie.srt", "English.srt", "notes.txt"]);
        assert_eq!(subtitles_for("Movie.mp4", &single), names(&["Movie.srt", "English.srt"]));
        let pack = names(&["Show.S01E01.mkv", "Show.S01E02.mkv", "Show.S01E01.eng.srt", "Show.S01E02.srt"]);
        assert_eq!(subtitles_for("Show.S01E01.mkv", &pack), names(&["Show.S01E01.eng.srt"]));
    }
}
