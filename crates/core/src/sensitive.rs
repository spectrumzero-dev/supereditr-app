//! Secrets in a selection: API keys, access tokens, private keys, and passwords.
//!
//! A selection that holds one of the secrets these rules recognize is never reviewed. The rule-based
//! checker doesn't see it, Apple Intelligence doesn't see it, and nothing is sent to an AI provider.
//! SuperEditr says what it found and on which line, never the secret itself. Detection is by
//! pattern: a secret in a format with no rule, or an unlabelled random string, isn't caught.
//!
//! Every rule is a row in [`RULES`], so each one can be checked against the documentation of the
//! format it matches, where the format is documented. The rules are meant to stay quiet on ordinary
//! writing: commit hashes, UUIDs, checksums, code that only names a key (`os.environ["API_KEY"]`),
//! and placeholders (`your-api-key`). A check that blocks harmless text gets in the way, so a rule is
//! added only when it rarely matches anything else.
//!
//! To add a format, add a row here and two cases in `sensitive/tests.rs`: a text the rule must
//! catch, and, if the format could look like ordinary text, a text it must leave alone. The tests
//! fail when a rule has no case that catches it.

use regex::Regex;
use std::sync::LazyLock;

/// One kind of secret.
pub struct Rule {
    /// What the message calls it, with its article: "a GitHub token".
    pub name: &'static str,
    /// A regular expression in Rust's `regex` syntax, which runs in time linear in the text, so no
    /// selection can make the check slow. When the pattern has a capture group, `accept` sees the
    /// group; otherwise it sees the whole match.
    pub pattern: &'static str,
    /// A second look at the match, for formats that the pattern alone would confuse with ordinary
    /// text.
    pub accept: fn(&str) -> bool,
    /// Where the format is documented.
    pub source: &'static str,
}

