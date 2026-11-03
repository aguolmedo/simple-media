use std::sync::LazyLock;

use regex::Regex;
use unicode_normalization::{UnicodeNormalization, char::is_combining_mark};

pub const QUALITIES: [&str; 7] = ["2160p", "1440p", "1080p", "720p", "576p", "480p", "360p"];

fn re(pattern: &str) -> Regex {
    Regex::new(pattern).expect("valid regex")
}

static VIDEO_EXT: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\.(mkv|mp4|avi|m4v|mov|wmv|ts|m2ts|webm|mpg|mpeg)$"));
static FILE_EXT: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\.(mkv|mp4|avi|m4v|mov|wmv|ts|m2ts|webm|mpg|mpeg|srt|sub|vtt|ass|ssa)$"));
static NOT_A_VIDEO: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\.(exe|msi|scr|bat|cmd|lnk|apk|dmg|zip|rar|7z)$"));
static SAMPLE: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\bsample\b"));
static UHD: LazyLock<Regex> = LazyLock::new(|| re(r"\b(4k|uhd)\b"));
static CAM: LazyLock<Regex> = LazyLock::new(|| re(r"\b(cam|hdcam|ts|telesync)\b"));
static ANY_EPISODE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)s\d{1,2}[ ._-]?e\d{1,3}|\b\d{1,2}x\d{2,3}\b|cap[ ._-]*\d{3,4}"));
static SEASON_RANGE: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)(?:\bs|seasons?|temporadas?)[ ._]?0*(\d{1,2})[ ._]*(?:-|to)[ ._]*(?:s|seasons?|temporadas?)?[ ._]?0*(\d{1,2})(?:\D|$)")
});
static SEASON_MARK: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(?:\bs|\bseason[ ._]?|\btemporada[ ._]?)0*(\d{1,2})(?:\D|$)"));
static ANY_PART: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)\b\d{1,3}[ ._]?of[ ._]?\d{1,3}\b|\b(?:ep|episode|part|pt|e)[ ._-]?\d{1,3}\b|[ _]-[ _]\d{1,3}(?:v\d)?(?:[ _.\[(]|$)"));
static EPISODE_TAG: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)s0*(\d{1,3})[ ._-]?e0*(\d{1,4})(?:\D|$)|\b0*(\d{1,3})x0*(\d{1,4})(?:\D|$)|cap[ ._-]*(\d{3,5})(?:\D|$)")
});
static PART_NUMBER: LazyLock<Regex> = LazyLock::new(|| {
    re(r"(?i)\b0*(\d{1,4})[ ._]?of[ ._]?\d{1,3}\b|\b(?:ep|episode|part|pt|e)[ ._-]?0*(\d{1,4})\b|[ _]-[ _]0*(\d{1,3})(?:v\d)?(?:[ _.\[(]|$)")
});
static SEASON_SINGLE: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)(?:\bs|season[ ._]?|temporada[ ._]?)0*(\d{1,3})(?:\D|$)"));
static COMPLETE_SERIES: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)complete[ ._]series|serie[ ._]completa"));
static SEPARATORS: LazyLock<Regex> = LazyLock::new(|| re(r"[._\-\[\]()]"));
static TAGS_START: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b(?:19|20)\d{2}\b|\bs\d{1,2}(?:e\d|\b)|\bseason\b|\btemporada\b|\b\d{3,4}p\b")
});
static SUBBED: LazyLock<Regex> = LazyLock::new(|| {
    re(r"\b(vose|vostfr|vost|subtitulad[oa]|legendado|subbed|[a-z]{2,3}[ ._-]?subs?)\b")
});
static MULTI: LazyLock<Regex> = LazyLock::new(|| re(r"\b(multi|dual)\b"));
static YIFY: LazyLock<Regex> = LazyLock::new(|| re(r"(?i)\b(yify|yts)\b"));
static GROUP: LazyLock<Regex> =
    LazyLock::new(|| re(r"(?i)-([a-z0-9]+)(?:[ .]?\[[^\]]*\]|\.[a-z]{2,3})?$"));

