mod decode;
mod languages;
mod providers;
mod rank;
mod video;

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;

use futures::future::join_all;
use tokio::sync::oneshot;

use crate::catalog::{self, Kind};
use crate::engine::Playback;
use crate::release::{Release, describe_release, is_video, matches_episode};
use crate::storage::cache_dir;
use decode::Hints;
use languages::code_from_filename;
use rank::Context;
pub use rank::Measured;

const MAX_PER_LANG: usize = 3;
const PREFERRED_LANGUAGES: [&str; 2] = ["spa", "eng"];
const EMBEDDED_SCORE: f64 = 1000.0;

pub struct Query {
    pub imdb_id: String,
    pub episode: Option<(u32, u32)>,
    pub release: Release,
    pub filename: String,
    pub video_hash: Option<String>,
    pub video_size: u64,
}

pub enum Download {
    OpenSubtitles { link: String, file_id: String },
    Direct { url: String },
    Yify { slug: String },
}

pub struct Candidate {
    pub id: String,
    pub lang: String,
    pub release: String,
    pub file_name: String,
    pub group: String,
    pub format: String,
    pub fps: Option<f64>,
    pub last_cue: Option<f64>,
    pub hash_match: bool,
    pub timing_group: Option<String>,
    pub downloads: u64,
    pub trusted: bool,
    pub rating: f64,
    pub hearing_impaired: bool,
    pub rejected: bool,
    pub download: Download,
    pub hints: Hints,
}

#[derive(Clone, Debug)]
pub struct Subtitle {
    pub path: PathBuf,
    pub lang: String,
    pub score: f64,
    pub reasons: Vec<&'static str>,
}

impl Subtitle {
    pub fn label(&self) -> String {
        format!("{} {}", self.lang.to_uppercase(), self.reasons.join(" "))
    }
}

pub struct Request {
    pub kind: Kind,
    pub imdb_id: String,
    pub episode: Option<(u32, u32)>,
}

struct Ranked {
    candidate: Candidate,
    score: f64,
    reasons: Vec<&'static str>,
}

async fn find_online(query: &Query) -> Vec<Candidate> {
    let results = join_all(providers::ALL.iter().map(|p| p.find(query))).await;
    results.into_iter().flatten().flatten().collect()
}

async fn search_online(request: &Request, playback: &Playback) -> (Release, Vec<Candidate>) {
    let size = playback.file.length;
    let query = Query {
        imdb_id: request.imdb_id.clone(),
        episode: request.episode,
        release: describe_release(&playback.file.path),
        filename: playback.file.name().to_string(),
        video_hash: video::opensubtitles_hash(&playback.url(), size).await.ok(),
        video_size: size,
    };
    let candidates = find_online(&query).await;
    (query.release, candidates)
}

fn rank(candidates: Vec<Candidate>, ctx: &Context) -> Vec<Ranked> {
    let mut scored: Vec<Ranked> = candidates
        .into_iter()
        .filter_map(|candidate| {
            let scored = rank::score(&candidate, ctx)?;
            Some(Ranked { candidate, score: scored.score, reasons: scored.reasons })
        })
        .collect();
    scored.sort_by(|a, b| b.score.total_cmp(&a.score));

    let mut seen = HashSet::new();
    scored.into_iter().filter(|sub| seen.insert(sub.candidate.id.clone())).collect()
}

fn worth_downloading(ranked: Vec<Ranked>) -> Vec<Ranked> {
    let mut taken: HashMap<String, usize> = HashMap::new();
    ranked
        .into_iter()
        .filter(|sub| {
            let limit = if PREFERRED_LANGUAGES.contains(&sub.candidate.lang.as_str()) { MAX_PER_LANG } else { 1 };
            let count = taken.entry(sub.candidate.lang.clone()).or_default();
            *count += 1;
            *count <= limit
        })
        .collect()
}

fn folder(playback: &Playback) -> PathBuf {
    cache_dir().join("subtitles").join(&playback.id)
}

