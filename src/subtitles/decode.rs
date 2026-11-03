use std::io::{Cursor, Read};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use encoding_rs::{Encoding, UTF_16BE, UTF_16LE, WINDOWS_1252};
use flate2::read::GzDecoder;
use regex::Regex;

use super::languages::codepage_for;

static ADS: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)opensubtitles|osdb\.link|anuncie su producto|advertise your product").expect("valid regex")
});
static MICRODVD: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^\{(\d+)\}\{(\d*)\}(.*)$").expect("valid regex"));
static MICRODVD_TAGS: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\{[^}]*\}").expect("valid regex"));
static CUE_END: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"-->\s*(\d+):(\d{2}):(\d{2})[,.](\d{1,3})").expect("valid regex"));

#[derive(Clone, Default)]
pub struct Hints {
    pub encoding: Option<String>,
    pub lang: Option<String>,
    pub format: Option<String>,
    pub fps: Option<f64>,
}

fn encoding_named(label: &str) -> Option<&'static Encoding> {
    let lower = label.to_lowercase();
    let label = match lower.strip_prefix("cp") {
        _ if lower == "ascii" => "windows-1252".to_string(),
        Some(number) if number.chars().all(|c| c.is_ascii_digit()) => format!("windows-{number}"),
        _ => lower,
    };
    Encoding::for_label(label.as_bytes())
}

pub fn to_text(bytes: &[u8], hints: &Hints) -> String {
    if let Some(rest) = bytes.strip_prefix(&[0xef, 0xbb, 0xbf]) {
        return String::from_utf8_lossy(rest).into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xff, 0xfe]) {
        return UTF_16LE.decode_without_bom_handling(rest).0.into_owned();
    }
    if let Some(rest) = bytes.strip_prefix(&[0xfe, 0xff]) {
        return UTF_16BE.decode_without_bom_handling(rest).0.into_owned();
    }
    if let Ok(text) = std::str::from_utf8(bytes) {
        return text.to_string();
    }
    let declared = hints.encoding.as_deref().and_then(encoding_named);
    let by_language = hints.lang.as_deref().map(codepage_for).and_then(encoding_named);
    let encoding = declared.or(by_language).unwrap_or(WINDOWS_1252);
    encoding.decode_without_bom_handling(bytes).0.into_owned()
}

fn unpack(bytes: Vec<u8>) -> Result<Vec<u8>> {
    if bytes.starts_with(&[0x1f, 0x8b]) {
        let mut out = Vec::new();
        GzDecoder::new(bytes.as_slice()).read_to_end(&mut out)?;
        return Ok(out);
    }
    if !bytes.starts_with(b"PK") {
        return Ok(bytes);
    }
    let mut archive = zip::ZipArchive::new(Cursor::new(bytes))?;
    let largest = (0..archive.len())
        .filter_map(|i| {
            let file = archive.by_index(i).ok()?;
            Some((i, file.name().ok()?.to_lowercase(), file.size()))
        })
        .filter(|(_, name, _)| name.ends_with(".srt"))
        .max_by_key(|(_, _, size)| *size)
        .map(|(i, _, _)| i)
        .context("the zip has no .srt")?;
    let mut out = Vec::new();
    archive.by_index(largest)?.read_to_end(&mut out)?;
    Ok(out)
}

fn srt_time(seconds: f64) -> String {
    let ms = (seconds * 1000.0).round() as u64;
    format!("{:02}:{:02}:{:02},{:03}", ms / 3_600_000, ms / 60_000 % 60, ms / 1000 % 60, ms % 1000)
}

fn microdvd_to_srt(text: &str, fps: Option<f64>) -> Result<String> {
    let mut lines: Vec<(f64, Option<f64>, String)> = text
        .lines()
        .filter_map(|line| MICRODVD.captures(line.trim_end()))
        .map(|c| (c[1].parse().unwrap_or(0.0), c[2].parse().ok(), c[3].to_string()))
        .collect();
    if lines.is_empty() {
        bail!("not a MicroDVD subtitle");
    }
    let mut fps = fps.unwrap_or(23.976);
    if let Some((1.0, Some(1.0), body)) = lines.first()
        && let Ok(declared) = body.parse::<f64>()
    {
        fps = declared;
        lines.remove(0);
    }
    let cues = lines.iter().enumerate().map(|(i, (start, end, body))| {
        let from = start / fps;
        let to = end.map(|e| e / fps).unwrap_or(from + 3.0);
        let content = MICRODVD_TAGS.replace_all(body, "").replace('|', "\n");
        format!("{}\n{} --> {}\n{content}", i + 1, srt_time(from), srt_time(to))
    });
    Ok(cues.collect::<Vec<_>>().join("\n\n"))
}

fn remove_ads(srt: &str) -> String {
    let normalized = srt.replace("\r\n", "\n").replace('\r', "\n");
    let blocks: Vec<&str> = normalized
        .trim()
        .split("\n\n")
        .map(|block| block.trim_matches('\n'))
        .filter(|block| !block.trim().is_empty() && !ADS.is_match(block))
        .collect();
    blocks.join("\n\n") + "\n"
}

pub fn to_srt(bytes: Vec<u8>, hints: &Hints) -> Result<String> {
    let mut text = to_text(&unpack(bytes)?, hints);
    if hints.format.as_deref() == Some("sub") {
        text = microdvd_to_srt(&text, hints.fps)?;
    }
    if !text.contains("-->") {
        bail!("the download is not a subtitle");
    }
    Ok(remove_ads(&text))
}

pub fn last_cue_end(srt: &str) -> Option<f64> {
    let c = CUE_END.captures_iter(srt).last()?;
    let number = |i: usize| c[i].parse::<f64>().unwrap_or(0.0);
    let millis: f64 = format!("{:0<3}", &c[4]).parse().unwrap_or(0.0);
    Some(number(1) * 3600.0 + number(2) * 60.0 + number(3) + millis / 1000.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn converts_and_cleans() {
        let hints = Hints { format: Some("sub".into()), fps: Some(25.0), ..Default::default() };
        let srt = to_srt(b"{1}{1}25\n{25}{50}Hola|mundo\n{100}{}Fin".to_vec(), &hints).unwrap();
        assert!(srt.contains("00:00:01,000 --> 00:00:02,000\nHola\nmundo"));
        assert_eq!(last_cue_end(&srt), Some(7.0));

        let ads = "1\n00:00:01,000 --> 00:00:02,000\nHola\n\n2\n00:00:03,000 --> 00:00:04,000\nwww.OpenSubtitles.org\n";
        assert!(!to_srt(ads.as_bytes().to_vec(), &Hints::default()).unwrap().contains("OpenSubtitles"));

        let latin1 = to_text(&[0x61, 0xf1, 0x6f], &Hints { lang: Some("spa".into()), ..Default::default() });
        assert_eq!(latin1, "año");
    }
}
