//! Fast language identification (whatlang, trigram based, ~microseconds for a paragraph).

pub struct Detected {
    pub code: &'static str,
    pub name: &'static str,
    pub reliable: bool,
}

pub fn detect(text: &str) -> Option<Detected> {
    let info = whatlang::detect(text)?;
    Some(Detected { code: info.lang().code(), name: info.lang().eng_name(), reliable: info.is_reliable() })
}

pub fn is_english(text: &str) -> bool {
    // Short or ambiguous snippets are treated as English, the primary language.
    detect(text).is_none_or(|d| d.code == "eng" || !d.reliable)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_common_languages() {
        assert!(is_english("This is a perfectly ordinary English sentence about writing."));
        let french = detect("Bonjour, je voudrais réserver une table pour deux personnes ce soir.").unwrap();
        assert_eq!(french.code, "fra");
        assert!(!is_english("Hola, ¿cómo estás? Espero que todo vaya muy bien con tu familia."));
    }
}
