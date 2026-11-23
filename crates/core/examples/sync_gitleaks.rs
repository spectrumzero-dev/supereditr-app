//! Rust half of scripts/sync-gitleaks.py. Uses the same regex engine as the app.

use regex::Regex;
use regex_syntax::hir::{Class, Hir, HirKind, Look};
use serde::Deserialize;
use std::io;

#[derive(Deserialize)]
struct UpstreamRule {
    id: String,
    regex: Option<String>,
}

// Upstream rules with formats already represented by the local named rows.
const COVERED: &[&str] = &[
    "age-secret-key",
    "anthropic-admin-api-key",
    "anthropic-api-key",
    "aws-access-token",
    "digitalocean-access-token",
    "digitalocean-pat",
    "digitalocean-refresh-token",
    "gcp-api-key",
    "github-app-token",
    "github-fine-grained-pat",
    "github-oauth",
    "github-pat",
    "github-refresh-token",
    "gitlab-pat",
    "gitlab-pat-routable",
    "huggingface-access-token",
    "jwt",
    "npm-access-token",
    "openai-api-key",
    "private-key",
    "pypi-upload-token",
    "sendgrid-api-token",
    "slack-bot-token",
    "slack-config-access-token",
    "slack-config-refresh-token",
    "slack-legacy-bot-token",
    "slack-legacy-token",
    "slack-legacy-workspace-token",
    "slack-user-token",
    "slack-webhook-url",
    "stripe-access-token",
];

// These formats identify public values or have a literal too broad to use by itself.
const AMBIGUOUS: &[&str] = &[
    "clickhouse-cloud-api-secret-key",
    "flutterwave-public-key",
    "gitlab-session-cookie",
    "gocardless-api-token",
    "jwt-base64",
    "lob-api-key",
    "lob-pub-api-key",
    "mailgun-pub-key",
    "mapbox-api-token",
    "microsoft-teams-webhook",
    "nuget-config-password",
    "octopus-deploy-api-key",
    "slack-app-token",
    "sonar-api-token",
    "square-access-token",
    "vault-service-token",
];

fn ascii_choices(class: &Class) -> Vec<u8> {
    b"aBcD1eF2gH3iJ4kL5mN6oP7qR8sT9uV0wXyYzZAECGIKMOQSUWbdfhjlnprtvx _-./:=+@\t\n"
        .iter()
        .copied()
        .filter(|&byte| match class {
            Class::Unicode(class) => class.iter().any(|r| r.start() <= char::from(byte) && char::from(byte) <= r.end()),
            Class::Bytes(class) => class.iter().any(|r| r.start() <= byte && byte <= r.end()),
        })
        .collect()
}

// HIR represents an ASCII case-insensitive literal as a two-member character class.
fn folded_ascii(class: &Class) -> Option<u8> {
    let ranges: Vec<_> = match class {
        Class::Bytes(class) => class.iter().map(|r| (r.start().into(), r.end().into())).collect(),
        Class::Unicode(class) => class.iter().map(|r| (r.start() as u32, r.end() as u32)).collect(),
    };
    let [(upper, upper_end), (lower, lower_end)] = ranges.as_slice() else { return None };
    let byte = u8::try_from(*lower).ok()?;
    (byte.is_ascii_lowercase()
        && lower == lower_end
        && upper == upper_end
        && *upper == u32::from(byte.to_ascii_uppercase()))
    .then_some(byte)
}

fn distinctive(prefix: &[u8]) -> bool {
    prefix.len() >= 4
        && prefix.iter().any(u8::is_ascii_alphabetic)
        && !(prefix.len() == 4 && prefix.iter().all(u8::is_ascii_hexdigit))
        && !prefix.iter().all(|byte| byte.eq_ignore_ascii_case(&prefix[0]))
}

