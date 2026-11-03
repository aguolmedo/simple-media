const LANGUAGES: &[&[&str]] = &[
    &["eng", "en", "english"],
    &["spa", "es", "spanish", "español", "espanol", "castellano", "esp", "spn"],
    &["fre", "fr", "french", "français", "francais", "fra"],
    &["ger", "de", "german", "deutsch", "deu"],
    &["ita", "it", "italian", "italiano"],
    &["por", "pt", "portuguese", "português", "portugues"],
    &["pob", "pb", "ptbr", "brazilian", "brazilian portuguese", "portuguese (brazil)"],
    &["rus", "ru", "russian"],
    &["ara", "ar", "arabic"],
    &["chi", "zh", "chinese", "zho", "zht", "zhe", "chinese (simplified)", "chinese (traditional)", "big 5 code"],
    &["jpn", "ja", "japanese"],
    &["kor", "ko", "korean"],
    &["dut", "nl", "dutch", "nld"],
    &["pol", "pl", "polish"],
    &["tur", "tr", "turkish"],
    &["gre", "el", "greek", "ell"],
    &["heb", "he", "hebrew"],
    &["cze", "cs", "czech", "ces"],
    &["hun", "hu", "hungarian"],
    &["rum", "ro", "romanian", "ron"],
    &["bul", "bg", "bulgarian"],
    &["hrv", "hr", "croatian"],
    &["scc", "sr", "serbian", "srp"],
    &["slv", "sl", "slovenian"],
    &["slo", "sk", "slovak", "slk"],
    &["swe", "sv", "swedish"],
    &["nor", "no", "norwegian", "nob"],
    &["dan", "da", "danish"],
    &["fin", "fi", "finnish"],
    &["per", "fa", "persian", "farsi", "farsi/persian", "fas"],
    &["ind", "id", "indonesian"],
    &["may", "ms", "malay", "msa"],
    &["tha", "th", "thai"],
    &["vie", "vi", "vietnamese"],
    &["ben", "bn", "bengali"],
    &["hin", "hi", "hindi"],
    &["urd", "ur", "urdu"],
    &["alb", "sq", "albanian", "sqi"],
    &["ukr", "uk", "ukrainian"],
    &["est", "et", "estonian"],
    &["lit", "lt", "lithuanian"],
    &["lav", "lv", "latvian"],
    &["mac", "mk", "macedonian", "mkd"],
    &["bos", "bs", "bosnian"],
    &["ice", "is", "icelandic", "isl"],
    &["cat", "ca", "catalan", "català"],
    &["baq", "eu", "basque", "euskera"],
    &["glg", "gl", "galician", "galego"],
];

const CODEPAGES: &[(&str, &[&str])] = &[
    ("windows-1250", &["cze", "hun", "pol", "rum", "hrv", "slv", "slo", "alb", "bos", "scc"]),
    ("windows-1251", &["rus", "ukr", "bul", "mac"]),
    ("windows-1253", &["gre"]),
    ("windows-1254", &["tur"]),
    ("windows-1255", &["heb"]),
    ("windows-1256", &["ara", "per", "urd"]),
    ("windows-1257", &["est", "lit", "lav"]),
    ("gbk", &["chi"]),
    ("shift_jis", &["jpn"]),
    ("euc-kr", &["kor"]),
    ("windows-874", &["tha"]),
];

const FLAG_TOKENS: &[&str] = &["forced", "sdh", "hi", "cc", "default", "full", "srt"];

pub fn to_code(alias: &str) -> Option<&'static str> {
    let alias = alias.trim().to_lowercase();
    LANGUAGES.iter().find(|aliases| aliases.contains(&alias.as_str())).map(|aliases| aliases[0])
}

pub fn codepage_for(code: &str) -> &'static str {
    CODEPAGES
        .iter()
        .find(|(_, codes)| codes.contains(&code))
        .map(|(encoding, _)| *encoding)
        .unwrap_or("windows-1252")
}

pub fn code_from_filename(path: &str) -> Option<&'static str> {
    let lower = path.to_lowercase();
    let stem = ["srt", "vtt", "ass", "ssa", "sub"]
        .iter()
        .find_map(|ext| lower.strip_suffix(&format!(".{ext}")))
        .unwrap_or(&lower)
        .replace("pt-br", "ptbr")
        .replace("pt.br", "ptbr")
        .replace("pt_br", "ptbr")
        .replace("pt br", "ptbr");
    let mut tokens: Vec<&str> = stem
        .split(|c: char| matches!(c, '/' | '.' | '_' | '-' | '[' | ']' | '(' | ')') || c.is_whitespace())
        .filter(|t| !t.is_empty())
        .collect();
    while tokens.last().is_some_and(|t| FLAG_TOKENS.contains(t)) {
        tokens.pop();
    }
    to_code(tokens.last()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn codes() {
        assert_eq!(to_code("Español"), Some("spa"));
        assert_eq!(code_from_filename("Subs/2_Spanish.forced.srt"), Some("spa"));
        assert_eq!(code_from_filename("movie.pt-br.srt"), Some("pob"));
        assert_eq!(codepage_for("rus"), "windows-1251");
    }
}