pub const RULES: &[Rule] = &[
    // Known formats: a fixed prefix or header that ordinary writing doesn't contain.
    Rule {
        name: "a private key",
        pattern: r"-----BEGIN (?:[A-Z0-9]+ )*PRIVATE KEY(?: BLOCK)?-----",
        accept: not_an_example,
        source: "RFC 7468 (PEM: RSA, EC, PKCS #8, ENCRYPTED); OpenSSH PROTOCOL.key; RFC 4880 §6.2 (PGP)",
    },
    Rule {
        name: "a private key",
        pattern: r"PuTTY-User-Key-File-\d+:",
        accept: not_an_example,
        source: "PuTTY manual, appendix C: PPK file format",
    },
    Rule {
        name: "an age secret key",
        pattern: r"\bAGE-SECRET-KEY-1[02-9AC-HJ-NP-Z]{58}\b",
        accept: not_an_example,
        source: "age specification (C2SP age.md): X25519 identity, Bech32 uppercase",
    },
    Rule {
        name: "a GitHub token",
        pattern: r"\bgh[pousr]_[A-Za-z0-9]{36}\b",
        accept: not_an_example,
        source: "GitHub Docs: About authentication to GitHub, GitHub's token formats",
    },
    Rule {
        name: "a GitHub token",
        pattern: r"\bgithub_pat_[A-Za-z0-9_]{40,}",
        accept: not_an_example,
        source: "GitHub Docs: Managing your personal access tokens (fine-grained)",
    },
    Rule {
        name: "a GitLab token",
        pattern: r"\bglpat-[A-Za-z0-9_-]{20,}",
        accept: not_an_example,
        source: "GitLab Docs: Personal access tokens, token prefixes",
    },
    Rule {
        name: "an AWS access key",
        pattern: r"\b(?:AKIA|ASIA|ABIA|ACCA)[A-Z0-9]{16}\b",
        accept: not_an_example,
        source: "AWS IAM User Guide: IAM identifiers, unique ID prefixes",
    },
    Rule {
        name: "a Slack token",
        pattern: r"\bxox[abeoprs]-[A-Za-z0-9-]{10,}",
        accept: not_an_example,
        source: "Slack API: Token types",
    },
    Rule {
        name: "a Slack webhook",
        pattern: r"https://hooks\.slack\.com/(?:services|workflows|triggers)/[A-Za-z0-9/_-]{20,}",
        accept: not_an_example,
        source: "Slack API: Sending messages using incoming webhooks",
    },
    Rule {
        name: "a Stripe key",
        pattern: r"\b[rs]k_(?:live|test)_[A-Za-z0-9]{20,}",
        accept: not_an_example,
        source: "Stripe Docs: API keys",
    },
    Rule {
        name: "an Anthropic API key",
        pattern: r"\bsk-ant-[A-Za-z0-9_-]{32,}",
        accept: not_an_example,
        source: "Anthropic API docs: Authentication (keys start with sk-ant-)",
    },
    Rule {
        name: "an OpenAI API key",
        pattern: r"\bsk-(?:proj|svcacct|admin)-[A-Za-z0-9_-]{32,}",
        accept: not_an_example,
        source: "OpenAI API reference: Authentication (project and service-account keys)",
    },
    Rule {
        name: "an OpenAI API key",
        // The original format: no hyphens after the prefix, so `sk-learn-…` words don't match.
        pattern: r"\bsk-[A-Za-z0-9]{48}\b",
        accept: not_an_example,
        source: "OpenAI API reference: Authentication (user keys)",
    },
    Rule {
        name: "a Google API key",
        pattern: r"\bAIza[0-9A-Za-z_-]{35}",
        accept: not_an_example,
        source: "Google Cloud Docs: Manage API keys",
    },
    Rule {
        name: "a Hugging Face token",
        pattern: r"\bhf_[A-Za-z0-9]{34,}",
        accept: not_an_example,
        source: "Hugging Face Hub docs: User access tokens",
    },
    Rule {
        name: "an npm token",
        pattern: r"\bnpm_[A-Za-z0-9]{36}\b",
        accept: not_an_example,
        source: "npm Docs: About access tokens",
    },
    Rule {
        name: "a PyPI token",
        // Every PyPI token is a macaroon whose first bytes encode "pypi.org".
        pattern: r"\bpypi-AgEIcHlwaS5vcmc[A-Za-z0-9_-]{50,}",
        accept: not_an_example,
        source: "PyPI help: API tokens",
    },
    Rule {
        name: "a DigitalOcean token",
        pattern: r"\bdo[opr]_v1_[a-f0-9]{64}\b",
        accept: not_an_example,
        source: "DigitalOcean Docs: Personal access tokens (dop_v1_, doo_v1_, dor_v1_)",
    },
    Rule {
        name: "a SendGrid key",
        pattern: r"\bSG\.[A-Za-z0-9_-]{22}\.[A-Za-z0-9_-]{43}",
        accept: not_an_example,
        source: "Twilio SendGrid Docs: API keys",
    },
    Rule {
        name: "a signed access token (JWT)",
        // A JSON header and payload both start `{"`, which is `eyJ` in base64url.
        pattern: r"\beyJ[A-Za-z0-9_-]{8,}\.eyJ[A-Za-z0-9_-]{8,}\.[A-Za-z0-9_-]{8,}",
        accept: not_an_example,
        source: "RFC 7519 (JWT) and RFC 7515 (JWS compact serialization)",
    },
    // A value given to a setting whose name says it's secret: `password = …`, `API_KEY: …`,
    // `"client_secret": "…"`. The name may carry a prefix (`DB_PASSWORD`, `aws_secret_access_key`)
    // but nothing after it, so `tokenizer = …` and `token_count: …` don't count. A setting named
    // for a password holds nothing else, so its value needs only 4 characters (`password=abc123`);
    // other names need 8, because short values there are often IDs or versions (`token: v2`).
    Rule {
        name: "a password",
        pattern: r#"(?i)[a-z0-9_.-]*?(?:password|passwd|pwd|passphrase)s?["']?[ \t]*[:=][ \t]*["'`]?([^\s"'`,;]+)"#,
        accept: password_value,
        source: "settings files, .env files, and code",
    },
    Rule {
        name: "a secret setting",
        pattern: r#"(?i)[a-z0-9_.-]*?(?:secret|token|api[_-]?key|apikey|access[_-]?key|private[_-]?key|credentials?)s?["']?[ \t]*[:=][ \t]*["'`]?([^\s"'`,;]+)"#,
        accept: secret_value,
        source: "settings files, .env files, and code",
    },
    // A password inside a link: `postgres://app:hunter2@db.internal/prod`.
    Rule {
        name: "a password in a link",
        pattern: r"\b[a-z][a-z0-9+.-]*://[^\s:/?#@]+:([^\s/?#@]+)@",
        accept: link_password,
        source: "RFC 3986 §3.2.1 (user information in a URI)",
    },
    // A random-looking string a few words after "token", "secret", "API key" and the like: "my
    // token is …". Strict, so prose about tokens, hashes and identifiers isn't caught.
    Rule {
        name: "a key or token",
        pattern: r"(?i)\b(?:password|passphrase|secret|token|(?:api|access|secret|private)[ _-]?key)s?\b([^\n]{1,60})",
        accept: has_random_string,
        source: "credentials shared in messages and notes",
    },
];

