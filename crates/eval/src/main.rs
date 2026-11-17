//! `supereditr-eval`: runs SuperEditr's AI edits over a fixed corpus and scores the results.
//!
//!   supereditr-eval run --provider KIND [--model M] [--base-url URL] [--modes plain,guided,shipped]
//!                       [--runs N] [--only ID|CATEGORY|PRESET,...] [--corpus FILE] --out FILE
//!   supereditr-eval score RESULTS... [--baseline SUMMARY] [--summary FILE] [--report FILE]
//!                         [--fail-on-regression]
//!
//! `run` appends one JSON line per run to `--out`. `score` prints a Markdown report, writes a
//! summary that can serve as the next baseline, and lists regressions against `--baseline`.
//! API keys come from ANTHROPIC_API_KEY, OPENAI_API_KEY and GEMINI_API_KEY (or GOOGLE_API_KEY).
//! See crates/eval/README.md.

mod corpus;
mod metrics;
mod run;
mod score;

use std::collections::HashMap;
use std::path::PathBuf;
use supereditr_core::provider::{ProviderKind, ProviderProfile};

const USAGE: &str = "usage:
  supereditr-eval run --provider apple|anthropic|openai|gemini|ollama|compatible|claude-cli [--model M]
                      [--base-url URL] [--modes plain,guided,shipped] [--runs N] [--only ID|CATEGORY|PRESET,...]
                      [--corpus FILE] --out FILE
  supereditr-eval score RESULTS... [--baseline SUMMARY] [--summary FILE] [--report FILE] [--fail-on-regression]";

fn fail(message: &str) -> ! {
    eprintln!("error: {message}\n{USAGE}");
    std::process::exit(2)
}

struct Args {
    command: String,
    flags: HashMap<String, String>,
    positional: Vec<String>,
}

const VALUE_FLAGS: &[&str] =
    &["provider", "model", "base-url", "modes", "runs", "only", "corpus", "out", "baseline", "summary", "report"];
const SWITCHES: &[&str] = &["fail-on-regression"];

fn parse_args(raw: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut raw = raw.into_iter();
    let command = raw.next().unwrap_or_default();
    let mut flags = HashMap::new();
    let mut positional = Vec::new();
    while let Some(arg) = raw.next() {
        if let Some(name) = arg.strip_prefix("--") {
            let value = if SWITCHES.contains(&name) {
                "1".into()
            } else if VALUE_FLAGS.contains(&name) {
                raw.next().filter(|v| !v.starts_with("--")).ok_or_else(|| format!("--{name} needs a value"))?
            } else {
                return Err(format!("unknown flag --{name}"));
            };
            flags.insert(name.to_string(), value);
        } else {
            positional.push(arg);
        }
    }
    Ok(Args { command, flags, positional })
}

fn kind_from(name: &str) -> Option<ProviderKind> {
    Some(match name {
        "anthropic" | "claude" => ProviderKind::Anthropic,
        "openai" => ProviderKind::OpenAi,
        "gemini" | "google" => ProviderKind::Gemini,
        "ollama" => ProviderKind::Ollama,
        "compatible" | "lmstudio" => ProviderKind::OpenAiCompatible,
        "claude-cli" => ProviderKind::ClaudeCli,
        "apple" => ProviderKind::AppleIntelligence,
        _ => return None,
    })
}

fn env_key(kind: ProviderKind) -> Option<String> {
    let names: &[&str] = match kind {
        ProviderKind::Anthropic => &["ANTHROPIC_API_KEY"],
        ProviderKind::OpenAi => &["OPENAI_API_KEY"],
        ProviderKind::Gemini => &["GEMINI_API_KEY", "GOOGLE_API_KEY"],
        ProviderKind::OpenAiCompatible => &["OPENAI_COMPATIBLE_API_KEY"],
        _ => &[],
    };
    names.iter().find_map(|n| std::env::var(n).ok().filter(|v| !v.is_empty()))
}

fn default_corpus() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("corpus.jsonl")
}

#[tokio::main]
async fn main() {
    let args = parse_args(std::env::args().skip(1)).unwrap_or_else(|e| fail(&e));
    match args.command.as_str() {
        "run" => run_command(args).await,
        "score" => score_command(args),
        _ => fail("name a command"),
    }
}

