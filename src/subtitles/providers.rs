use std::sync::LazyLock;

use anyhow::Result;
use regex::Regex;
use serde::Deserialize;

use super::decode::Hints;
use super::languages::to_code;
use super::{Candidate, Download, Query};
use crate::http;

const OPENSUBTITLES: &str = "https://rest.opensubtitles.org/search";
const STREMIO_PROXY: &str = "https://subs5.strem.io/en/download/subencoding-stremio-utf8/src-api/file";
const OPENSUBTITLES_UA: &str = "TemporaryUserAgent";
const OPENSUBTITLES_V3: &str = "https://opensubtitles-v3.strem.io";
const YIFY: &str = "https://yifysubtitles.ch";

#[derive(Clone, Copy, Debug)]
pub enum Provider {
    OpenSubtitles,
    OpenSubtitlesV3,
    Yify,
}

pub const ALL: [Provider; 3] = [Provider::OpenSubtitles, Provider::OpenSubtitlesV3, Provider::Yify];

impl Provider {
    pub async fn find(self, query: &Query) -> Result<Vec<Candidate>> {
        match self {
            Provider::OpenSubtitles => opensubtitles(query).await,
            Provider::OpenSubtitlesV3 => opensubtitles_v3(query).await,
            Provider::Yify if query.episode.is_none() && query.release.is_yify => yify(&query.imdb_id).await,
            Provider::Yify => Ok(vec![]),
        }
    }
}

pub async fn download(download: &Download) -> Result<Vec<u8>> {
    match download {
        Download::OpenSubtitles { link, file_id } => {
            let urls = [link.clone(), format!("{STREMIO_PROXY}/{file_id}")];
            let urls: Vec<&str> = urls.iter().map(String::as_str).collect();
            http::first_ok(&urls, |url| async move {
                let request = http::get(&url).header("user-agent", OPENSUBTITLES_UA).header("x-user-agent", OPENSUBTITLES_UA);
                get_file(request).await
            })
            .await
        }
        Download::Direct { url } => get_file(http::get(url)).await,
        Download::Yify { slug } => {
            let request = http::get(&format!("{YIFY}/subtitle/{slug}.zip")).header("referer", format!("{YIFY}/subtitles/{slug}"));
            get_file(request).await
        }
    }
}

