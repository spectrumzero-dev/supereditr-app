//! `supereditr` — run the SuperEditr engine from a terminal.
//!
//!   supereditr check [--preset ID] [--provider KIND] [--model M] [--base-url URL]
//!                    [--steer TEXT] [--words "Rhyan,1234 Glenmoor Ct"] [--bench N] [--offline] [--no-local]
//!                    [--json] [TEXT]
//!                    (TEXT or stdin; --offline uses no AI, --no-local skips the offline checker,
//!                    --words is a comma-separated word list)
//!   supereditr detect
//!   supereditr models --provider KIND [--base-url URL]
//!
//! API keys come from the usual environment variables (ANTHROPIC_API_KEY, OPENAI_API_KEY,
//! GEMINI_API_KEY); nothing is stored.

use std::io::Read;
use std::time::Instant;
use supereditr_core::provider::{ProviderKind, ProviderProfile, preferred_model};
use supereditr_core::{Config, Engine, Event, Job};

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

struct Args {
    command: String,
    flags: std::collections::HashMap<String, String>,
    positional: Vec<String>,
    bench: usize,
}

/// Flags that take a value, and switches that don't.
const VALUE_FLAGS: &[&str] = &["preset", "provider", "model", "base-url", "steer", "words", "bench"];
const SWITCHES: &[&str] = &["offline", "json", "no-local"];

/// Parses the arguments after the program name. An unknown flag, a flag missing its value (nothing
/// after it, or another `--flag`), or a `--bench` that isn't a whole number of at least 1 is an error.
fn parse_args(raw: impl IntoIterator<Item = String>) -> Result<Args, String> {
    let mut raw = raw.into_iter();
    let command = raw.next().unwrap_or_else(|| "help".into());
    let mut flags = std::collections::HashMap::new();
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
    let bench = match flags.get("bench") {
        None => 1,
        Some(n) => match n.parse::<usize>() {
            Ok(n) if n >= 1 => n,
            _ => return Err(format!("--bench needs a whole number of at least 1, not {n:?}")),
        },
    };
    Ok(Args { command, flags, positional, bench })
}

fn parse() -> Args {
    parse_args(std::env::args().skip(1)).unwrap_or_else(|message| {
        eprintln!("error: {message}");
        usage()
    })
}

fn usage() -> ! {
    eprintln!(
        "usage:\n  supereditr check [--preset proofread|polish|shorter|friendly|professional|simplify|english]\n                   [--provider anthropic|openai|gemini|ollama|compatible|claude-cli|apple] [--model M]\n                   [--base-url URL] [--steer TEXT] [--words LIST] [--bench N] [--offline] [--no-local] [--json] [TEXT]\n  supereditr detect\n  supereditr models --provider KIND [--base-url URL]"
    );
    std::process::exit(2)
}

fn profile_from(args: &Args) -> Option<ProviderProfile> {
    if args.flags.contains_key("offline") {
        return None;
    }
    let kind = match args.flags.get("provider") {
        Some(name) => kind_from(name).unwrap_or_else(|| usage()),
        None => [ProviderKind::Anthropic, ProviderKind::OpenAi, ProviderKind::Gemini]
            .into_iter()
            .find(|k| env_key(*k).is_some())?,
    };
    let mut profile = ProviderProfile::new(kind);
    profile.api_key = env_key(kind);
    if let Some(url) = args.flags.get("base-url") {
        profile.base_url = url.clone();
    }
    if let Some(model) = args.flags.get("model") {
        profile.model = model.clone();
    }
    Some(profile)
}

#[tokio::main]
async fn main() {
    let args = parse();
    match args.command.as_str() {
        "check" => check(args).await,
        "detect" => detect().await,
        "models" => models(args).await,
        _ => usage(),
    }
}

