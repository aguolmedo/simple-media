use std::collections::HashSet;

use super::Candidate;
use crate::release::{Release, describe_release, source_of};

const SAME_END_TOLERANCE_S: f64 = 3.0;
const CONSENSUS_TOLERANCE_S: f64 = 2.0;
const MAX_FPS_DIFF: f64 = 0.5;

pub struct Context {
    pub release: Release,
    pub duration: Option<f64>,
    pub duration_is_exact: bool,
    pub fps: Option<f64>,
    pub reference_last_cue: Option<f64>,
    pub consensus_last_cue: Option<f64>,
    pub synced_timing_groups: HashSet<String>,
}

pub struct Scored {
    pub score: f64,
    pub reasons: Vec<&'static str>,
}

fn median(numbers: impl Iterator<Item = f64>) -> Option<f64> {
    let mut sorted: Vec<f64> = numbers.filter(|n| *n > 0.0).collect();
    sorted.sort_by(f64::total_cmp);
    sorted.get(sorted.len() / 2).copied()
}

fn most_common_last_cue(subs: &[&Candidate]) -> Option<f64> {
    let times: Vec<f64> = subs.iter().filter_map(|s| s.last_cue).collect();
    let mut best = None;
    let mut best_count = 1;
    for &t in &times {
        let count = times.iter().filter(|u| (*u - t).abs() <= CONSENSUS_TOLERANCE_S).count();
        if count > best_count {
            (best, best_count) = (Some(t), count);
        }
    }
    best
}

#[derive(Debug)]
pub struct Measured {
    pub duration: Option<f64>,
    pub fps: Option<f64>,
}

pub fn build_context(
    release: Release,
    measured: Option<Measured>,
    runtime: Option<f64>,
    embedded_last_cues: &[f64],
    candidates: &[Candidate],
) -> Context {
    let usable: Vec<&Candidate> = candidates.iter().filter(|c| !c.rejected).collect();
    let hash_matches: Vec<&&Candidate> = usable.iter().filter(|c| c.hash_match).collect();
    let probed_duration = measured.as_ref().and_then(|m| m.duration);
    Context {
        release,
        duration: probed_duration.or(runtime),
        duration_is_exact: probed_duration.is_some(),
        fps: measured.and_then(|m| m.fps),
        reference_last_cue: median(hash_matches.iter().filter_map(|c| c.last_cue))
            .or_else(|| median(embedded_last_cues.iter().copied())),
        consensus_last_cue: most_common_last_cue(&usable),
        synced_timing_groups: hash_matches.iter().filter_map(|c| c.timing_group.clone()).collect(),
    }
}

fn is_incompatible(sub: &Candidate, ctx: &Context, synced: bool) -> bool {
    if sub.rejected {
        return true;
    }
    if synced {
        return false;
    }
    if let (Some(theirs), Some(ours)) = (sub.fps, ctx.fps)
        && (theirs - ours).abs() > MAX_FPS_DIFF
    {
        return true;
    }
    let (Some(last_cue), Some(duration)) = (sub.last_cue, ctx.duration) else { return false };
    let latest_end = if ctx.duration_is_exact { duration + 30.0 } else { duration * 1.1 + 120.0 };
    last_cue > latest_end || last_cue < duration * 0.5
}

fn timing_points(sub: &Candidate, ctx: &Context) -> Vec<(f64, Option<&'static str>)> {
    let Some(last_cue) = sub.last_cue.filter(|_| !sub.hash_match) else { return vec![] };
    if let Some(reference) = ctx.reference_last_cue {
        let diff = (last_cue - reference).abs();
        return match diff {
            d if d <= SAME_END_TOLERANCE_S => vec![(40.0, Some("same-end"))],
            d if d <= 15.0 => vec![(15.0, None)],
            d if d > 60.0 => vec![(-40.0, None)],
            _ => vec![],
        };
    }
    match ctx.consensus_last_cue {
        Some(consensus) if (last_cue - consensus).abs() <= CONSENSUS_TOLERANCE_S => vec![(10.0, Some("consensus"))],
        _ => vec![],
    }
}

