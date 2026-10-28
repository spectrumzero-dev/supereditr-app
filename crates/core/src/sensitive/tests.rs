//! Cases for the secret rules. Every key and token here is made up and assembled at run time from
//! pieces, so no complete secret-shaped string appears in the source for scanners (GitHub push
//! protection, gitleaks) to flag.

use super::*;
use std::collections::BTreeSet;
use std::time::{Duration, Instant};

const BASE62: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
const UPPER_DIGITS: &str = "ABCDEFGHIJKLMNOPQRSTUVWXYZ0123456789";
const HEX: &str = "0123456789abcdef";
const BECH32_UPPER: &str = "QPZRY9X8GF2TVDW0S3JN54KHCE6MUA7L";

/// `len` characters from `alphabet` in a scrambled order that is the same on every run.
fn scrambled(alphabet: &str, len: usize, seed: u64) -> String {
    let alphabet: Vec<char> = alphabet.chars().collect();
    let mut state = seed;
    (0..len)
        .map(|_| {
            state = state.wrapping_mul(6364136223846793005).wrapping_add(1442695040888963407);
            alphabet[(state >> 33) as usize % alphabet.len()]
        })
        .collect()
}

fn random(len: usize) -> String {
    scrambled(BASE62, len, len as u64)
}

/// Texts each rule must catch, with the rule's name. Each is set in ordinary words, as a pasted
/// secret would be.
fn caught() -> Vec<(&'static str, String)> {
    let pem = |kind: &str| format!("{}{kind} PRIVATE KEY-----\nMIIEow{}\n-----END", "-----BEGIN ", random(64));
    let cases = vec![
        ("a private key", pem("RSA")),
        ("a private key", pem("OPENSSH")),
        ("a private key", pem("EC")),
        ("a private key", pem("ENCRYPTED")),
        ("a private key", format!("{}PRIVATE KEY BLOCK-----", "-----BEGIN PGP ")),
        ("a private key", format!("{}3: ssh-ed25519", "PuTTY-User-Key-File-")),
        ("an age secret key", format!("{}1{}", "AGE-SECRET-KEY-", scrambled(BECH32_UPPER, 58, 1))),
        ("a GitHub token", format!("{}{}", "ghp_", random(36))),
        ("a GitHub token", format!("{}{}", "gho_", random(36))),
        ("a GitHub token", format!("{}{}_{}", "github_pat_", random(22), random(59))),
        ("a GitLab token", format!("{}{}", "glpat-", random(20))),
        ("an AWS access key", format!("{}{}", "AKIA", scrambled(UPPER_DIGITS, 16, 2))),
        ("a Slack token", format!("{}-123456789012-{}", "xoxb", random(24))),
        (
            "a Slack webhook",
            format!(
                "https://hooks.slack.com/{}/T{}/B{}/{}",
                "services",
                scrambled(UPPER_DIGITS, 8, 3),
                scrambled(UPPER_DIGITS, 10, 4),
                random(24)
            ),
        ),
        ("a Stripe key", format!("{}_{}", "sk_live", random(24))),
        ("a Stripe key", format!("{}_{}", "rk_test", random(24))),
        ("an Anthropic API key", format!("{}api03-{}", "sk-ant-", random(80))),
        ("an OpenAI API key", format!("{}{}", "sk-proj-", random(48))),
        ("an OpenAI API key", format!("{}{}", "sk-", random(48))),
        ("a Google API key", format!("{}{}", "AIza", random(35))),
        ("a Hugging Face token", format!("{}{}", "hf_", random(34))),
        ("an npm token", format!("{}{}", "npm_", random(36))),
        ("a PyPI token", format!("{}{}", "pypi-AgEIcHlwaS5vcmc", random(60))),
        ("a DigitalOcean token", format!("{}{}", "dop_v1_", scrambled(HEX, 64, 5))),
        ("a SendGrid key", format!("{}.{}.{}", "SG", random(22), random(43))),
        ("a signed access token (JWT)", format!("eyJhbGciOiJIUzI1NiJ9.eyJzdWIiOiIxMjM0NTY3ODkwIn0.{}", random(43))),
        ("a password", format!("DB_PASSWORD=hunter2-{}", random(8))),
        ("a secret setting", format!(r#"{{"client_secret": "{}"}}"#, random(32))),
        ("a secret setting", format!("aws_secret_access_key = {}", random(40))),
        ("a password", "Password: Summer2024!".to_string()),
        ("a secret setting", format!("export GH_TOKEN='{}9'", random(14))),
        ("a password in a link", "postgres://app:Pa55-word-9@db.internal:5432/prod".to_string()),
        // Punctuation that also appears in code is still part of a password.
        ("a password", "DB_PASSWORD=Summer$2024!".to_string()),
        // A setting named for a password: short values count.
        ("a password", "password=abc123".to_string()),
        ("a password", "password: hunt3r".to_string()),
        ("a password", format!("DB_PASSWD={}", "ab12")),
        ("a password", "password: 50%off-Sale".to_string()),
        ("a password", "password=p(ss)w0rd".to_string()),
        ("a password", "DB_PASSWORD=MyP4ssword(2024)".to_string()),
        ("a password", "password: Hunter2[x]".to_string()),
        ("a password in a link", "postgres://app:MyP4ssword(2024)@db.internal".to_string()),
        ("a secret setting", "secret = {k3y}[7]".to_string()),
        ("a password in a link", "postgres://app:Pa%24ssword2024@db.internal".to_string()),
        ("a password in a link", "redis://default:$ecret9@cache:6379".to_string()),
        ("a password in a link", "https://deploy:tr0ub4dor@git.example.org/repo.git".to_string()),
        ("a key or token", format!("Here’s my API key {} — don’t share it.", random(40))),
        ("a key or token", format!("the token was \"{}\".", random(32))),
    ];
    cases.into_iter().map(|(name, secret)| (name, format!("Can you look at this?\n{secret}\nThanks!"))).collect()
}

#[test]
fn catches_each_kind_of_secret() {
    for (name, text) in caught() {
        assert_eq!(find(&text).map(|f| f.name), Some(name), "{text}");
    }
}

#[test]
fn every_rule_has_a_case_that_it_catches() {
    let matched: BTreeSet<usize> = caught().iter().filter_map(|(_, text)| first_match(text)).map(|(_, i)| i).collect();
    for (index, rule) in RULES.iter().enumerate() {
        assert!(matched.contains(&index), "no case in caught() is found by the rule {:?}", rule.pattern);
    }
}

#[test]
fn every_rule_is_documented_and_named_for_the_message() {
    for rule in RULES {
        assert!(!rule.source.is_empty(), "{}", rule.pattern);
        assert!(rule.name.starts_with("a ") || rule.name.starts_with("an "), "{}", rule.name);
        Regex::new(rule.pattern).unwrap();
    }
}

#[test]
fn leaves_ordinary_writing_alone() {
    let texts = [
        // Prose about passwords, keys and tokens.
        "Reset your password: it needs at least 12 characters.".to_string(),
        "Pick a strong password and keep it secret.".into(),
        "Our API key rotation policy is every 90 days.".into(),
        "Secret Santa is on the 12th, and the token exchange is at 3.".into(),
        "The tokenizer splits text into tokens: about 4 characters each.".into(),
        "Password requirements: 8+ characters, one digit.".into(),
        // Settings that aren't secrets.
        "max_tokens: 4096".into(),
        "tokenizer = cl100k_base".into(),
        "token_count = 1024".into(),
        "credentials: include".into(),
        "token: v2beta".into(),
        "secret_version = 3a".into(),
        "Password: 8 characters".into(),
        "password: abc".into(),
        "PWD=/Users/sam/project2".into(),
        "pwd: ./build1".into(),
        // Code that names a secret without holding it.
        r#"api_key = os.environ["OPENAI_API_KEY"]"#.into(),
        "password: ${DB_PASSWORD}".into(),
        r#""apiKey": process.env.API_KEY_2"#.into(),
        "token = getToken(user1)".into(),
        "password = keyring.get_password(svc, user)".into(),
        r#"token = ENV["GH_TOKEN_2"]"#.into(),
        r#"secret = getpass("Passphrase 2: ")"#.into(),
        "SECRET_KEY = settings.SECRET_KEY".into(),
        "SECRET = OPENAI_API_KEY_2".into(),
        "password: $DB_PASSWORD_2".into(),
        "password=%DB_PASSWORD2%".into(),
        "token = {{ secrets.TOKEN_2 }}".into(),
        "api_key = config.openai_key_v2".into(),
        "token: keys[2]".into(),
        // Placeholders.
        "API_KEY=your-api-key-here".into(),
        "OPENAI_API_KEY=sk-...".into(),
        r#"password = "changeme123""#.into(),
        "token: <YOUR_TOKEN_1>".into(),
        "password=********".into(),
        "secret: xxxx-xxxx-1234".into(),
        "AWS_ACCESS_KEY_ID=AKIAIOSFODNN7EXAMPLE".into(),
        // Links without a password.
        "postgres://user:password@localhost:5432/db".into(),
        "https://admin:${PASS}@host.internal".into(),
        "git clone ssh://git@github.com:22/acme/app.git".into(),
        "mailto:someone@example.com and https://example.com/search?q=rust+regex&page=2".into(),
        // Hashes, IDs and other long strings.
        format!("Fixed in commit {} last week.", scrambled(HEX, 40, 6)),
        format!("The token hash is {}.", scrambled(HEX, 64, 7)),
        "Session token 3f2a1b4c-9d8e-4f7a-b6c5-d4e3f2a1b0c9 expired.".into(),
        "API key ID 550E8400-E29B-41D4-A716-446655440000".into(),
        "The token refreshTokenExpiryHandler2024 runs hourly.".into(),
        "My public key is 1A1zP1eP5QGefi2DMPTfTL5SLmv7DivfNa.".into(),
        "We use sk-learn-compatible-estimators-and-transformers everywhere.".into(),
        format!("integrity sha512-{}", random(86)),
        format!("A long random string with no keyword nearby: {}", random(40)),
        // Public halves and certificates.
        "-----BEGIN PUBLIC KEY-----".into(),
        "-----BEGIN CERTIFICATE-----".into(),
        "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIG4rT3vTt99Ox5kndS4HmgTrKBT8SKzhK4rhGkEVGlCI user@laptop".into(),
    ];
    for text in texts {
        assert_eq!(find(&text), None, "{text}");
    }
}

#[test]
fn says_which_line_and_never_quotes_the_secret() {
    let secret = format!("{}{}", "ghp_", random(36));
    let found = find(&format!("Hi team,\n\nThe deploy token is {secret}\n")).unwrap();
    assert_eq!(found, Finding { name: "a GitHub token", line: Some(3) });
    let message = found.message();
    assert!(message.contains("a GitHub token on line 3"), "{message}");
    assert!(!message.contains(&secret[4..12]), "{message}");

    let one_line = find(&format!("use {secret} for now")).unwrap();
    assert_eq!(one_line.line, None);
    assert!(!one_line.message().contains("line"), "{}", one_line.message());
}

#[test]
fn reports_the_earliest_secret() {
    let text = format!("password: {}\nand {}{}", random(12), "ghp_", random(36));
    assert_eq!(find(&text).map(|f| f.name), Some("a password"));
}

#[test]
fn a_long_selection_is_checked_quickly() {
    let paragraph = "The token budget and the secret plan: password resets, API keys, access keys. ";
    let text = paragraph.repeat(3_000); // about 240 KB, with a keyword every few words
    let started = Instant::now();
    assert_eq!(find(&text), None);
    assert!(started.elapsed() < Duration::from_secs(2), "took {:?}", started.elapsed());
}

#[test]
fn tells_references_from_passwords_by_their_whole_shape() {
    for reference in [
        "${DB_PASSWORD}",
        "$DB_PASSWORD_2",
        "%API_KEY2%",
        "<YOUR_TOKEN_1>",
        "{{",
        "os.getenv(",
        "ENV[",
        "keyring.get_password(svc2",
        "getToken(user1)",
        "process.env.API_KEY_2",
        "settings.SECRET_KEY2",
        "OPENAI_API_KEY_2",
        "/Users/sam/project2",
        "~/keys/id2",
    ] {
        assert!(refers_elsewhere(reference), "{reference} should count as a reference");
    }
    for password in [
        "Summer$2024!",
        "Pa%24ssword2024",
        "50%off-Sale",
        "p(ss)w0rd",
        "MyP4ssword(2024)",
        "Hunter2[x]",
        "$ecret9",
        "S3CR3T",
    ] {
        assert!(!refers_elsewhere(password), "{password} should count as a password");
    }
}