async fn run_command(args: Args) {
    let provider = args.flags.get("provider").unwrap_or_else(|| fail("--provider is required"));
    let kind = kind_from(provider).unwrap_or_else(|| fail(&format!("unknown provider {provider}")));
    let mut profile = ProviderProfile::new(kind);
    profile.api_key = env_key(kind);
    if kind.needs_key() && profile.api_key.is_none() {
        fail(&format!("{provider} needs its API key in the environment"));
    }
    if let Some(url) = args.flags.get("base-url") {
        profile.base_url = url.clone();
    }
    if let Some(model) = args.flags.get("model") {
        profile.model = model.clone();
    }
    let default_modes = if kind == ProviderKind::AppleIntelligence { "plain,guided,shipped" } else { "plain,shipped" };
    let modes: Vec<run::Mode> = args
        .flags
        .get("modes")
        .map_or(default_modes, String::as_str)
        .split(',')
        .map(|m| run::Mode::parse(m.trim()).unwrap_or_else(|| fail(&format!("unknown mode {m}"))))
        .collect();
    if modes.contains(&run::Mode::Guided) && kind != ProviderKind::AppleIntelligence {
        fail("guided mode exists only for Apple Intelligence");
    }
    let runs = match args.flags.get("runs").map(|n| n.parse::<usize>()) {
        None => 3,
        Some(Ok(n)) if n >= 1 => n,
        _ => fail("--runs needs a whole number of at least 1"),
    };
    let corpus_path = args.flags.get("corpus").map_or_else(default_corpus, PathBuf::from);
    let mut samples = corpus::load(&corpus_path).unwrap_or_else(|e| fail(&e));
    if let Some(only) = args.flags.get("only") {
        let wanted: Vec<&str> = only.split(',').map(str::trim).collect();
        samples.retain(|s| wanted.iter().any(|w| *w == s.id || *w == s.category || *w == s.preset));
        if samples.is_empty() {
            fail(&format!("no sample matches --only {only}"));
        }
    }
    let out_path = args.flags.get("out").unwrap_or_else(|| fail("--out is required"));
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(out_path)
        .unwrap_or_else(|e| fail(&format!("can't open {out_path}: {e}")));
    let options = run::Options { profile, modes, runs, dialect: "American English".into() };
    match run::run(&samples, &options, &mut out).await {
        Ok(n) => eprintln!("Wrote {n} runs to {out_path}"),
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1)
        }
    }
}

/// A regression gate with nothing to compare against would pass every run.
fn check_score_flags(args: &Args) -> Result<(), String> {
    if args.positional.is_empty() {
        return Err("name at least one results file".into());
    }
    if args.flags.contains_key("fail-on-regression") && !args.flags.contains_key("baseline") {
        return Err("--fail-on-regression needs --baseline".into());
    }
    Ok(())
}

fn score_command(args: Args) {
    if let Err(message) = check_score_flags(&args) {
        fail(&message);
    }
    let mut records = Vec::new();
    for path in &args.positional {
        let content = std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("can't read {path}: {e}")));
        for (n, line) in content.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
            let record: run::Record =
                serde_json::from_str(line).unwrap_or_else(|e| fail(&format!("{path} line {}: {e}", n + 1)));
            records.push(record);
        }
    }
    let summary = score::summarize(&records);
    let comparison = args.flags.get("baseline").map(|path| {
        let content = std::fs::read_to_string(path).unwrap_or_else(|e| fail(&format!("can't read {path}: {e}")));
        let baseline: score::Summary =
            serde_json::from_str(&content).unwrap_or_else(|e| fail(&format!("{path} isn't a summary: {e}")));
        (score::regressions(&summary, &baseline), score::unmatched(&summary, &baseline))
    });
    let unmatched = comparison.as_ref().map_or(&[][..], |(_, u)| u.as_slice());
    let report = score::report(&summary, comparison.as_ref().map(|(r, _)| r.as_slice()), unmatched);
    match args.flags.get("report") {
        Some(path) => std::fs::write(path, &report).unwrap_or_else(|e| fail(&format!("can't write {path}: {e}"))),
        None => print!("{report}"),
    }
    if let Some(path) = args.flags.get("summary") {
        let json = serde_json::to_string_pretty(&summary).expect("a summary serializes");
        std::fs::write(path, json + "\n").unwrap_or_else(|e| fail(&format!("can't write {path}: {e}")));
    }
    if let Some((found, unmatched)) = &comparison {
        eprintln!("{} regressions against the baseline", found.len());
        for label in unmatched {
            eprintln!("no baseline to compare {label} with");
        }
        if (!found.is_empty() || !unmatched.is_empty()) && args.flags.contains_key("fail-on-regression") {
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(line: &str) -> Args {
        parse_args(line.split_whitespace().map(str::to_string)).unwrap()
    }

    #[test]
    fn a_regression_gate_needs_a_baseline() {
        assert!(check_score_flags(&args("score r.jsonl --fail-on-regression")).unwrap_err().contains("--baseline"));
        assert!(check_score_flags(&args("score r.jsonl --fail-on-regression --baseline b.json")).is_ok());
        assert!(check_score_flags(&args("score r.jsonl")).is_ok());
        assert!(check_score_flags(&args("score --baseline b.json")).is_err());
    }
}
