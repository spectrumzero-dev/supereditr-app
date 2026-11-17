//! Runs the corpus through the app's own code: the prompt builder, a provider, `clean_output` and
//! the guard, or the whole engine.

use crate::corpus::Sample;
use serde::{Deserialize, Serialize};
use std::io::Write;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use supereditr_core::provider::{ProviderError, ProviderKind, ProviderProfile};
use supereditr_core::{Engine, Event, Job, lang, prompts, words};

/// How a sample is sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    /// The app's prompt to the model as free text, then `clean_output` and the guard: the model
    /// alone, without the engine's retry, final pass or code protection.
    Plain,
    /// The same with guided generation (Apple Intelligence only).
    Guided,
    /// The whole engine (`Engine::run`), exactly as the app reviews a selection, including any
    /// retry and the offline checker's final pass. Its result is what the review panel applies.
    Shipped,
}

impl Mode {
    pub fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "plain" => Self::Plain,
            "guided" => Self::Guided,
            "shipped" => Self::Shipped,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::Plain => "plain",
            Self::Guided => "guided",
            Self::Shipped => "shipped",
        }
    }
}

/// Where and with what a run happened.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq)]
pub struct Env {
    pub os: String,
    pub os_version: String,
    pub os_build: String,
    pub arch: String,
    pub app_version: String,
    /// The commit of this repository the runner was built from, when known.
    pub app_commit: String,
}

impl Env {
    pub fn detect() -> Self {
        let os = std::env::consts::OS.to_string();
        let (os_version, os_build) = match os.as_str() {
            "macos" => (command("sw_vers", &["-productVersion"]), command("sw_vers", &["-buildVersion"])),
            "linux" => (os_release("PRETTY_NAME"), command("uname", &["-r"])),
            _ => (String::new(), String::new()),
        };
        let app_commit = std::env::var("SUPEREDITR_EVAL_COMMIT")
            .ok()
            .filter(|c| !c.is_empty())
            .unwrap_or_else(|| command("git", &["rev-parse", "HEAD"]));
        Self {
            os,
            os_version,
            os_build,
            arch: std::env::consts::ARCH.into(),
            app_version: env!("CARGO_PKG_VERSION").into(),
            app_commit,
        }
    }
}

fn command(program: &str, args: &[&str]) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

fn os_release(key: &str) -> String {
    let content = std::fs::read_to_string("/etc/os-release").unwrap_or_default();
    content
        .lines()
        .find_map(|l| l.strip_prefix(&format!("{key}=")))
        .map(|v| v.trim_matches('"').to_string())
        .unwrap_or_default()
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Outcome {
    /// The user would get `output` to review.
    Edited,
    /// The guard rejected the reply: the user would see "answered the text instead of editing it"
    /// or "returned no text".
    Refused,
    /// The provider failed.
    #[default]
    Error,
}

/// One run of one sample: a line of the results file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Record {
    pub run_id: String,
    pub started_unix: u64,
    pub env: Env,
    pub provider: String,
    pub model: String,
    /// What identifies the model's weights: the model name for a cloud or local server, the OS
    /// build for Apple's on-device model, which ships with the OS.
    pub model_identity: String,
    pub mode: String,
    pub rep: usize,
    pub sample: Sample,
    pub outcome: Outcome,
    /// What the user would get, when `outcome` is `edited`.
    #[serde(default)]
    pub output: Option<String>,
    /// The model's reply after `clean_output`, before the guard (plain and guided modes).
    #[serde(default)]
    pub reply: Option<String>,
    /// The reply as the model wrote it (plain and guided modes).
    #[serde(default)]
    pub raw: Option<String>,
    /// Whether `answers_instead_of_editing` flagged the reply (plain and guided modes).
    #[serde(default)]
    pub guard_flagged: Option<bool>,
    /// The error or refusal message.
    #[serde(default)]
    pub message: Option<String>,
    #[serde(default)]
    pub first_token_ms: Option<u64>,
    pub total_ms: u64,
    /// Requests repeated after the provider rate-limited one.
    #[serde(default)]
    pub rate_limit_retries: u32,
}

