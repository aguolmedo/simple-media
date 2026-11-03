use anyhow::Result;
use serde::Deserialize;
use serde_json::json;

use super::{ARCHIVE, Target, Torrent};
use crate::catalog::Kind;
use crate::http;
use crate::release::{episode_tag, is_season_pack, is_whole_show, matches_episode, season_tag, title_matches};

#[derive(Clone, Copy, Debug)]
pub enum Provider {
    Yts,
    Eztv,
    Tpb,
    Knaben,
    TorrentsCsv,
    Archive,
}

pub const ALL: [Provider; 6] =
    [Provider::Yts, Provider::Eztv, Provider::Tpb, Provider::Knaben, Provider::TorrentsCsv, Provider::Archive];

impl Provider {
    pub fn name(self) -> &'static str {
        match self {
            Provider::Yts => "YTS",
            Provider::Eztv => "EZTV",
            Provider::Tpb => "TPB",
            Provider::Knaben => "Knaben",
            Provider::TorrentsCsv => "torrents-csv",
            Provider::Archive => ARCHIVE,
        }
    }

    pub async fn find(self, target: &Target) -> Result<Vec<Torrent>> {
        match (self, target.episode) {
            (Provider::Yts, None) => yts(&target.imdb_id).await,
            (Provider::Yts, Some(_)) => Ok(vec![]),
            (Provider::Eztv, Some((season, episode))) => eztv(&target.imdb_id, season, episode).await,
            (Provider::Eztv, None) => Ok(vec![]),
            (Provider::Tpb, None) => tpb(&target.imdb_id).await,
            (Provider::Archive, _) => archive(target).await,
            (_, None) => self.text_movie(target).await,
            (_, Some((season, episode))) => self.text_episode(target, season, episode).await,
        }
    }

    async fn search_text(self, query: &str, kind: Kind) -> Result<Vec<Torrent>> {
        match self {
            Provider::Tpb => tpb(query).await,
            Provider::Knaben => knaben(query, kind).await,
            _ => torrents_csv(query).await,
        }
    }

    async fn text_movie(self, target: &Target) -> Result<Vec<Torrent>> {
        let query = match target.year {
            Some(year) => format!("{} {year}", target.name),
            None => target.name.clone(),
        };
        let found = self.search_text(&query, Kind::Movie).await?;
        Ok(found.into_iter().filter(|t| title_matches(&t.name, &target.name)).collect())
    }

    async fn text_episode(self, target: &Target, season: u32, episode: u32) -> Result<Vec<Torrent>> {
        let name = &target.name;
        let queries = [
            format!("{name} {}", episode_tag(season, episode)),
            format!("{name} {}", season_tag(season)),
            format!("{name} Season {season}"),
            name.clone(),
        ];
        let searches = queries.iter().map(|query| async move {
            let found = self.search_text(query, Kind::Series).await?;
            Ok::<_, anyhow::Error>(
                found
                    .into_iter()
                    .filter(|t| title_matches(&t.name, name))
                    .filter_map(|t| Some(Torrent { pack: is_pack(&t.name, season, episode)?, ..t }))
                    .collect::<Vec<_>>(),
            )
        });
        let results = futures::future::join_all(searches).await;
        if results.iter().all(Result::is_err) {
            return results.into_iter().next().unwrap_or(Ok(vec![]));
        }
        Ok(results.into_iter().flatten().flatten().collect())
    }
}

fn is_pack(name: &str, season: u32, episode: u32) -> Option<bool> {
    if matches_episode(name, season, episode) {
        return Some(false);
    }
    (is_season_pack(name, season) || is_whole_show(name, season)).then_some(true)
}

fn torrent(info_hash: &str, name: String, seeds: u64, leechers: Option<u64>, size: u64, source: &str) -> Torrent {
    Torrent {
        info_hash: info_hash.to_lowercase(),
        name,
        seeds,
        leechers,
        size,
        source: source.to_string(),
        quality: None,
        pack: false,
        archive: None,
        file_idx: None,
        file_name: None,
        local: None,
    }
}

#[derive(Deserialize)]
struct YtsResponse {
    data: YtsData,
}

#[derive(Deserialize)]
struct YtsData {
    #[serde(default)]
    movies: Option<Vec<YtsMovie>>,
}

#[derive(Deserialize)]
struct YtsMovie {
    imdb_code: String,
    title_long: String,
    torrents: Vec<YtsTorrent>,
}

#[derive(Deserialize)]
struct YtsTorrent {
    hash: String,
    quality: String,
    #[serde(rename = "type")]
    kind: String,
    #[serde(default)]
    video_codec: String,
    seeds: u64,
    peers: u64,
    size_bytes: u64,
}

async fn yts(imdb_id: &str) -> Result<Vec<Torrent>> {
    let mirrors = ["https://movies-api.accel.li", "https://yts.gg", "https://yts.mx"];
    let data: YtsResponse = http::first_ok(&mirrors, |base| async move {
        http::get_json(&format!("{base}/api/v2/list_movies.json?query_term={imdb_id}")).await
    })
    .await?;
    let movie = data.data.movies.unwrap_or_default().into_iter().find(|m| m.imdb_code == imdb_id);
    let Some(movie) = movie else { return Ok(vec![]) };
    Ok(movie
        .torrents
        .into_iter()
        .map(|t| {
            let name = format!("{} {} {} {}", movie.title_long, t.quality, t.kind, t.video_codec);
            Torrent { quality: Some(t.quality.clone()), ..torrent(&t.hash, name, t.seeds, Some(t.peers), t.size_bytes, "YTS") }
        })
        .collect())
}

#[derive(Deserialize)]
struct EztvPage {
    #[serde(default)]
    torrents: Vec<EztvTorrent>,
    #[serde(default)]
    torrents_count: usize,
}