/// The first secret found in a text.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Finding {
    /// The rule's name, as in "a GitHub token".
    pub name: &'static str,
    /// Which line it starts on, counting from 1, or `None` for a one-line text.
    pub line: Option<usize>,
}

impl Finding {
    /// What the panel says. It never quotes the secret.
    pub fn message(&self) -> String {
        let place = self.line.map(|n| format!(" on line {n}")).unwrap_or_default();
        format!(
            "This text has what looks like {}{place}, so SuperEditr didn’t check it or send it anywhere.",
            self.name
        )
    }
}

/// The first secret in `text`, or `None` when it has none.
pub fn find(text: &str) -> Option<Finding> {
    let (start, rule) = first_match(text)?;
    let line = text.contains('\n').then(|| text[..start].matches('\n').count() + 1);
    Some(Finding { name: RULES[rule].name, line })
}

static COMPILED: LazyLock<Vec<Regex>> =
    LazyLock::new(|| RULES.iter().map(|rule| Regex::new(rule.pattern).expect(rule.pattern)).collect());

/// Where the first accepted match starts, and the index of its rule in [`RULES`]. On a line that
/// several rules match, the earlier rule wins, so a named format beats the general rules below it.
fn first_match(text: &str) -> Option<(usize, usize)> {
    let line = |start: usize| text[..start].matches('\n').count();
    COMPILED
        .iter()
        .zip(RULES)
        .enumerate()
        .filter_map(|(index, (regex, rule))| {
            regex
                .captures_iter(text)
                .find(|found| (rule.accept)(found.get(1).or(found.get(0)).map_or("", |m| m.as_str())))
                .and_then(|found| found.get(0))
                .map(|whole| (whole.start(), index))
        })
        .min_by_key(|&(start, index)| (line(start), index))
}

/// Documentation shows made-up keys that say so (`AKIAIOSFODNN7EXAMPLE`); a real key never spells
/// out "example".
fn not_an_example(found: &str) -> bool {
    !found.to_ascii_lowercase().contains("example")
}

/// A password setting's value: like [`secret_value`], but 4 characters are enough.
fn password_value(value: &str) -> bool {
    setting_value(value, 4)
}

/// A setting's value that is likely a real secret: 8 or more characters with both letters and
/// digits, and not a placeholder or a piece of code that only refers to the secret.
fn secret_value(value: &str) -> bool {
    setting_value(value, 8)
}

fn setting_value(value: &str, shortest: usize) -> bool {
    (shortest..=512).contains(&value.len())
        && value.chars().any(|c| c.is_ascii_alphabetic())
        && value.chars().any(|c| c.is_ascii_digit())
        && !refers_elsewhere(value)
        && !is_placeholder(value)
}