fn release_points(sub: &Candidate, ctx: &Context) -> Vec<(f64, Option<&'static str>)> {
    let video = &ctx.release;
    let theirs = describe_release(if sub.release.is_empty() { &sub.file_name } else { &sub.release });
    let group = if sub.group.is_empty() { theirs.group.clone() } else { Some(sub.group.clone()) };
    let source = theirs.source.or_else(|| source_of(&sub.format));
    let mut points = vec![];
    if video.group.is_some() && group == video.group {
        points.push((30.0, Some("same-group")));
    }
    if let (Some(ours), Some(theirs)) = (video.source, source) {
        points.push(if ours == theirs { (15.0, Some("same-source")) } else { (-15.0, None) });
    }
    if video.quality != "SD" && theirs.quality == video.quality {
        points.push((3.0, None));
    }
    points
}

fn popularity_points(sub: &Candidate) -> Vec<(f64, Option<&'static str>)> {
    vec![
        (((sub.downloads as f64 + 1.0).log10() * 2.0).min(10.0), None),
        (if sub.trusted { 3.0 } else { 0.0 }, None),
        ((sub.rating * 0.5).min(5.0), None),
        (if sub.hearing_impaired { -2.0 } else { 0.0 }, None),
    ]
}

fn sync_points(sub: &Candidate, synced: bool) -> Vec<(f64, Option<&'static str>)> {
    match (sub.hash_match, synced) {
        (true, _) => vec![(100.0, Some("hash"))],
        (false, true) => vec![(60.0, Some("same-timing-as-hash"))],
        _ => vec![],
    }
}

pub fn score(sub: &Candidate, ctx: &Context) -> Option<Scored> {
    let synced = sub.hash_match || sub.timing_group.as_ref().is_some_and(|g| ctx.synced_timing_groups.contains(g));
    if is_incompatible(sub, ctx, synced) {
        return None;
    }
    let points: Vec<(f64, Option<&'static str>)> = [
        sync_points(sub, synced),
        timing_points(sub, ctx),
        release_points(sub, ctx),
        popularity_points(sub),
    ]
    .concat();
    Some(Scored {
        score: points.iter().map(|(value, _)| value).sum(),
        reasons: points.iter().filter_map(|(_, reason)| *reason).collect(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::subtitles::Download;
    use crate::subtitles::decode::Hints;

    fn candidate(id: &str) -> Candidate {
        Candidate {
            id: id.into(),
            lang: "spa".into(),
            release: String::new(),
            file_name: String::new(),
            group: String::new(),
            format: String::new(),
            fps: None,
            last_cue: None,
            hash_match: false,
            timing_group: None,
            downloads: 0,
            trusted: false,
            rating: 0.0,
            hearing_impaired: false,
            rejected: false,
            download: Download::Direct { url: String::new() },
            hints: Hints::default(),
        }
    }

    fn context(candidates: &[Candidate]) -> Context {
        let release = describe_release("Movie.2020.1080p.WEB-DL-NTb.mkv");
        let measured = Measured { duration: Some(6000.0), fps: Some(23.976) };
        build_context(release, Some(measured), None, &[], candidates)
    }

    #[test]
    fn hash_and_timing_win() {
        let hash = Candidate { hash_match: true, timing_group: Some("g".into()), ..candidate("hash") };
        let same_timing = Candidate { timing_group: Some("g".into()), ..candidate("timing") };
        let plain = Candidate { downloads: 100_000, ..candidate("plain") };
        let all = [hash, same_timing, plain];
        let ctx = context(&all);
        let scores: Vec<Scored> = all.iter().map(|c| score(c, &ctx).unwrap()).collect();
        assert!(scores[0].score > scores[1].score && scores[1].score > scores[2].score);
        assert_eq!(scores[0].reasons, vec!["hash"]);
        assert_eq!(scores[1].reasons, vec!["same-timing-as-hash"]);
    }

    #[test]
    fn rejects_what_cannot_fit() {
        let ctx = context(&[]);
        assert!(score(&Candidate { fps: Some(25.0), ..candidate("fps") }, &ctx).is_none());
        assert!(score(&Candidate { last_cue: Some(9000.0), ..candidate("long") }, &ctx).is_none());
        assert!(score(&Candidate { last_cue: Some(1000.0), ..candidate("short") }, &ctx).is_none());
        assert!(score(&Candidate { rejected: true, ..candidate("bad") }, &ctx).is_none());
        let same_group = Candidate { release: "Movie.2020.1080p.WEB-DL-NTb".into(), ..candidate("group") };
        let reasons = score(&same_group, &ctx).unwrap().reasons;
        assert_eq!(reasons, vec!["same-group", "same-source"]);
    }
}