async fn check(args: Args) {
    let text = if args.positional.is_empty() {
        let mut buf = String::new();
        std::io::stdin().read_to_string(&mut buf).expect("read stdin");
        buf
    } else {
        args.positional.join(" ")
    };
    let mut config = Config::default();
    if let Some(list) = args.flags.get("words") {
        config.words = list.split(',').map(str::trim).filter(|w| !w.is_empty()).map(String::from).collect();
    }
    let preset_id = args.flags.get("preset").map(String::as_str).unwrap_or("proofread");
    let preset = config.preset(preset_id).cloned().unwrap_or_else(|| usage());
    let profile = profile_from(&args);
    let engine = Engine::new(&config.dialect);
    let warm_start = Instant::now();
    engine.prepare(&config.words);
    if let Some(p) = &profile {
        engine.providers.warm(p).await;
    }
    let json = args.flags.contains_key("json");
    if !json {
        eprintln!(
            "prepared in {} ms · {}",
            warm_start.elapsed().as_millis(),
            profile.as_ref().map(|p| format!("{} / {}", p.label, p.model)).unwrap_or_else(|| "offline checker".into())
        );
    }
    let runs = args.bench;
    let mut firsts = Vec::new();
    let mut totals = Vec::new();
    for run in 0..runs {
        let job = Job {
            text: text.clone(),
            preset: preset.clone(),
            steer: args.flags.get("steer").cloned(),
            profile: profile.clone(),
            style_notes: String::new(),
            dialect: config.dialect.clone(),
            words: config.words.clone(),
            code: Vec::new(),
            local_checks: !args.flags.contains_key("no-local"),
            builtin_ai: !args.flags.contains_key("offline"),
        };
        let mut printed_local = false;
        engine
            .run(job, &mut |event| match event {
                Event::Local { view, ms } if !json && run == 0 && !printed_local => {
                    printed_local = true;
                    eprintln!("local check: {} change(s) in {ms} ms", view.change_count);
                }
                Event::Done(outcome) => {
                    if let Some(f) = outcome.first_token_ms {
                        firsts.push(f);
                    }
                    totals.push(outcome.total_ms);
                    if run == 0 {
                        if json {
                            println!("{}", serde_json::to_string_pretty(&outcome).unwrap());
                        } else {
                            // Like quick fix, without changes that would drop the author's terms.
                            println!("{}", outcome.kept_text());
                            eprintln!(
                                "{} change(s) · first token {} · total {} ms · {}",
                                outcome.view.change_count,
                                outcome.first_token_ms.map(|f| format!("{f} ms")).unwrap_or("–".into()),
                                outcome.total_ms,
                                outcome.source
                            );
                        }
                    }
                }
                Event::Failed { message, .. } => {
                    eprintln!("error: {message}");
                    std::process::exit(1);
                }
                _ => {}
            })
            .await;
    }
    if runs > 1 {
        let median = |v: &mut Vec<u64>| {
            v.sort();
            v.get(v.len() / 2).copied().unwrap_or(0)
        };
        eprintln!(
            "bench ×{runs}: median first token {} ms, median total {} ms",
            median(&mut firsts),
            median(&mut totals)
        );
    }
}

async fn detect() {
    let engine = Engine::new("American English");
    let home = std::env::var("HOME").map(std::path::PathBuf::from).unwrap_or_default();
    let started = Instant::now();
    let found = supereditr_core::detect::scan(&engine.providers, &home).await;
    for c in &found {
        println!(
            "{:<26} {}{}{}",
            c.kind.display_name(),
            c.source,
            c.key_hint.as_ref().map(|k| format!(" [{k}]")).unwrap_or_default(),
            c.note.as_ref().map(|n| format!(" — {n}")).unwrap_or_default()
        );
    }
    eprintln!("{} found in {} ms", found.len(), started.elapsed().as_millis());
}

async fn models(args: Args) {
    let Some(profile) = profile_from(&args) else { usage() };
    let engine = Engine::new("American English");
    match engine.providers.list_models(&profile).await {
        Ok(models) => {
            for m in &models {
                println!("{m}");
            }
            eprintln!("preferred: {}", preferred_model(profile.kind, &models));
        }
        Err(e) => {
            eprintln!("error: {e}");
            std::process::exit(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_strs(args: &[&str]) -> Result<Args, String> {
        parse_args(args.iter().map(|a| a.to_string()))
    }

    #[test]
    fn valid_flags_parse() {
        let args = parse_strs(&[
            "check",
            "--provider",
            "openai",
            "--model",
            "m",
            "--bench",
            "3",
            "--offline",
            "--no-local",
            "--json",
            "Rhyan wrote",
            "this",
        ])
        .unwrap();
        assert_eq!(args.command, "check");
        assert_eq!(args.flags["provider"], "openai");
        assert_eq!(args.flags["model"], "m");
        assert!(args.flags.contains_key("offline"));
        assert!(args.flags.contains_key("no-local"));
        assert!(args.flags.contains_key("json"));
        assert_eq!(args.bench, 3);
        assert_eq!(args.positional, ["Rhyan wrote", "this"]);
    }

    #[test]
    fn bench_defaults_to_one() {
        assert_eq!(parse_strs(&["check", "text"]).unwrap().bench, 1);
    }

    #[test]
    fn unknown_flag_is_an_error() {
        let err = parse_strs(&["check", "--provder", "openai"]).err().unwrap();
        assert!(err.contains("--provder"), "{err}");
    }

    #[test]
    fn bad_bench_is_an_error() {
        for bad in ["abc", "0", "-1", "2.5", ""] {
            assert!(parse_strs(&["check", "--bench", bad]).is_err(), "--bench {bad:?} should fail");
        }
    }

    #[test]
    fn missing_value_is_an_error() {
        assert!(parse_strs(&["check", "--bench"]).is_err());
        assert!(parse_strs(&["models", "--provider"]).is_err());
        // A value flag doesn't swallow the next flag, known or unknown.
        assert!(parse_strs(&["check", "--words", "--offline", "text"]).is_err());
        let err = parse_strs(&["check", "--words", "--provder", "Rhyan"]).err().unwrap();
        assert!(err.contains("--words"), "{err}");
    }
}