static SOURCES: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    vec![
        ("bluray", re(r"(?i)\b(blu[ ._-]?ray|bdrip|brrip|bdremux|remux|bd25|bd50)\b")),
        ("web", re(r"(?i)\b(web[ ._-]?dl|webrip|web|amzn|nf|dsnp|hmax|atvp)\b")),
        ("hdtv", re(r"(?i)\b(hdtv|pdtv|dsr|tvrip)\b")),
        ("dvd", re(r"(?i)\b(dvdrip|dvd[ ._-]?r|dvd|dvdscr)\b")),
        ("cam", re(r"(?i)\b(cam|hdcam|ts|telesync|hdts)\b")),
    ]
});

static LANGUAGES: LazyLock<Vec<(&str, Regex)>> = LazyLock::new(|| {
    vec![
        ("ENG", re(r"\b(english|eng)\b")),
        ("SPA", re(r"\b(castellano|español|espanol|spanish|esp|spa|vose|subtitulad[oa])\b")),
        ("LAT", re(r"\b(latino?|latam)\b")),
        ("FRE", re(r"\b(french|truefrench|vff|vfq|vf2?|fre|fra|vostfr)\b")),
        ("GER", re(r"\b(german|deutsch|ger)\b")),
        ("ITA", re(r"\b(italian|ita)\b")),
        ("POR", re(r"\b(portuguese|por|pt)\b")),
        ("PT-BR", re(r"\b(pt[ ._-]?br|dublado|brazilian|legendado)\b")),
        ("RUS", re(r"\b(russian|rus)\b")),
        ("JPN", re(r"\b(japanese|jap|jpn)\b")),
        ("KOR", re(r"\b(korean|kor)\b")),
        ("CHI", re(r"\b(chinese|chi|mandarin|cantonese)\b")),
        ("HIN", re(r"\b(hindi|hin|tamil|telugu)\b")),
        ("POL", re(r"\b(polish|pol|pl)\b")),
        ("TUR", re(r"\b(turkish|tur)\b")),
        ("DUT", re(r"\b(dutch|nl)\b")),
    ]
});

pub fn season_tag(season: u32) -> String {
    format!("S{season:02}")
}

pub fn episode_tag(season: u32, episode: u32) -> String {
    format!("S{season:02}E{episode:02}")
}

pub fn parse_quality(name: &str) -> String {
    let lower = name.to_lowercase();
    if UHD.is_match(&lower) {
        return "2160p".into();
    }
    if let Some(quality) = QUALITIES.iter().find(|q| lower.contains(*q)) {
        return quality.to_string();
    }
    if CAM.is_match(&lower) { "CAM" } else { "SD" }.into()
}

pub fn quality_rank(quality: &str) -> usize {
    match QUALITIES.iter().position(|q| *q == quality) {
        Some(i) => i,
        None => QUALITIES.len() + usize::from(quality == "CAM"),
    }
}

pub fn format_size(bytes: u64) -> String {
    if bytes == 0 {
        return "?".into();
    }
    let units = ["B", "KB", "MB", "GB", "TB"];
    let i = ((bytes as f64).log(1024.0).floor() as usize).min(units.len() - 1);
    let value = bytes as f64 / 1024f64.powi(i as i32);
    let decimals = if i >= 3 { 2 } else { 0 };
    format!("{value:.decimals$} {}", units[i])
}

fn number(captures: &regex::Captures, group: usize) -> Option<u32> {
    captures.get(group)?.as_str().parse().ok()
}

fn is_tagged(captures: &regex::Captures, season: u32, episode: u32) -> bool {
    let wanted = Some((season, episode));
    let numbered = |s, e| number(captures, s).zip(number(captures, e)) == wanted;
    let cap = captures.get(5).is_some_and(|c| c.as_str() == format!("{season}{episode:02}"));
    numbered(1, 2) || numbered(3, 4) || cap
}

pub fn matches_episode(name: &str, season: u32, episode: u32) -> bool {
    EPISODE_TAG.captures_iter(name).any(|c| is_tagged(&c, season, episode)) || matches_part(name, season, episode)
}