pub struct Options {
    pub profile: ProviderProfile,
    pub modes: Vec<Mode>,
    pub runs: usize,
    pub dialect: String,
}

fn kind_name(kind: ProviderKind) -> &'static str {
    match kind {
        ProviderKind::Anthropic => "anthropic",
        ProviderKind::OpenAi => "openai",
        ProviderKind::Gemini => "gemini",
        ProviderKind::Ollama => "ollama",
        ProviderKind::OpenAiCompatible => "compatible",
        ProviderKind::ClaudeCli => "claude-cli",
        ProviderKind::AppleIntelligence => "apple",
    }
}

/// Runs every sample `runs` times in each mode, appending a record per run to `out` as it goes.
pub async fn run(samples: &[Sample], options: &Options, out: &mut dyn Write) -> Result<usize, String> {
    let env = Env::detect();
    let engine = Engine::new(&options.dialect);
    let profile = &options.profile;
    if profile.kind == ProviderKind::AppleIntelligence {
        let status = engine.providers.apple_status(profile).await;
        if !status.available {
            return Err(format!(
                "Apple Intelligence isn't available: {}",
                status.message().unwrap_or_else(|| "no reason given".into())
            ));
        }
        eprintln!("Apple Intelligence is available ({} {})", env.os_version, env.os_build);
    }
    let model_identity = match profile.kind {
        ProviderKind::AppleIntelligence => format!("apple-on-device/{}", env.os_build),
        _ => profile.model.clone(),
    };
    let started_unix = SystemTime::now().duration_since(UNIX_EPOCH).map_or(0, |d| d.as_secs());
    let mut written = 0;
    for &mode in &options.modes {
        let run_id = format!("{}-{}-{}", kind_name(profile.kind), mode.name(), started_unix);
        // The first request pays for loading the model or opening a connection; it isn't measured.
        engine.providers.warm(profile).await;
        let warmup = Sample {
            id: "warmup".into(),
            category: "draft".into(),
            preset: "proofread".into(),
            text: "Thanks for the update, see you tomorow.".into(),
            expected: Vec::new(),
            words: Vec::new(),
            must_keep: Vec::new(),
            note: String::new(),
        };
        let _ = once(&engine, options, mode, &warmup).await;
        for sample in samples {
            for rep in 1..=options.runs {
                let result = once(&engine, options, mode, sample).await;
                let record = Record {
                    run_id: run_id.clone(),
                    started_unix,
                    env: env.clone(),
                    provider: kind_name(profile.kind).into(),
                    model: profile.model.clone(),
                    model_identity: model_identity.clone(),
                    mode: mode.name().into(),
                    rep,
                    sample: sample.clone(),
                    outcome: result.outcome,
                    output: result.output,
                    reply: result.reply,
                    raw: result.raw,
                    guard_flagged: result.guard_flagged,
                    message: result.message,
                    first_token_ms: result.first_token_ms,
                    total_ms: result.total_ms,
                    rate_limit_retries: result.rate_limit_retries,
                };
                eprintln!(
                    "{:>7} {:<22} run {rep}: {:?} in {} ms",
                    mode.name(),
                    sample.id,
                    record.outcome,
                    record.total_ms
                );
                let line = serde_json::to_string(&record).map_err(|e| e.to_string())?;
                writeln!(out, "{line}").map_err(|e| e.to_string())?;
                out.flush().map_err(|e| e.to_string())?;
                written += 1;
            }
        }
    }
    Ok(written)
}

/// What one request gave, before it becomes a [`Record`].
#[derive(Default)]
struct RunResult {
    outcome: Outcome,
    output: Option<String>,
    reply: Option<String>,
    raw: Option<String>,
    guard_flagged: Option<bool>,
    message: Option<String>,
    first_token_ms: Option<u64>,
    total_ms: u64,
    rate_limit_retries: u32,
}

const RATE_LIMIT_RETRIES: u32 = 3;

