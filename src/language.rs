//! Speech-language choices shared by settings, quick controls, and the bar.
//! This is a vocabulary, not a claim that every backend model supports every language.
use anyhow::{Result, ensure};

pub(crate) const LANGUAGES: &[(&str, &str)] = &[
    ("", "Auto-detect"),
    ("af", "Afrikaans"),
    ("sq", "Albanian"),
    ("am", "Amharic"),
    ("ar", "Arabic"),
    ("hy", "Armenian"),
    ("as", "Assamese"),
    ("az", "Azerbaijani"),
    ("ba", "Bashkir"),
    ("eu", "Basque"),
    ("be", "Belarusian"),
    ("bn", "Bengali"),
    ("bs", "Bosnian"),
    ("br", "Breton"),
    ("bg", "Bulgarian"),
    ("my", "Burmese"),
    ("yue", "Cantonese"),
    ("ca", "Catalan"),
    ("zh", "Chinese"),
    ("hr", "Croatian"),
    ("cs", "Czech"),
    ("da", "Danish"),
    ("nl", "Dutch"),
    ("en", "English"),
    ("et", "Estonian"),
    ("fo", "Faroese"),
    ("fi", "Finnish"),
    ("fr", "French"),
    ("gl", "Galician"),
    ("ka", "Georgian"),
    ("de", "German"),
    ("el", "Greek"),
    ("gu", "Gujarati"),
    ("ht", "Haitian Creole"),
    ("ha", "Hausa"),
    ("haw", "Hawaiian"),
    ("he", "Hebrew"),
    ("hi", "Hindi"),
    ("hu", "Hungarian"),
    ("is", "Icelandic"),
    ("id", "Indonesian"),
    ("it", "Italian"),
    ("ja", "Japanese"),
    ("jw", "Javanese"),
    ("kn", "Kannada"),
    ("kk", "Kazakh"),
    ("km", "Khmer"),
    ("ko", "Korean"),
    ("lo", "Lao"),
    ("la", "Latin"),
    ("lv", "Latvian"),
    ("ln", "Lingala"),
    ("lt", "Lithuanian"),
    ("lb", "Luxembourgish"),
    ("mk", "Macedonian"),
    ("mg", "Malagasy"),
    ("ms", "Malay"),
    ("ml", "Malayalam"),
    ("mt", "Maltese"),
    ("mi", "Maori"),
    ("mr", "Marathi"),
    ("mn", "Mongolian"),
    ("ne", "Nepali"),
    ("no", "Norwegian"),
    ("nn", "Norwegian Nynorsk"),
    ("oc", "Occitan"),
    ("ps", "Pashto"),
    ("fa", "Persian"),
    ("pl", "Polish"),
    ("pt", "Portuguese"),
    ("pa", "Punjabi"),
    ("ro", "Romanian"),
    ("ru", "Russian"),
    ("sa", "Sanskrit"),
    ("sr", "Serbian"),
    ("sn", "Shona"),
    ("sd", "Sindhi"),
    ("si", "Sinhala"),
    ("sk", "Slovak"),
    ("sl", "Slovenian"),
    ("so", "Somali"),
    ("es", "Spanish"),
    ("su", "Sundanese"),
    ("sw", "Swahili"),
    ("sv", "Swedish"),
    ("tl", "Tagalog"),
    ("tg", "Tajik"),
    ("ta", "Tamil"),
    ("tt", "Tatar"),
    ("te", "Telugu"),
    ("th", "Thai"),
    ("bo", "Tibetan"),
    ("tr", "Turkish"),
    ("tk", "Turkmen"),
    ("uk", "Ukrainian"),
    ("ur", "Urdu"),
    ("uz", "Uzbek"),
    ("vi", "Vietnamese"),
    ("cy", "Welsh"),
    ("yi", "Yiddish"),
    ("yo", "Yoruba"),
];