fn only_first_season(name: &str, season: u32) -> bool {
    season == 1 && !ANY_EPISODE.is_match(name) && SEASON_MARK.captures_iter(name).all(|c| &c[1] == "1")
}

fn matches_part(name: &str, season: u32, episode: u32) -> bool {
    let is_episode = |c: regex::Captures| (1..=3).find_map(|group| number(&c, group)) == Some(episode);
    only_first_season(name, season) && PART_NUMBER.captures_iter(name).any(is_episode)
}

pub fn is_whole_show(name: &str, season: u32) -> bool {
    only_first_season(name, season) && !ANY_PART.is_match(name)
}

pub fn is_season_pack(name: &str, season: u32) -> bool {
    if ANY_EPISODE.is_match(name) {
        return false;
    }
    if let Some(range) = SEASON_RANGE.captures(name) {
        let from: u32 = range[1].parse().unwrap_or(0);
        let to: u32 = range[2].parse().unwrap_or(0);
        return (from..=to).contains(&season);
    }
    let single = SEASON_SINGLE.captures_iter(name).any(|c| number(&c, 1) == Some(season));
    single || COMPLETE_SERIES.is_match(name)
}

pub fn is_not_a_video(name: &str) -> bool {
    NOT_A_VIDEO.is_match(name.trim())
}

pub fn is_video(path: &str) -> bool {
    VIDEO_EXT.is_match(path) && !SAMPLE.is_match(path)
}

pub struct Languages {
    pub audio: Vec<&'static str>,
    pub subs: Vec<&'static str>,
    pub multi: bool,
}

pub fn detect_languages(name: &str) -> Languages {
    let clean = SEPARATORS.replace_all(&name.to_lowercase(), " ").into_owned();
    let tags = match TAGS_START.find(&clean) {
        Some(m) if m.start() > 0 => &clean[m.start()..],
        _ => clean.as_str(),
    };
    let subbed = SUBBED.is_match(tags);
    let codes: Vec<&'static str> = LANGUAGES
        .iter()
        .filter(|(_, pattern)| pattern.is_match(tags))
        .map(|(code, _)| *code)
        .collect();
    Languages {
        audio: if subbed { vec![] } else { codes.clone() },
        subs: if subbed { codes } else { vec![] },
        multi: MULTI.is_match(tags),
    }
}

pub fn normalize(text: &str) -> String {
    let folded: String = text
        .nfd()
        .filter(|c| !is_combining_mark(*c))
        .collect::<String>()
        .to_lowercase();
    folded
        .split(|c: char| !c.is_ascii_alphanumeric())
        .filter(|word| !word.is_empty())
        .collect::<Vec<_>>()
        .join(" ")
}

fn spelling(text: &str) -> String {
    normalize(text).replace("isation", "ization")
}

pub fn title_matches(name: &str, title: &str) -> bool {
    format!(" {} ", spelling(name)).contains(&format!(" {} ", spelling(title)))
}

pub fn source_of(text: &str) -> Option<&'static str> {
    SOURCES.iter().find(|(_, pattern)| pattern.is_match(text)).map(|(source, _)| *source)
}

pub struct Release {
    pub quality: String,
    pub source: Option<&'static str>,
    pub group: Option<String>,
    pub is_yify: bool,
}