fn save(dir: &PathBuf, name: &str, srt: &str) -> Option<PathBuf> {
    std::fs::create_dir_all(dir).ok()?;
    let safe: String = name.chars().map(|c| if c.is_ascii_alphanumeric() || c == '-' { c } else { '_' }).collect();
    let path = dir.join(format!("{safe}.srt"));
    std::fs::write(&path, srt).ok()?;
    Some(path)
}

async fn fetch(sub: Ranked, dir: &PathBuf) -> Option<Subtitle> {
    let bytes = providers::download(&sub.candidate.download).await.ok()?;
    let srt = decode::to_srt(bytes, &sub.candidate.hints).ok()?;
    Some(Subtitle {
        path: save(dir, &sub.candidate.id, &srt)?,
        lang: sub.candidate.lang,
        score: sub.score,
        reasons: sub.reasons,
    })
}

fn belongs_to_episode(path: &str, playback: &Playback, episode: Option<(u32, u32)>) -> bool {
    let videos = playback.files.iter().filter(|f| is_video(&f.path)).count();
    match episode {
        Some((season, number)) if videos > 1 => matches_episode(path, season, number),
        _ => true,
    }
}

async fn embedded(playback: &Playback, episode: Option<(u32, u32)>) -> Vec<(Subtitle, Option<f64>)> {
    let dir = folder(playback);
    let reads = playback
        .subtitle_files()
        .into_iter()
        .filter(|&idx| belongs_to_episode(&playback.files[idx].path, playback, episode))
        .map(|idx| {
            let (url, dir) = (playback.url_for(idx), dir.clone());
            let path = playback.files[idx].path.clone();
            async move {
                let lang = code_from_filename(&path);
                let bytes = video::read(&url).await.ok()?;
                let hints = Hints { lang: lang.map(String::from), ..Default::default() };
                let srt = decode::to_srt(bytes, &hints).ok()?;
                let subtitle = Subtitle {
                    path: save(&dir, &format!("torrent-{idx}"), &srt)?,
                    lang: lang.unwrap_or("und").to_string(),
                    score: EMBEDDED_SCORE,
                    reasons: vec!["in-torrent"],
                };
                Some((subtitle, decode::last_cue_end(&srt)))
            }
        });
    join_all(reads).await.into_iter().flatten().collect()
}

fn default_first(mut subtitles: Vec<Subtitle>) -> Vec<Subtitle> {
    subtitles.sort_by(|a, b| b.score.total_cmp(&a.score));
    let preferred = PREFERRED_LANGUAGES.iter().find_map(|lang| subtitles.iter().position(|s| s.lang == *lang));
    if let Some(index) = preferred {
        let best = subtitles.remove(index);
        subtitles.insert(0, best);
    }
    subtitles
}

pub async fn prepare(request: Request, playback: &Playback, measured: oneshot::Receiver<Measured>) -> Vec<Subtitle> {
    let (measured, meta, embedded, (release, candidates)) = tokio::join!(
        async { measured.await.ok() },
        catalog::meta(request.kind, &request.imdb_id),
        embedded(playback, request.episode),
        search_online(&request, playback),
    );
    let runtime = meta.ok().and_then(|m| m.details.runtime_seconds());
    let embedded_cues: Vec<f64> = embedded.iter().filter_map(|(_, cue)| *cue).collect();
    let ctx = rank::build_context(release, measured, runtime, &embedded_cues, &candidates);

    let dir = folder(playback);
    let chosen = worth_downloading(rank(candidates, &ctx));
    let downloaded = join_all(chosen.into_iter().map(|sub| fetch(sub, &dir))).await;

    let all = embedded.into_iter().map(|(s, _)| s).chain(downloaded.into_iter().flatten()).collect();
    default_first(all)
}

pub fn forget(playback: &Playback) {
    let _ = std::fs::remove_dir_all(folder(playback));
}
