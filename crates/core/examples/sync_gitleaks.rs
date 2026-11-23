//! Rust half of scripts/sync-gitleaks.py. Uses the same regex engine as the app.

use regex::Regex;
use regex_syntax::hir::{Class, Hir, HirKind};
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
    "slack-legacy-bot-token",
    "slack-legacy-token",
    "slack-legacy-workspace-token",
    "slack-user-token",
    "slack-webhook-url",
    "stripe-access-token",
];

// These formats identify public values or have a literal too broad to use by itself.
const AMBIGUOUS: &[&str] = &[
    "flutterwave-public-key",
    "gocardless-api-token",
    "jwt-base64",
    "lob-api-key",
    "lob-pub-api-key",
    "mailgun-pub-key",
    "mapbox-api-token",
    "nuget-config-password",
    "octopus-deploy-api-key",
    "sonar-api-token",
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

fn fixed_ascii(hir: &Hir) -> Option<Vec<u8>> {
    match hir.kind() {
        HirKind::Literal(literal) => Some(literal.0.to_vec()),
        // The parser represents a case-insensitive literal as a class of case variants.
        HirKind::Class(class) => {
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
            .then_some(vec![byte])
        }
        _ => None,
    }
}

fn distinctive(literal: &[u8]) -> bool {
    literal.len() >= 4 && literal.iter().any(u8::is_ascii_alphabetic)
        || literal.len() == 3
            && literal.iter().any(u8::is_ascii_alphabetic)
            && literal.iter().any(u8::is_ascii_punctuation)
}

fn has_literal(hir: &Hir) -> bool {
    match hir.kind() {
        HirKind::Concat(parts) => {
            let mut literal = Vec::new();
            for part in parts {
                if let Some(bytes) = fixed_ascii(part) {
                    literal.extend(bytes);
                } else {
                    if distinctive(&literal) || has_literal(part) {
                        return true;
                    }
                    literal.clear();
                }
            }
            distinctive(&literal)
        }
        HirKind::Alternation(parts) => parts.iter().all(has_literal),
        HirKind::Capture(capture) => has_literal(&capture.sub),
        HirKind::Repetition(repeat) if repeat.min > 0 => has_literal(&repeat.sub),
        _ => fixed_ascii(hir).is_some_and(|literal| distinctive(&literal)),
    }
}

// Assignment rules use this shared upstream context. Inspect their first captured value for
// a literal so a vendor name in a setting identifier alone cannot qualify the rule.
fn first_capture(hir: &Hir) -> Option<&Hir> {
    if let HirKind::Capture(capture) = hir.kind() {
        return Some(&capture.sub);
    }
    hir.kind().subs().iter().find_map(first_capture)
}

// Build one invented match from each syntax tree. Keep bytes separate in the generated fixture.
fn sample(hir: &Hir, bytes: &mut Vec<u8>) {
    match hir.kind() {
        HirKind::Empty | HirKind::Look(_) => {}
        HirKind::Literal(literal) => bytes.extend(&*literal.0),
        HirKind::Class(class) => {
            let choices = ascii_choices(class);
            bytes.push(choices.get(bytes.len() % choices.len().max(1)).copied().unwrap_or(b' '));
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
    let pattern = match id {
        // A printable delimiter or the end of the selection. Use Unicode for negated classes
        // only, so they can match a non-ASCII character without making the regex byte-oriented.
        "sentry-org-token" => upstream.replace(r"[^a-zA-Z0-9+/]", r"(?u:[^a-zA-Z0-9+/])"),
        "openshift-user-token" => upstream.replace(r"[^\w-]", r"(?u:[^a-zA-Z0-9_-])"),
        "flutterwave-encryption-key" | "gitlab-runner-authentication-token" => format!(r"{upstream}\b"),
        "slack-config-access-token" => upstream.replace("xoxe.xox", r"xoxe\.xox"),
        // These formats are bounded by the token, so a longer identifier cannot trigger them.
        "linear-api-key"
        | "doppler-api-token"
        | "shopify-access-token"
        | "shopify-custom-access-token"
        | "shopify-private-app-access-token"
        | "shopify-shared-secret" => {
            format!(r"\b{upstream}\b")
        }
        _ => upstream.to_owned(),
    };
    // RE2's Perl classes and word boundaries are ASCII.
    format!("(?-u:{pattern})")
}

fn select(rule: &UpstreamRule) -> Result<(String, String), &'static str> {
    let pattern = rule.regex.as_deref().ok_or("path only")?;
    if COVERED.contains(&rule.id.as_str()) {
        return Err("covered by a local rule");
    }
    if AMBIGUOUS.contains(&rule.id.as_str()) {
        return Err("public value or ambiguous format");
    }

    let pattern = adapt(&rule.id, pattern);
    let regex = Regex::new(&pattern).map_err(|_| "incompatible with Rust regex")?;
    let hir = regex_syntax::parse(&pattern).map_err(|_| "incompatible with Rust regex")?;
    let value = if pattern.contains("{0,50}?") {
        // Mixed generic/distinctive alternations are excluded as a whole.
        if matches!(hir.kind(), HirKind::Alternation(_)) {
            return Err("includes an unprefixed alternative");
        }
        first_capture(&hir).unwrap_or(&hir)
    } else {
        &hir
    };
    if !has_literal(value) {
        return Err("no distinctive literal in the value");
    }

    let mut bytes = Vec::new();
    sample(&hir, &mut bytes);
    let example = String::from_utf8(bytes).expect("expected an ASCII synthetic match");
    if !regex.is_match(&example) {
        return Err("could not synthesize a match");
    }
    // A pattern made entirely of literals would also block prose mentioning its prefix.
    if !value.kind().subs().iter().any(|sub| fixed_ascii(sub).is_none()) && fixed_ascii(value).is_some() {
        return Err("literal with no variable value");
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
