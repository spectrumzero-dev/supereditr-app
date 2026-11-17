//! The eval corpus: invented texts, each with a category, the task to run on it, and what a good
//! result looks like.

use serde::{Deserialize, Serialize};
use std::path::Path;

/// The kinds of selection the corpus covers. Reports break every metric down by these.
pub const CATEGORIES: &[&str] =
    &["draft", "formatted", "markdown_code", "long", "question", "injection", "correct", "non_english"];

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct Sample {
    pub id: String,
    pub category: String,
    /// A built-in preset id from `supereditr_core::prompts::builtin_presets`.
    #[serde(default = "proofread")]
    pub preset: String,
    pub text: String,
    /// Acceptable results, any of which counts as an exact match. Empty for tasks with no single
    /// right answer (Polish, Shorter and the other rewrites); for the `correct` category it
    /// defaults to the text itself.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub expected: Vec<String>,
    /// The author's word list for this sample, passed as the app passes it.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub words: Vec<String>,
    /// Terms any good result still contains (compared without regard to case), for tasks where
    /// exact matching doesn't apply.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub must_keep: Vec<String>,
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub note: String,
}

fn proofread() -> String {
    "proofread".into()
}

impl Sample {
    pub fn is_proofread(&self) -> bool {
        self.preset == "proofread"
    }

    /// Whether a correct result differs from the text, so leaving it unchanged is a miss.
    pub fn needs_change(&self) -> bool {
        !self.expected.is_empty() && !self.expected.contains(&self.text)
    }
}

/// Reads a corpus file: one JSON object per line; blank lines and lines starting with `//` are
/// skipped. Checks it the same way the corpus test does.
pub fn load(path: &Path) -> Result<Vec<Sample>, String> {
    let content = std::fs::read_to_string(path).map_err(|e| format!("can't read {}: {e}", path.display()))?;
    parse(&content)
}

pub fn parse(content: &str) -> Result<Vec<Sample>, String> {
    let mut samples = Vec::new();
    for (n, line) in content.lines().enumerate() {
        let line = line.trim();
        if line.is_empty() || line.starts_with("//") {
            continue;
        }
        let mut sample: Sample = serde_json::from_str(line).map_err(|e| format!("corpus line {}: {e}", n + 1))?;
        if sample.category == "correct" && sample.expected.is_empty() {
            sample.expected.push(sample.text.clone());
        }
        samples.push(sample);
    }
    validate(&samples)?;
    Ok(samples)
}

fn validate(samples: &[Sample]) -> Result<(), String> {
    let presets: Vec<String> = supereditr_core::prompts::builtin_presets().into_iter().map(|p| p.id).collect();
    let mut ids = std::collections::HashSet::new();
    for sample in samples {
        if !ids.insert(&sample.id) {
            return Err(format!("duplicate sample id {}", sample.id));
        }
        if !CATEGORIES.contains(&sample.category.as_str()) {
            return Err(format!("{}: unknown category {}", sample.id, sample.category));
        }
        if !presets.contains(&sample.preset) {
            return Err(format!("{}: unknown preset {}", sample.id, sample.preset));
        }
        if sample.is_proofread() && sample.expected.is_empty() {
            return Err(format!("{}: a proofread sample needs an expected result", sample.id));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn corpus() -> Vec<Sample> {
        load(&Path::new(env!("CARGO_MANIFEST_DIR")).join("corpus.jsonl")).unwrap()
    }

    #[test]
    fn the_corpus_loads_and_covers_every_category_and_preset() {
        let samples = corpus();
        assert!((60..=100).contains(&samples.len()), "{} samples", samples.len());
        for category in CATEGORIES {
            assert!(samples.iter().any(|s| s.category == *category), "no {category} samples");
        }
        for preset in supereditr_core::prompts::builtin_presets() {
            assert!(samples.iter().any(|s| s.preset == preset.id), "no {} samples", preset.id);
        }
        let drafts = samples.iter().filter(|s| s.category == "draft").count();
        let largest_other = CATEGORIES
            .iter()
            .filter(|c| **c != "draft")
            .map(|c| samples.iter().filter(|s| s.category == *c).count())
            .max()
            .unwrap();
        assert!(drafts > largest_other, "ordinary drafts are the common case");
    }

    #[test]
    fn the_corpus_has_the_question_that_started_it() {
        let samples = corpus();
        let question = samples
            .iter()
            .find(|s| s.text.starts_with("are there any other tricks or techniques we can take"))
            .expect("the latency question");
        assert_eq!(question.category, "question");
        assert!(question.expected.contains(
            &"Are there any other tricks or techniques we can use (without compromising or doing really hacky things) to improve the latency or perceived latency?".to_string()
        ));
    }

    #[test]
    fn rejects_bad_samples() {
        assert!(parse(r#"{"id":"a","category":"draft","text":"x"}"#).unwrap_err().contains("expected"));
        assert!(parse(r#"{"id":"a","category":"nope","text":"x","expected":["x"]}"#).is_err());
        let twice = "{\"id\":\"a\",\"category\":\"correct\",\"text\":\"x\"}\n{\"id\":\"a\",\"category\":\"correct\",\"text\":\"y\"}";
        assert!(parse(twice).unwrap_err().contains("duplicate"));
        let correct = parse(r#"{"id":"a","category":"correct","text":"Fine."}"#).unwrap();
        assert_eq!(correct[0].expected, vec!["Fine.".to_string()]);
        assert!(!correct[0].needs_change());
    }
}