pub fn describe_release(name: &str) -> Release {
    let file = name.rsplit(['/', '\\']).next().unwrap_or(name);
    let base = FILE_EXT.replace(file, "");
    let is_yify = YIFY.is_match(&base);
    let group = GROUP.captures(&base).map(|c| c[1].to_lowercase());
    Release {
        quality: parse_quality(&base),
        source: source_of(&base),
        group: group.or_else(|| is_yify.then(|| "yify".to_string())),
        is_yify,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn episodes_and_packs() {
        assert!(matches_episode("Breaking.Bad.S02E03.720p", 2, 3));
        assert!(!matches_episode("Breaking.Bad.S02E31.720p", 2, 3));
        assert!(matches_episode("breaking bad 2x03", 2, 3));
        assert!(is_season_pack("Breaking Bad Season 2 1080p", 2));
        assert!(is_season_pack("Breaking.Bad.S01-S05.Complete", 3));
        assert!(!is_season_pack("Breaking Bad S02E03", 2));
        assert!(!is_season_pack("Breaking Bad Season 12", 1));
        assert!(matches_episode("Show.S01E10.1080p", 1, 10));
        assert!(!matches_episode("Show.S01E10.1080p", 1, 1));
        assert!(!matches_episode("Show.S11E01.1080p", 1, 1));
        assert!(matches_episode("Serie [Cap.203]", 2, 3));
        assert!(matches_episode("Serie Cap.1012", 10, 12));
        assert!(!matches_episode("Serie Cap.1012", 1, 12));
        assert!(matches_episode("show.2x3.avi", 2, 3));
        assert!(is_season_pack("Show S03 1080p", 3));
        assert!(!is_season_pack("Show S13 1080p", 3));
        assert!(is_not_a_video("The Odyssey 2160pHD (2026) ENGSubs EZTV.exe"));
        assert!(is_not_a_video("Movie 1080p.RAR "));
        assert!(!is_not_a_video("The Odyssey (2026) [1080p] [WEBRip] [5.1] [YTS.GG - YTS.BZ]"));
        assert!(!is_not_a_video("Movie.2026.1080p.WEB-DL.mkv"));
    }

    #[test]
    fn documentary_episodes() {
        assert!(matches_episode("BBC.Seven.Worlds.One.Planet.3of7.Asia.1080p", 1, 3));
        assert!(!matches_episode("BBC.Seven.Worlds.One.Planet.3of7.Asia.1080p", 1, 4));
        assert!(!matches_episode("BBC.Seven.Worlds.One.Planet.3of7.Asia.1080p", 2, 3));
        assert!(matches_episode("PBS The Civil War by Ken Burns 01of09", 1, 1));
        assert!(matches_episode("The Ascent of Man - Ep 03 - The Grain in the Stone.avi", 1, 3));
        assert!(matches_episode("Planet Earth II Part 2 Mountains", 1, 2));
        assert!(matches_episode("Cosmos/E05.mkv", 1, 5));
        assert!(!matches_episode("Show Season 2 Ep 03", 1, 3));
        assert!(!matches_episode("Show S02E03 Part 1", 1, 1));
        assert!(!matches_episode("Show 1of10 1080p", 1, 10));
        assert!(is_whole_show("BBC Seven Worlds One Planet 1080p x265", 1));
        assert!(is_whole_show("Prehistoric Planet Season 1 2160p", 1));
        assert!(!is_whole_show("Prehistoric Planet Season 2 2160p", 1));
        assert!(!is_whole_show("BBC Seven Worlds One Planet 3of7 1080p", 1));
        assert!(!is_whole_show("Breaking Bad 1080p", 2));
        assert!(title_matches("BBC.Civilisations.2018.1080p", "Civilizations"));
        assert!(matches_episode("[0000]_Serial_Experiments_Lain_-_01v2_[E6F3CE81].mkv", 1, 1));
        assert!(matches_episode("[Group] Show - 12 [1080p].mkv", 1, 12));
        assert!(!matches_episode("[Group] Show - 12 [1080p].mkv", 1, 1));
        assert!(!matches_episode("Show - 1080p WEB", 1, 108));
        assert!(!is_whole_show("[Group] Show - 03 [720p]", 1));
    }

    #[test]
    fn languages_and_titles() {
        let langs = detect_languages("Dune.2021.1080p.Castellano.English.x264");
        assert_eq!(langs.audio, vec!["ENG", "SPA"]);
        assert!(title_matches("Amélie.2001.1080p", "Amelie"));
        assert!(title_matches("Breaking.Bad.S02E03.720p", "Breaking Bad"));
        assert!(!title_matches("The.Bad.Guys.Breaking.In.S02E03", "Breaking Bad"));
        assert!(!title_matches("Breaking.Badly.S02E03", "Breaking Bad"));
        assert_eq!(format_size(1_500_000_000), "1.40 GB");
        assert_eq!(describe_release("Show.S01E01.1080p.WEB-DL-NTb.mkv").group.as_deref(), Some("ntb"));
    }
}