/// The password part of `scheme://user:password@host`, unless it's a placeholder.
fn link_password(value: &str) -> bool {
    const STAND_INS: [&str; 6] = ["password", "pass", "passwd", "pwd", "pw", "secret"];
    !STAND_INS.contains(&value.to_ascii_lowercase().as_str())
        && !refers_elsewhere(value)
        && !is_placeholder(value)
        && !value.chars().all(|c| c == '*' || c == 'x' || c == 'X')
}

/// Code that names where a secret lives instead of holding it. The value must have one of these
/// shapes as a whole: passwords may contain `$`, `%`, brackets and dots too (`Summer$2024!`,
/// `Pa%24s`, `MyP4ssword(2024)`), so a single character proves nothing, and a call counts only
/// when it reads like code that fetches a secret.
fn refers_elsewhere(value: &str) -> bool {
    static REFERENCE: LazyLock<Regex> = LazyLock::new(|| {
        Regex::new(
            r"(?x)^(?:
                \$\{.*\}                                # ${DB_PASSWORD}
              | \$[A-Z_][A-Z0-9_]*                      # $DB_PASSWORD
              | %[A-Za-z_][A-Za-z0-9_]*%                # %API_KEY%
              | <.*>                                    # <token>
              | \{\{.*                                  # {{ secrets.TOKEN }}
              | [A-Za-z_][A-Za-z0-9_.]*[(\[]            # cut off at a quote: os.getenv(  ENV[
              | [A-Za-z_]+(?:\.[A-Za-z_]+)+[(\[].*      # a dotted path: keyring.get_password(svc
              | (?:get|read|load|fetch|lookup|resolve|decrypt)[A-Z_][A-Za-z_]*\(.*  # getToken(user1)
              | (?:process\.env|os\.environ|env|secrets|vars|settings|config)\..*
              | [A-Z][A-Z0-9]*(?:_[A-Z0-9]+)+           # OPENAI_API_KEY_2
              | (?:/|\./|~/).*                          # a path: PWD=/Users/sam/app2
            )$",
        )
        .unwrap()
    });
    REFERENCE.is_match(value)
}

/// Text written in place of a real secret: `your-api-key`, `example123`, `xxxx1234`.
fn is_placeholder(value: &str) -> bool {
    const WORDS: [&str; 10] =
        ["your", "example", "placeholder", "redacted", "changeme", "dummy", "sample", "xxxx", "****", "..."];
    let lower = value.to_ascii_lowercase();
    WORDS.iter().any(|word| lower.contains(word))
}

/// Whether the words after a keyword include a string that looks machine-generated.
fn has_random_string(window: &str) -> bool {
    window.split_whitespace().map(|word| word.trim_matches(|c: char| "\"'`.,;:!?()[]<>".contains(c))).any(looks_random)
}

/// A string of 24 or more characters from the alphabets keys use (letters, digits, `+/=_-`), with
/// upper case, lower case and digits mixed together: letters and digits alternate at least four
/// times. That rules out words, names, camelCase identifiers (`refreshTokenHandler2024`),
/// lowercase hex (commit hashes, SHA-256 checksums) and UUIDs.
fn looks_random(word: &str) -> bool {
    if word.len() < 24 || !word.chars().all(|c| c.is_ascii_alphanumeric() || "+/=_-".contains(c)) {
        return false;
    }
    let (upper, lower, digit) = (
        word.chars().any(|c| c.is_ascii_uppercase()),
        word.chars().any(|c| c.is_ascii_lowercase()),
        word.chars().any(|c| c.is_ascii_digit()),
    );
    let alphanumeric: Vec<bool> =
        word.chars().filter(|c| c.is_ascii_alphanumeric()).map(|c| c.is_ascii_digit()).collect();
    let switches = alphanumeric.windows(2).filter(|pair| pair[0] != pair[1]).count();
    upper && lower && digit && switches >= 4
}

#[cfg(test)]
mod tests;
