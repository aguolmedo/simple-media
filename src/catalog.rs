use std::collections::HashMap;
use std::sync::{LazyLock, Mutex};

use anyhow::Result;
use serde::{Deserialize, Serialize};

use crate::http;
use crate::release::normalize;

const CINEMETA: &str = "https://v3-cinemeta.strem.io";

static METAS: LazyLock<Mutex<HashMap<(Kind, String), Meta>>> = LazyLock::new(Default::default);

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Movie,
    Series,
}

impl Kind {
    fn path(self) -> &'static str {
        match self {
            Kind::Movie => "movie",
            Kind::Series => "series",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Kind::Movie => "Movie",
            Kind::Series => "Series",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Details {
    #[serde(default)]
    pub description: Option<String>,
    #[serde(rename = "imdbRating", default)]
    pub rating: Option<String>,
    #[serde(default)]
    pub genres: Vec<String>,
    #[serde(default)]
    pub cast: Vec<String>,
    #[serde(default)]
    pub runtime: Option<String>,
}

impl Details {
    pub fn is_empty(&self) -> bool {
        self.description.is_none() && self.rating.is_none()
    }

    pub fn rating(&self) -> Option<&str> {
        self.rating.as_deref().filter(|r| !r.is_empty())
    }

    pub fn runtime_seconds(&self) -> Option<f64> {
        let minutes: f64 = self.runtime.as_deref()?.split_whitespace().next()?.parse().ok()?;
        Some(minutes * 60.0)
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Title {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: Kind,
    pub name: String,
    #[serde(rename = "releaseInfo", default)]
    pub release_info: Option<String>,
    #[serde(flatten)]
    pub details: Details,
}

impl Title {
    pub fn year(&self) -> String {
        self.release_info.clone().unwrap_or_default()
    }

    pub fn first_year(&self) -> Option<u32> {
        let info = self.release_info.as_deref()?;
        info.get(..4)?.parse().ok()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Episode {
    pub season: u32,
    pub episode: u32,
    #[serde(default)]
    pub name: Option<String>,
    #[serde(default)]
    pub released: Option<String>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Meta {
    #[serde(flatten)]
    pub details: Details,
    #[serde(default)]
    pub videos: Vec<Episode>,
}

impl Meta {
    pub fn seasons(&self) -> Vec<u32> {
        let mut seasons: Vec<u32> = self.videos.iter().map(|v| v.season).collect();
        seasons.sort_by_key(|s| (*s == 0, *s));
        seasons.dedup();
        seasons
    }

    pub fn episodes(&self, season: u32) -> Vec<Episode> {
        let mut episodes: Vec<Episode> = self.videos.iter().filter(|v| v.season == season).cloned().collect();
        episodes.sort_by_key(|e| e.episode);
        episodes
    }

    pub fn next_episode(&self, season: u32, episode: u32) -> Option<Episode> {
        let mut all: Vec<&Episode> = self.videos.iter().filter(|v| v.season > 0).collect();
        all.sort_by_key(|e| (e.season, e.episode));
        all.into_iter().find(|e| (e.season, e.episode) > (season, episode)).cloned()
    }
}

#[derive(Deserialize)]
struct Catalog {
    #[serde(default)]
    metas: Vec<Title>,
}

#[derive(Deserialize)]
struct MetaResponse {
    meta: Meta,
}

async fn search_kind(kind: Kind, query: &str) -> Result<Vec<Title>> {
    let url = format!("{CINEMETA}/catalog/{}/top/search={}.json", kind.path(), urlencoding::encode(query));
    Ok(http::get_json::<Catalog>(&url).await?.metas)
}

pub async fn search(query: &str) -> Result<Vec<Title>> {
    let (movies, series) = tokio::join!(search_kind(Kind::Movie, query), search_kind(Kind::Series, query));
    if let (Err(err), Err(_)) = (&movies, &series) {
        anyhow::bail!("{err}");
    }
    let mut titles = interleave(series.unwrap_or_default(), movies.unwrap_or_default());
    let wanted = normalize(query);
    titles.sort_by_cached_key(|t| normalize(&t.name) != wanted);
    Ok(titles)
}

fn interleave(first: Vec<Title>, second: Vec<Title>) -> Vec<Title> {
    let mut merged = Vec::with_capacity(first.len() + second.len());
    let (mut a, mut b) = (first.into_iter(), second.into_iter());
    loop {
        match (a.next(), b.next()) {
            (None, None) => return merged,
            (x, y) => merged.extend(x.into_iter().chain(y)),
        }
    }
}

pub async fn meta(kind: Kind, id: &str) -> Result<Meta> {
    let key = (kind, id.to_string());
    if let Some(meta) = METAS.lock().unwrap().get(&key) {
        return Ok(meta.clone());
    }
    let url = format!("{CINEMETA}/meta/{}/{id}.json", kind.path());
    let meta = http::get_json::<MetaResponse>(&url).await?.meta;
    METAS.lock().unwrap().insert(key, meta.clone());
    Ok(meta)
}
