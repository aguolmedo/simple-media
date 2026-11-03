use anyhow::{Context, Result};
use serde::Deserialize;

use crate::engine::{Playback, TorrentFile};
use crate::http;
use crate::release::is_video;

const ARCHIVE: &str = "https://archive.org";

#[derive(Deserialize)]
struct Metadata {
    #[serde(default)]
    files: Vec<ItemFile>,
}

#[derive(Deserialize)]
struct ItemFile {
    name: String,
    #[serde(default)]
    size: Option<String>,
    #[serde(default)]
    source: Option<String>,
}

impl ItemFile {
    fn length(&self) -> u64 {
        self.size.as_deref().and_then(|s| s.parse().ok()).unwrap_or(0)
    }

    fn is_derivative(&self) -> bool {
        self.source.as_deref() == Some("derivative")
    }
}

fn best_video(files: &[ItemFile]) -> Option<usize> {
    files
        .iter()
        .enumerate()
        .filter(|(_, f)| is_video(&f.name))
        .max_by_key(|(_, f)| (f.is_derivative(), f.length()))
        .map(|(i, _)| i)
}

async fn metadata(identifier: &str) -> Result<Metadata> {
    http::get_json(&format!("{ARCHIVE}/metadata/{}", urlencoding::encode(identifier))).await
}

fn to_files(metadata: &Metadata) -> Vec<TorrentFile> {
    metadata.files.iter().map(|f| TorrentFile { path: f.name.clone(), length: f.length() }).collect()
}

pub async fn list_files(identifier: &str) -> Result<Vec<TorrentFile>> {
    Ok(to_files(&metadata(identifier).await?))
}

pub async fn play(identifier: &str, file_idx: Option<usize>) -> Result<Playback> {
    let metadata = metadata(identifier).await?;
    let file_idx = file_idx.or_else(|| best_video(&metadata.files)).context("the item has no video")?;
    let base_url = format!("{ARCHIVE}/download/{}", urlencoding::encode(identifier));
    Playback::direct(identifier, base_url, to_files(&metadata), file_idx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file(name: &str, size: u64, source: &str) -> ItemFile {
        ItemFile { name: name.into(), size: Some(size.to_string()), source: Some(source.into()) }
    }

    #[test]
    fn prefers_the_largest_streaming_copy() {
        let files = [
            file("movie.mpeg", 3_292_000_000, "original"),
            file("movie.mp4", 575_000_000, "derivative"),
            file("movie_512kb.mp4", 401_000_000, "derivative"),
            file("movie_archive.torrent", 1000, "metadata"),
        ];
        assert_eq!(best_video(&files), Some(1));
        assert_eq!(best_video(&files[..1]), Some(0));
        assert_eq!(best_video(&files[3..]), None);
    }
}