fn has_distinctive_prefix(hir: &Hir, mut prefix: Vec<u8>) -> bool {
    match hir.kind() {
        HirKind::Literal(literal) => {
            prefix.extend(&*literal.0);
            distinctive(&prefix)
        }
        HirKind::Capture(capture) => has_distinctive_prefix(&capture.sub, prefix),
        HirKind::Alternation(parts) => parts.iter().all(|part| has_distinctive_prefix(part, prefix.clone())),
        HirKind::Concat(parts) => {
            for part in parts {
                match part.kind() {
                    HirKind::Look(Look::WordAscii) if prefix.is_empty() => {}
                    HirKind::Literal(literal) => prefix.extend(&*literal.0),
                    HirKind::Class(class) => {
                        let Some(byte) = folded_ascii(class) else { return distinctive(&prefix) };
                        prefix.push(byte);
                    }
                    _ => return distinctive(&prefix) || has_distinctive_prefix(part, prefix),
                }
            }
            distinctive(&prefix)
        }
        _ => distinctive(&prefix),
    }
}

fn is_pure_literal(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Empty | HirKind::Literal(_) | HirKind::Look(_) => true,
        HirKind::Class(class) => folded_ascii(class).is_some(),
        HirKind::Capture(capture) => is_pure_literal(&capture.sub),
        HirKind::Concat(parts) | HirKind::Alternation(parts) => parts.iter().all(is_pure_literal),
        HirKind::Repetition(repeat) => is_pure_literal(&repeat.sub),
    }
}

// Build one invented match from each syntax tree. Keep bytes separate in the generated fixture.
fn sample(hir: &Hir, bytes: &mut Vec<u8>) {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => {}
        HirKind::Literal(literal) => bytes.extend(&*literal.0),
        HirKind::Class(class) => {
            let byte = folded_ascii(class).unwrap_or_else(|| {
                let choices = ascii_choices(class);
                choices.get(bytes.len() % choices.len().max(1)).copied().unwrap_or(b' ')
            });
            bytes.push(byte);
        }
        HirKind::Repetition(repeat) => {
            for _ in 0..repeat.min {
                sample(&repeat.sub, bytes);
            }
        }
        HirKind::Capture(capture) => sample(&capture.sub, bytes),
        HirKind::Concat(parts) => parts.iter().for_each(|part| sample(part, bytes)),
        HirKind::Alternation(parts) => sample(&parts[0], bytes),
    }
}

fn name(id: &str) -> String {
    let words = id.replace("personnal", "personal").replace("-pat", "-personal-access-token");
    let mut words = words.split('-');
    let vendor = words.next().unwrap();
    let vendor = match vendor {
        "1password" => "1Password".into(),
        "aws" => "AWS".into(),
        "flyio" => "Fly.io".into(),
        "frameio" => "Frame.io".into(),
        "gitlab" => "GitLab".into(),
        "intra42" => "Intra42".into(),
        "npm" => "npm".into(),
        "openshift" => "OpenShift".into(),
        "planetscale" => "PlanetScale".into(),
        "rubygems" => "RubyGems".into(),
        "easypost" => "EasyPost".into(),
        "infracost" => "Infracost".into(),
        "sendinblue" => "Sendinblue".into(),
        "settlemint" => "SettleMint".into(),
        "clickhouse" => "ClickHouse".into(),
        _ => format!("{}{}", vendor[..1].to_ascii_uppercase(), &vendor[1..]),
    };
    let suffix = words
        .filter(|word| *word != "v2")
        .map(|word| match word {
            "api" => "API",
            "ca" => "CA",
            "cicd" => "CI/CD",
            "id" => "ID",
            "oauth" => "OAuth",
            "ptt" => "pipeline trigger token",
            "rrt" => "runner registration token",
            "scim" => "SCIM",
            "org" => "organization",
            "tf" => "Terraform",
            "pub" => "public",
            "teams" => "Teams",
            "bedrock" => "Bedrock",
            "amazon" => "Amazon",
            "relic" => "Relic",
            _ => word,
        })
        .collect::<Vec<_>>()
        .join(" ");
    let article = if matches!(vendor.as_str(), "AWS" | "npm") || vendor.starts_with(['A', 'E', 'I', 'O', 'U']) {
        "an"
    } else {
        "a"
    };
    if id == "aws-amazon-bedrock-api-key-long-lived" {
        return "a long-lived Amazon Bedrock API key".into();
    }
    format!("{article} {vendor} {suffix}")
}