pub(crate) fn normalize(value: &str) -> Result<String> {
    let value = value.trim();
    if value.is_empty() || value.eq_ignore_ascii_case("auto") {
        return Ok(String::new());
    }
    LANGUAGES
        .iter()
        .find(|(code, name)| code.eq_ignore_ascii_case(value) || name.eq_ignore_ascii_case(value))
        .map(|(code, _)| (*code).to_owned())
        .ok_or_else(|| anyhow::anyhow!("Unknown language. Use a listed language code or auto."))
}

pub(crate) fn name(code: &str) -> &str {
    LANGUAGES
        .iter()
        .find(|(value, _)| *value == code)
        .map_or(code, |(_, name)| *name)
}

/// Only catalog values reach Pango, never arbitrary settings or IPC strings.
pub(crate) fn short_label(code: &str) -> String {
    match normalize(code) {
        Ok(code) if code.is_empty() => "Auto".into(),
        Ok(code) => code.to_ascii_uppercase(),
        Err(_) => "?".into(),
    }
}

pub(crate) fn default_quick_languages() -> Vec<String> {
    vec![String::new(), "en".into(), "de".into()]
}

pub(crate) fn validate_quick_languages(values: &[String]) -> Result<Vec<String>> {
    ensure!(
        !values.is_empty() && values.len() <= 8,
        "Choose between one and eight quick languages."
    );
    let mut result = Vec::new();
    for value in values {
        let code = normalize(value)?;
        if !result.contains(&code) {
            result.push(code);
        }
    }
    Ok(result)
}

pub(crate) fn parse_quick_languages(text: &str) -> Result<Vec<String>> {
    ensure!(
        !text.trim().is_empty(),
        "Enter quick languages, for example: auto, en, de."
    );
    validate_quick_languages(
        &text
            .split(',')
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned)
            .collect::<Vec<_>>(),
    )
}

pub(crate) fn quick_languages_text(values: &[String]) -> String {
    values
        .iter()
        .map(|code| if code.is_empty() { "auto" } else { code })
        .collect::<Vec<_>>()
        .join(", ")
}

pub(crate) fn next<'a>(current: &str, choices: &'a [String]) -> Option<&'a str> {
    if choices.is_empty() {
        return None;
    }
    let ix = choices
        .iter()
        .position(|code| code == current)
        .map_or(0, |ix| (ix + 1) % choices.len());
    Some(&choices[ix])
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn normalizes_codes_names_and_auto() {
        assert_eq!(normalize(" EN ").unwrap(), "en");
        assert_eq!(normalize("German").unwrap(), "de");
        assert_eq!(normalize("auto").unwrap(), "");
        assert!(normalize("<b>en</b>").is_err());
        assert_eq!(short_label("<b>en</b>"), "?");
    }
    #[test]
    fn quick_languages_are_bounded_normalized_and_deduplicated() {
        assert_eq!(
            parse_quick_languages("auto, EN, english, de").unwrap(),
            default_quick_languages()
        );
        assert_eq!(parse_quick_languages("en, de,").unwrap(), vec!["en", "de"]);
        assert!(parse_quick_languages("").is_err());
        assert!(parse_quick_languages("unknown").is_err());
        assert!(parse_quick_languages("en,de,fr,es,it,pt,ja,zh,ko").is_err());
    }
    #[test]
    fn cycle_wraps_and_recovers_when_current_is_not_a_favorite() {
        let choices = default_quick_languages();
        assert_eq!(next("", &choices), Some("en"));
        assert_eq!(next("en", &choices), Some("de"));
        assert_eq!(next("de", &choices), Some(""));
        assert_eq!(next("fr", &choices), Some(""));
        assert_eq!(next("en", &[]), None);
    }
    #[test]
    fn catalog_codes_are_unique() {
        let codes: std::collections::HashSet<_> = LANGUAGES.iter().map(|(code, _)| code).collect();
        assert_eq!(codes.len(), LANGUAGES.len());
    }
}
