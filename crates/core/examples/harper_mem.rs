// Measures resident memory of Harper's components: `cargo run --release --example harper_mem`.
use harper_core::linting::{LintGroup, Linter};
use harper_core::spell::FstDictionary;
use harper_core::{Dialect, Document};

fn rss() -> String {
    let out =
        std::process::Command::new("ps").args(["-o", "rss=", "-p", &std::process::id().to_string()]).output().unwrap();
    format!("{} MB", String::from_utf8_lossy(&out.stdout).trim().parse::<u64>().unwrap_or(0) / 1024)
}

fn main() {
    println!("start: {}", rss());
    let dict = FstDictionary::curated();
    println!("dictionary: {}", rss());
    let mut group = LintGroup::new_curated(dict, Dialect::American);
    println!("lint group: {}", rss());
    let doc = Document::new_plain_english_curated("This is an test of teh system.");
    let lints = group.lint(&doc);
    println!("after lint ({} lints): {}", lints.len(), rss());
}