async fn get_file(request: reqwest::RequestBuilder) -> Result<Vec<u8>> {
    let res = request.send().await?.error_for_status()?;
    let is_html = res.headers().get("content-type").is_some_and(|t| t.to_str().unwrap_or("").contains("text/html"));
    if is_html {
        anyhow::bail!("got HTML instead of a subtitle");
    }
    Ok(res.bytes().await?.to_vec())
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct OsSubtitle {
    #[serde(rename = "IDSubtitleFile")]
    id_subtitle_file: String,
    #[serde(rename = "SubLanguageID")]
    sub_language_id: String,
    #[serde(default)]
    movie_release_name: String,
    #[serde(default)]
    sub_file_name: String,
    #[serde(default)]
    info_release_group: String,
    #[serde(default)]
    info_format: String,
    #[serde(rename = "MovieFPS", default)]
    movie_fps: String,
    #[serde(rename = "SubLastTS", default)]
    sub_last_ts: String,
    #[serde(default)]
    matched_by: String,
    #[serde(default)]
    sub_ts_group_hash: String,
    #[serde(default)]
    sub_downloads_cnt: String,
    #[serde(default)]
    sub_from_trusted: String,
    #[serde(default)]
    sub_hearing_impaired: String,
    #[serde(default)]
    sub_format: String,
    #[serde(rename = "SubSumCD", default)]
    sub_sum_cd: String,
    #[serde(default)]
    sub_bad: String,
    #[serde(default)]
    sub_auto_translation: String,
    #[serde(default)]
    sub_foreign_parts_only: String,
    #[serde(default)]
    sub_encoding: String,
    #[serde(default)]
    sub_download_link: String,
    #[serde(rename = "IDMovieImdb", default)]
    id_movie_imdb: String,
    #[serde(rename = "SeriesIMDBParent", default)]
    series_imdb_parent: String,
    #[serde(default)]
    series_season: String,
    #[serde(default)]
    series_episode: String,
}

fn hms_to_seconds(timestamp: &str) -> Option<f64> {
    let parts: Vec<f64> = timestamp.split(':').take(3).map(|p| p.parse().ok()).collect::<Option<_>>()?;
    let [h, m, s] = parts[..] else { return None };
    Some(h * 3600.0 + m * 60.0 + s)
}

impl OsSubtitle {
    fn is_unusable(&self) -> bool {
        let fps: f64 = self.movie_fps.parse().unwrap_or(0.0);
        let convertible = self.sub_format == "srt" || (self.sub_format == "sub" && fps > 0.0);
        !convertible
            || self.sub_sum_cd.parse::<u32>().unwrap_or(1) > 1
            || self.sub_bad == "1"
            || self.sub_auto_translation == "1"
            || self.sub_foreign_parts_only == "1"
    }

    fn belongs_to(&self, query: &Query) -> bool {
        let numeric_id = query.imdb_id.trim_start_matches("tt").trim_start_matches('0');
        let same_title = self.id_movie_imdb == numeric_id || self.series_imdb_parent == numeric_id;
        let same_episode = match query.episode {
            None => true,
            Some((season, episode)) => {
                self.series_season.parse() == Ok(season) && self.series_episode.parse() == Ok(episode)
            }
        };
        same_title && same_episode
    }

    fn into_candidate(self) -> Candidate {
        let fps = self.movie_fps.parse().ok().filter(|f| *f > 0.0);
        let rejected = self.is_unusable();
        let timing_group = Some(self.sub_ts_group_hash.clone()).filter(|g| !g.is_empty() && g != "0");
        Candidate {
            id: format!("os-{}", self.id_subtitle_file),
            lang: self.sub_language_id.clone(),
            release: if self.movie_release_name.is_empty() { self.sub_file_name.clone() } else { self.movie_release_name },
            file_name: self.sub_file_name,
            group: self.info_release_group.to_lowercase(),
            format: self.info_format.to_lowercase(),
            fps,
            last_cue: hms_to_seconds(&self.sub_last_ts),
            hash_match: self.matched_by == "moviehash",
            timing_group,
            downloads: self.sub_downloads_cnt.parse().unwrap_or(0),
            trusted: self.sub_from_trusted == "1",
            rating: 0.0,
            hearing_impaired: self.sub_hearing_impaired == "1",
            rejected,
            download: Download::OpenSubtitles { link: self.sub_download_link, file_id: self.id_subtitle_file },
            hints: Hints {
                encoding: Some(self.sub_encoding),
                lang: Some(self.sub_language_id),
                format: Some(self.sub_format),
                fps,
            },
        }
    }
}

fn opensubtitles_paths(query: &Query) -> Vec<String> {
    let id = format!("{:0>7}", query.imdb_id.trim_start_matches("tt"));
    let by_imdb = match query.episode {
        Some((season, episode)) => format!("episode-{episode}/imdbid-{id}/season-{season}"),
        None => format!("imdbid-{id}"),
    };
    let by_hash = query.video_hash.as_ref().map(|hash| format!("moviebytesize-{}/moviehash-{hash}", query.video_size));
    [Some(by_imdb), by_hash].into_iter().flatten().collect()
}

async fn opensubtitles(query: &Query) -> Result<Vec<Candidate>> {
    let searches = opensubtitles_paths(query).into_iter().map(|path| async move {
        let request = http::get(&format!("{OPENSUBTITLES}/{path}"))
            .header("user-agent", OPENSUBTITLES_UA)
            .header("x-user-agent", OPENSUBTITLES_UA);
        http::json::<Vec<OsSubtitle>>(request).await
    });
    let results = futures::future::join_all(searches).await;
    if results.iter().all(Result::is_err) {
        return results.into_iter().next().unwrap_or(Ok(vec![])).map(|_| vec![]);
    }
    Ok(results
        .into_iter()
        .flatten()
        .flatten()
        .filter(|s| s.belongs_to(query))
        .map(OsSubtitle::into_candidate)
        .collect())
}

#[derive(Deserialize)]
struct V3Response {
    #[serde(default)]
    subtitles: Vec<V3Subtitle>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct V3Subtitle {
    id: String,
    url: String,
    lang: String,
    #[serde(default)]
    m: String,
    #[serde(default)]
    g: Option<String>,
    #[serde(default)]
    subtitle_file_name: String,
    #[serde(default)]
    movie_release_name: String,
    #[serde(default)]
    release_group: String,
    #[serde(default)]
    release_format: String,
    #[serde(default)]
    fps_milli: Option<f64>,
}

static CD_PART: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)\bcd[ ._-]?\d\b").expect("valid regex"));
static FILE_ID: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/file/(\d+)").expect("valid regex"));

fn v3_path(query: &Query) -> String {
    let (kind, id) = match query.episode {
        Some((season, episode)) => ("series", format!("{}:{season}:{episode}", query.imdb_id)),
        None => ("movie", query.imdb_id.clone()),
    };
    let mut extra = vec![format!("filename={}", urlencoding::encode(&query.filename))];
    if let Some(hash) = &query.video_hash {
        extra.push(format!("videoHash={hash}"));
        extra.push(format!("videoSize={}", query.video_size));
    }
    format!("subtitles/{kind}/{id}/{}.json", extra.join("&"))
}

impl V3Subtitle {
    fn into_candidate(self) -> Candidate {
        let file_id = FILE_ID.captures(&self.url).map(|c| c[1].to_string()).unwrap_or(self.id);
        let fps = self.fps_milli.map(|f| f / 1000.0);
        Candidate {
            id: format!("os-{file_id}"),
            lang: self.lang.clone(),
            release: if self.movie_release_name.is_empty() { self.subtitle_file_name.clone() } else { self.movie_release_name },
            rejected: CD_PART.is_match(&self.subtitle_file_name),
            file_name: self.subtitle_file_name,
            group: self.release_group.to_lowercase(),
            format: self.release_format.to_lowercase(),
            fps,
            last_cue: None,
            hash_match: self.m == "h",
            timing_group: self.g.map(|g| format!("v3-{g}")),
            downloads: 0,
            trusted: false,
            rating: 0.0,
            hearing_impaired: false,
            download: Download::Direct { url: self.url },
            hints: Hints { lang: Some(self.lang), ..Default::default() },
        }
    }
}

async fn opensubtitles_v3(query: &Query) -> Result<Vec<Candidate>> {
    let url = format!("{OPENSUBTITLES_V3}/{}", v3_path(query));
    let response: V3Response = http::get_json(&url).await?;
    Ok(response.subtitles.into_iter().map(V3Subtitle::into_candidate).collect())
}

static YIFY_ROW: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<tr[^>]*data-id="\d+"[^>]*>(?s:.*?)</tr>"#).expect("valid regex"));
static YIFY_RATING: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"rating-cell(?s:.*?)>(-?\d+)<").expect("valid regex"));
static YIFY_LANG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"class="sub-lang">([^<]+)<"#).expect("valid regex"));
static YIFY_SLUG: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"href="/subtitles/([^"]+)""#).expect("valid regex"));
static YIFY_RELEASE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r#"<span class="text-muted">subtitle</span>\s*([^<]+)<"#).expect("valid regex"));

fn decode_html(text: &str) -> String {
    text.replace("&amp;", "&").replace("&quot;", "\"").replace("&#39;", "'").replace("&lt;", "<").replace("&gt;", ">")
}

fn yify_row(row: &str) -> Option<Candidate> {
    let capture = |re: &Regex| re.captures(row).map(|c| c[1].to_string());
    let rating: f64 = capture(&YIFY_RATING).and_then(|r| r.parse().ok()).unwrap_or(0.0);
    let lang = to_code(&capture(&YIFY_LANG)?)?.to_string();
    let slug = capture(&YIFY_SLUG)?;
    let release = decode_html(capture(&YIFY_RELEASE).unwrap_or_default().trim());
    Some(Candidate {
        id: format!("yify-{slug}"),
        lang: lang.clone(),
        release,
        file_name: String::new(),
        group: String::new(),
        format: String::new(),
        fps: None,
        last_cue: None,
        hash_match: false,
        timing_group: None,
        downloads: 0,
        trusted: false,
        rating,
        hearing_impaired: false,
        rejected: rating < 0.0,
        download: Download::Yify { slug },
        hints: Hints { lang: Some(lang), ..Default::default() },
    })
}

async fn yify(imdb_id: &str) -> Result<Vec<Candidate>> {
    let res = http::get(&format!("{YIFY}/movie-imdb/{imdb_id}")).send().await?;
    if res.status() == reqwest::StatusCode::NOT_FOUND {
        return Ok(vec![]);
    }
    let html = res.error_for_status()?.text().await?;
    Ok(YIFY_ROW.find_iter(&html).filter_map(|m| yify_row(m.as_str())).collect())
}