async fn once(engine: &Engine, options: &Options, mode: Mode, sample: &Sample) -> RunResult {
    let mut retries = 0;
    loop {
        let mut result = match mode {
            Mode::Shipped => shipped(engine, options, sample).await,
            Mode::Plain | Mode::Guided => direct(engine, options, mode == Mode::Guided, sample).await,
        };
        let rate_limited = result.message.as_deref() == Some(&ProviderError::RateLimited.to_string());
        if rate_limited && retries < RATE_LIMIT_RETRIES {
            retries += 1;
            tokio::time::sleep(Duration::from_secs(5 * retries as u64)).await;
            continue;
        }
        result.rate_limit_retries = retries;
        return result;
    }
}

/// The prompt and provider alone, as `Engine::run` builds and sends them, then the guard.
async fn direct(engine: &Engine, options: &Options, guided: bool, sample: &Sample) -> RunResult {
    let presets = prompts::builtin_presets();
    let preset = presets.iter().find(|p| p.id == sample.preset).expect("the corpus names built-in presets");
    let detected = lang::detect(&sample.text);
    let language = detected.as_ref().filter(|d| d.reliable && d.code != "eng").map(|d| d.name);
    let mut request = prompts::build(&prompts::PromptInput {
        text: &sample.text,
        preset,
        steer: None,
        style_notes: "",
        dialect: &options.dialect,
        words: &words::found_in(&sample.text, &sample.words),
        language,
    });
    request.guided = guided;
    let started = Instant::now();
    let mut first_token = None;
    let streamed = {
        let mut on_delta = |_: &str| {
            first_token.get_or_insert_with(|| started.elapsed().as_millis() as u64);
        };
        engine.providers.stream(&options.profile, &request, &mut on_delta).await
    };
    let mut result =
        RunResult { first_token_ms: first_token, total_ms: started.elapsed().as_millis() as u64, ..Default::default() };
    match streamed {
        Ok(raw) => {
            let cleaned = prompts::clean_output(&sample.text, &raw);
            let empty = cleaned.trim().is_empty() && !sample.text.trim().is_empty();
            let flagged = prompts::answers_instead_of_editing(&sample.text, &raw, &cleaned, sample.is_proofread());
            result.guard_flagged = Some(flagged);
            if empty || flagged {
                result.outcome = Outcome::Refused;
                result.message = Some(if empty { "no text" } else { "answered instead of editing" }.into());
            } else {
                result.outcome = Outcome::Edited;
                result.output = Some(cleaned.clone());
            }
            result.reply = Some(cleaned);
            result.raw = Some(raw);
        }
        Err(error) => {
            result.outcome = Outcome::Error;
            result.message = Some(error.to_string());
        }
    }
    result
}

/// The whole engine, as the app runs a review. The result is what applying the review gives:
/// the revision with any change that drops one of the author's terms left out.
async fn shipped(engine: &Engine, options: &Options, sample: &Sample) -> RunResult {
    let presets = prompts::builtin_presets();
    let preset = presets.iter().find(|p| p.id == sample.preset).expect("the corpus names built-in presets").clone();
    let job = Job {
        text: sample.text.clone(),
        preset,
        steer: None,
        profile: Some(options.profile.clone()),
        style_notes: String::new(),
        dialect: options.dialect.clone(),
        words: sample.words.clone(),
        code: Vec::new(),
        local_checks: true,
        builtin_ai: false,
    };
    let started = Instant::now();
    let mut last = None;
    engine
        .run(job, &mut |event| {
            if matches!(event, Event::Done(_) | Event::Failed { .. }) {
                last = Some(event);
            }
        })
        .await;
    let mut result = RunResult { total_ms: started.elapsed().as_millis() as u64, ..Default::default() };
    match last {
        Some(Event::Done(outcome)) => {
            result.outcome = Outcome::Edited;
            result.output = Some(outcome.kept_text());
            result.first_token_ms = outcome.first_token_ms;
            result.total_ms = outcome.total_ms;
        }
        Some(Event::Failed { message, .. }) => {
            let refused = message.contains("instead of editing") || message.contains("returned no text");
            result.outcome = if refused { Outcome::Refused } else { Outcome::Error };
            result.message = Some(message);
        }
        _ => {
            result.outcome = Outcome::Error;
            result.message = Some("the engine finished without a result".into());
        }
    }
    result
}