#[derive(Deserialize)]
struct EztvTorrent {
    hash: String,
    title: String,
    season: String,
    episode: String,
    seeds: u64,
    peers: u64,
    size_bytes: String,
}

async fn eztv(imdb_id: &str, season: u32, episode: u32) -> Result<Vec<Torrent>> {
    const PAGE_SIZE: usize = 100;
    const MAX_PAGES: usize = 10;
    let mirrors = ["https://eztvx.to", "https://eztv.re", "https://eztv.wf"];
    let id = imdb_id.trim_start_matches("tt");
    let mut all = Vec::new();
    for page in 1..=MAX_PAGES {
        let data: EztvPage = http::first_ok(&mirrors, |base| async move {
            http::get_json(&format!("{base}/api/get-torrents?imdb_id={id}&limit={PAGE_SIZE}&page={page}")).await
        })
        .await?;
        let count = data.torrents.len();
        all.extend(data.torrents);
        if all.len() >= data.torrents_count || count < PAGE_SIZE {
            break;
        }
    }
    let wanted = |t: &EztvTorrent| t.season.parse() == Ok(season) && t.episode.parse() == Ok(episode);
    Ok(all
        .into_iter()
        .filter(wanted)
        .map(|t| torrent(&t.hash, t.title, t.seeds, Some(t.peers), t.size_bytes.parse().unwrap_or(0), "EZTV"))
        .collect())
}

#[derive(Deserialize)]
struct TpbTorrent {
    id: String,
    name: String,
    info_hash: String,
    seeders: String,
    leechers: String,
    size: String,
    category: String,
}

async fn tpb(query: &str) -> Result<Vec<Torrent>> {
    let url = format!("https://apibay.org/q.php?q={}", urlencoding::encode(query));
    let results: Vec<TpbTorrent> = http::get_json(&url).await?;
    Ok(results
        .into_iter()
        .filter(|t| t.id != "0" && t.category.starts_with('2'))
        .map(|t| {
            let seeds = t.seeders.parse().unwrap_or(0);
            torrent(&t.info_hash, t.name, seeds, t.leechers.parse().ok(), t.size.parse().unwrap_or(0), "TPB")
        })
        .filter(|t| t.seeds > 0)
        .collect())
}

#[derive(Deserialize)]
struct KnabenResponse {
    hits: Vec<KnabenHit>,
}

#[derive(Deserialize)]
struct KnabenHit {
    #[serde(default)]
    hash: Option<String>,
    title: String,
    #[serde(default)]
    seeders: u64,
    #[serde(default)]
    peers: Option<u64>,
    #[serde(default)]
    bytes: u64,
    #[serde(default)]
    tracker: String,
}

async fn knaben(query: &str, kind: Kind) -> Result<Vec<Torrent>> {
    let category = match kind {
        Kind::Movie => 3000000,
        Kind::Series => 2000000,
    };
    let body = json!({
        "search_type": "100%",
        "search_field": "title",
        "query": query,
        "categories": [category],
        "order_by": "seeders",
        "order_direction": "desc",
        "size": 50,
        "hide_unsafe": true,
        "hide_xxx": true,
    });
    let data: KnabenResponse = http::json(http::post("https://api.knaben.org/v1").json(&body)).await?;
    Ok(data
        .hits
        .into_iter()
        .filter(|hit| hit.seeders > 0)
        .filter_map(|hit| {
            let source = format!("Knaben/{}", hit.tracker);
            Some(torrent(&hit.hash?, hit.title, hit.seeders, hit.peers, hit.bytes, &source))
        })
        .collect())
}

#[derive(Deserialize)]
struct CsvResponse {
    #[serde(default)]
    torrents: Vec<CsvTorrent>,
}

#[derive(Deserialize)]
struct CsvTorrent {
    infohash: String,
    name: String,
    seeders: u64,
    leechers: u64,
    size_bytes: u64,
}

async fn torrents_csv(query: &str) -> Result<Vec<Torrent>> {
    let url = format!("https://torrents-csv.com/service/search?q={}&size=50", urlencoding::encode(query));
    let data: CsvResponse = http::get_json(&url).await?;
    Ok(data
        .torrents
        .into_iter()
        .filter(|t| t.seeders > 0)
        .map(|t| torrent(&t.infohash, t.name, t.seeders, Some(t.leechers), t.size_bytes, "torrents-csv"))
        .collect())
}

#[derive(Deserialize)]
struct ArchiveResponse {
    response: ArchiveDocs,
}

#[derive(Deserialize)]
struct ArchiveDocs {
    docs: Vec<ArchiveItem>,
}

#[derive(Deserialize)]
struct ArchiveItem {
    identifier: String,
    title: String,
    #[serde(default)]
    item_size: u64,
}

async fn archive(target: &Target) -> Result<Vec<Torrent>> {
    let query = format!("title:(\"{}\") AND mediatype:movies", target.name.replace('"', ""));
    let url = format!(
        "https://archive.org/advancedsearch.php?q={}&fl[]=identifier&fl[]=title&fl[]=item_size&sort[]=downloads+desc&rows=50&output=json",
        urlencoding::encode(&query)
    );
    let data: ArchiveResponse = http::get_json(&url).await?;
    Ok(data
        .response
        .docs
        .into_iter()
        .filter(|item| title_matches(&item.title, &target.name))
        .filter_map(|item| {
            let pack = match target.episode {
                Some((season, episode)) => is_pack(&item.title, season, episode)?,
                None => false,
            };
            let base = torrent(&item.identifier, item.title, 0, None, item.item_size, ARCHIVE);
            Some(Torrent { pack, archive: Some(item.identifier), ..base })
        })
        .collect())
}