fn adapt(id: &str, upstream: &str) -> String {
    // RE2 uses ASCII Perl classes and word boundaries, but negated delimiter
    // classes must accept non-ASCII separators while staying valid Rust string regexes.
    match id {
        "sentry-org-token" => upstream.replace(r"[^a-zA-Z0-9+/]", r"(?u:[^a-zA-Z0-9+/])"),
        "openshift-user-token" => upstream.replace(r"[^\w-]", r"(?u:[^a-zA-Z0-9_-])"),
        _ => upstream.to_owned(),
    }
}

fn delimit(pattern: &str) -> String {
    let start = if pattern.trim_start_matches("(?i)").starts_with(r"\b") { "" } else { r"\b" };
    let has_end = [r"\b", "|$)", r"|\z)", r"|$|\b)"].iter().any(|suffix| pattern.ends_with(suffix));
    // Many tokens can end in a hyphen or padding, where a trailing \b is insufficient.
    let end = if has_end { "" } else { r"(?:\z|(?u:[^a-zA-Z0-9_]))" };
    format!("(?-u:{start}(?:{pattern}){end})")
}

fn select(rule: &UpstreamRule) -> Result<(String, String), &'static str> {
    let pattern = rule.regex.as_deref().ok_or("path only")?;
    if COVERED.contains(&rule.id.as_str()) {
        return Err("covered by a local rule");
    }
    if pattern.contains("{0,50}?") {
        return Err("requires keyword context");
    }
    if AMBIGUOUS.contains(&rule.id.as_str()) {
        return Err("public value or ambiguous format");
    }

    let pattern = adapt(&rule.id, pattern);
    let hir = regex_syntax::parse(&format!("(?-u:{pattern})")).map_err(|_| "incompatible with Rust regex")?;
    if !has_distinctive_prefix(&hir, Vec::new()) {
        return Err("no distinctive leading literal");
    }
    if is_pure_literal(&hir) {
        return Err("literal with no variable value");
    }

    let mut bytes = Vec::new();
    sample(&hir, &mut bytes);
    let example = String::from_utf8(bytes).expect("expected an ASCII synthetic match");
    let pattern = delimit(&pattern);
    let regex = Regex::new(&pattern).map_err(|_| "incompatible with Rust regex")?;
    if !regex.is_match(&example) {
        return Err("could not synthesize a match");
    }
    Ok((pattern, example))
}

fn main() -> Result<(), Box<dyn std::error::Error>> {
    let tag = std::env::args().nth(1).expect("expected a Gitleaks tag");
    let rules: Vec<UpstreamRule> = serde_json::from_reader(io::stdin().lock())?;
    let mut included = Vec::new();
    let mut excluded = Vec::new();
    for rule in &rules {
        match select(rule) {
            Ok((pattern, example)) => included.push((&rule.id, pattern, example)),
            Err(reason) => excluded.push((&rule.id, reason)),
        }
    }

    println!("// Generated by scripts/sync-gitleaks.py from Gitleaks {tag} (MIT).");
    println!("// See GITLEAKS-LICENSE for the upstream copyright and license.");
    println!("// Imported {} of {} upstream rules. Excluded {}:", included.len(), rules.len(), excluded.len());
    for (id, reason) in excluded {
        println!("// {id}: {reason}");
    }
    println!("\nuse super::{{Rule, not_an_example}};\n\npub(super) const RULES: &[Rule] = &[");
    for (id, pattern, _) in &included {
        let name = name(id);
        println!(
            "Rule {{ name: {name:?}, pattern: {pattern:?}, accept: not_an_example, source: \"gitleaks {tag}: {id}\" }},"
        );
    }
    println!("];\n\n#[cfg(test)]\npub(super) const EXAMPLES: &[(&str, &str)] = &[");
    for (_, _, example) in &included {
        let (first, second) = example.split_at(example.len() / 2);
        println!("({first:?}, {second:?}),");
    }
    println!("];");
    Ok(())
}
