//! Scores results files: metrics per provider, model, mode and OS version, broken down by
//! category and task, and regressions against a stored baseline summary.

use crate::metrics::{self, quantile};
use crate::run::{Outcome, Record};
use serde::{Deserialize, Serialize};
use std::collections::{BTreeMap, BTreeSet};
use std::fmt::Write;

/// A count out of the runs it applies to.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
pub struct Ratio {
    pub n: usize,
    pub of: usize,
}

impl Ratio {
    fn add(&mut self, hit: bool) {
        self.of += 1;
        self.n += usize::from(hit);
    }

    pub fn rate(&self) -> Option<f64> {
        (self.of > 0).then(|| self.n as f64 / self.of as f64)
    }
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Metrics {
    pub runs: usize,
    pub samples: usize,
    /// The provider failed.
    pub errors: Ratio,
    /// The guard (or the engine) turned the reply away, so the user saw an error.
    pub refused: Ratio,
    /// The model's reply was not an edit (answered, obeyed or stopped short), before the guard.
    /// Known only in plain and guided modes, where the reply is recorded.
    pub not_edit: Ratio,
    /// Of those, the guard caught them.
    pub caught: Ratio,
    /// A reply that was not an edit reached the user: of all runs.
    pub escaped: Ratio,
    /// The guard turned away a real edit: of all edits.
    pub false_alarms: Ratio,
    /// The result matched an acceptable one exactly: of runs with an expected result.
    pub exact: Ratio,
    /// Mean word distance from the closest acceptable result, as a share of its words.
    pub distance: Option<f64>,
    /// The result was the text unchanged although it needed a fix.
    pub unchanged: Ratio,
    /// Already-correct text was changed.
    pub changed_correct: Ratio,
    /// Any formatting damage, then each kind: of delivered results.
    pub damage: Ratio,
    pub trailing_space: Ratio,
    pub curly_quotes: Ratio,
    pub lines_merged: Ratio,
    pub lists_lost: Ratio,
    pub code_changed: Ratio,
    /// A protected token or `must_keep` term was lost: of delivered results.
    pub content_lost: Ratio,
    pub first_token_p50: Option<u64>,
    pub first_token_p90: Option<u64>,
    pub total_p50: Option<u64>,
    pub total_p90: Option<u64>,
    /// Samples whose runs gave more than one distinct result: of samples run more than once.
    pub unstable: Ratio,
    /// Samples whose runs disagreed on exact match.
    pub exact_flips: Ratio,
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct GroupKey {
    pub provider: String,
    pub model: String,
    pub model_identity: String,
    pub mode: String,
    pub os: String,
    pub os_version: String,
}

impl GroupKey {
    fn label(&self) -> String {
        let os = if self.os_version.is_empty() { self.os.clone() } else { format!("{} {}", self.os, self.os_version) };
        format!("{} {} · {} · {}", self.provider, self.model, self.mode, os)
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Group {
    pub key: GroupKey,
    pub run_ids: Vec<String>,
    pub app_commits: Vec<String>,
    pub overall: Metrics,
    /// Samples where a reply that was not an edit reached the user, and in how many runs.
    #[serde(default)]
    pub escaped_samples: BTreeMap<String, usize>,
    pub categories: BTreeMap<String, Metrics>,
    pub tasks: BTreeMap<String, Metrics>,
    /// Every sample run, by id, with a fingerprint of its definition, so a comparison can tell
    /// when a run covered fewer samples than its baseline or a sample changed.
    #[serde(default)]
    pub samples: BTreeMap<String, String>,
}

/// FNV-1a over the sample's JSON: stable across runs and platforms.
fn fingerprint(sample: &crate::corpus::Sample) -> String {
    let json = serde_json::to_string(sample).expect("a sample serializes");
    let hash = json.bytes().fold(0xcbf2_9ce4_8422_2325_u64, |h, b| (h ^ u64::from(b)).wrapping_mul(0x0100_0000_01b3));
    format!("{hash:016x}")
}

/// What `score --summary` writes and `--baseline` reads.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Summary {
    pub groups: Vec<Group>,
}

pub fn summarize(records: &[Record]) -> Summary {
    let mut grouped: BTreeMap<GroupKey, Vec<&Record>> = BTreeMap::new();
    for record in records {
        let key = GroupKey {
            provider: record.provider.clone(),
            model: record.model.clone(),
            model_identity: record.model_identity.clone(),
            mode: record.mode.clone(),
            os: record.env.os.clone(),
            os_version: record.env.os_version.clone(),
        };
        grouped.entry(key).or_default().push(record);
    }
    let groups = grouped
        .into_iter()
        .map(|(key, records)| {
            let by = |field: fn(&Record) -> &str| {
                let mut map: BTreeMap<String, Vec<&Record>> = BTreeMap::new();
                for record in &records {
                    map.entry(field(record).to_string()).or_default().push(record);
                }
                map.into_iter().map(|(name, records)| (name, measure(&records))).collect()
            };
            Group {
                run_ids: records.iter().map(|r| r.run_id.clone()).collect::<BTreeSet<_>>().into_iter().collect(),
                app_commits: records
                    .iter()
                    .map(|r| r.env.app_commit.clone())
                    .filter(|c| !c.is_empty())
                    .collect::<BTreeSet<_>>()
                    .into_iter()
                    .collect(),
                overall: measure(&records),
                escaped_samples: escaped_samples(&records),
                samples: records.iter().map(|r| (r.sample.id.clone(), fingerprint(&r.sample))).collect(),
                categories: by(|r| &r.sample.category),
                tasks: by(|r| &r.sample.preset),
                key,
            }
        })
        .collect();
    Summary { groups }
}

fn escaped_samples(records: &[&Record]) -> BTreeMap<String, usize> {
    let mut found = BTreeMap::new();
    for record in records {
        let sample = &record.sample;
        let output = record.output.as_deref().filter(|_| record.outcome == Outcome::Edited);
        if output.is_some_and(|o| metrics::not_an_edit(&sample.preset, &sample.text, o)) {
            *found.entry(sample.id.clone()).or_insert(0) += 1;
        }
    }
    found
}

fn measure(records: &[&Record]) -> Metrics {
    let mut m = Metrics { runs: records.len(), ..Default::default() };
    let mut distances = Vec::new();
    let mut first_tokens = Vec::new();
    let mut totals = Vec::new();
    let mut per_sample: BTreeMap<&str, Vec<&Record>> = BTreeMap::new();
    for record in records {
        per_sample.entry(&record.sample.id).or_default().push(record);
        let sample = &record.sample;
        m.errors.add(record.outcome == Outcome::Error);
        if record.outcome == Outcome::Error {
            continue;
        }
        m.refused.add(record.outcome == Outcome::Refused);
        if let Some(reply) = &record.reply {
            let not_edit = metrics::not_an_edit(&sample.preset, &sample.text, reply);
            m.not_edit.add(not_edit);
            if not_edit {
                m.caught.add(record.outcome == Outcome::Refused);
            } else {
                m.false_alarms.add(record.outcome == Outcome::Refused);
            }
        }
        let output = record.output.as_deref().filter(|_| record.outcome == Outcome::Edited);
        m.escaped.add(output.is_some_and(|o| metrics::not_an_edit(&sample.preset, &sample.text, o)));
        if !sample.expected.is_empty() {
            m.exact.add(output.is_some_and(|o| sample.expected.iter().any(|e| e == o)));
            if let Some(d) = output.and_then(|o| metrics::distance_from_expected(o, &sample.expected)) {
                distances.push(d);
            }
        }
        if sample.needs_change() {
            m.unchanged.add(output == Some(sample.text.as_str()));
        }
        if sample.category == "correct" {
            m.changed_correct.add(output.is_some_and(|o| o != sample.text));
        }
        if let Some(output) = output {
            let damage = metrics::damage(&sample.preset, &sample.text, output);
            m.damage.add(damage.any());
            m.trailing_space.add(damage.trailing_space);
            m.curly_quotes.add(damage.curly_quotes);
            m.lines_merged.add(damage.lines_merged);
            m.lists_lost.add(damage.lists_lost);
            m.code_changed.add(damage.code_changed);
            m.content_lost.add(!metrics::content_lost(&sample.text, output, &sample.must_keep).is_empty());
            totals.push(record.total_ms);
            first_tokens.extend(record.first_token_ms);
        }
    }
    m.samples = per_sample.len();
    for runs in per_sample.values().filter(|runs| runs.len() > 1) {
        let distinct: BTreeSet<Option<&str>> = runs.iter().map(|r| r.output.as_deref()).collect();
        m.unstable.add(distinct.len() > 1);
        if !runs[0].sample.expected.is_empty() {
            let exact: BTreeSet<bool> =
                runs.iter().map(|r| r.output.as_ref().is_some_and(|o| r.sample.expected.contains(o))).collect();
            m.exact_flips.add(exact.len() > 1);
        }
    }
    m.distance = (!distances.is_empty()).then(|| distances.iter().sum::<f64>() / distances.len() as f64);
    m.first_token_p50 = quantile(&first_tokens, 0.5);
    m.first_token_p90 = quantile(&first_tokens, 0.9);
    m.total_p50 = quantile(&totals, 0.5);
    m.total_p90 = quantile(&totals, 0.9);
    m
}

/// How far a rate may move against the baseline before the report flags it.
pub struct Threshold {
    pub metric: &'static str,
    /// Higher is better (exact matches); otherwise lower is better.
    pub higher_is_better: bool,
    /// Allowed change in the rate, in absolute terms (0.10 is ten percentage points).
    pub allowed: f64,
    pub get: fn(&Metrics) -> Ratio,
}

pub const THRESHOLDS: &[Threshold] = &[
    Threshold { metric: "exact", higher_is_better: true, allowed: 0.10, get: |m| m.exact },
    Threshold { metric: "escaped", higher_is_better: false, allowed: 0.03, get: |m| m.escaped },
    Threshold { metric: "not_edit", higher_is_better: false, allowed: 0.05, get: |m| m.not_edit },
    Threshold { metric: "refused", higher_is_better: false, allowed: 0.05, get: |m| m.refused },
    Threshold { metric: "false_alarms", higher_is_better: false, allowed: 0.03, get: |m| m.false_alarms },
    Threshold { metric: "unchanged", higher_is_better: false, allowed: 0.10, get: |m| m.unchanged },
    Threshold { metric: "changed_correct", higher_is_better: false, allowed: 0.10, get: |m| m.changed_correct },
    Threshold { metric: "damage", higher_is_better: false, allowed: 0.05, get: |m| m.damage },
    Threshold { metric: "content_lost", higher_is_better: false, allowed: 0.05, get: |m| m.content_lost },
    Threshold { metric: "errors", higher_is_better: false, allowed: 0.05, get: |m| m.errors },
];

/// A category needs this many runs on both sides before its rates are compared; fewer is noise.
const MIN_RUNS: usize = 9;
/// Median total latency may grow by half, and by at least this much, before it's flagged.
const LATENCY_FLOOR_MS: u64 = 250;

#[derive(Debug, Clone, PartialEq)]
pub struct Regression {
    pub group: String,
    pub scope: String,
    pub metric: String,
    pub baseline: String,
    pub current: String,
}

/// The baseline group a current one is compared with: same provider and mode, and the same model
/// or, when the baseline has exactly one model for that provider and mode, that one, so a new cloud
/// model is compared with the one it replaces. Among those, the same OS version if the baseline has
/// it, otherwise the latest version of the same OS: a new OS version is compared with the old one,
/// which is the point of re-running after an update.
fn baseline_for<'a>(baseline: &'a Summary, key: &GroupKey) -> Option<&'a Group> {
    // Apple's model ships with the OS; a cloud or local server's doesn't depend on the client's OS.
    let same_os = |g: &Group| key.provider != "apple" || g.key.os == key.os;
    let same_mode = |g: &&Group| g.key.provider == key.provider && g.key.mode == key.mode && same_os(g);
    let mut candidates: Vec<&Group> = baseline.groups.iter().filter(same_mode).collect();
    if candidates.iter().any(|g| g.key.model == key.model) {
        candidates.retain(|g| g.key.model == key.model);
    } else if candidates.iter().map(|g| &g.key.model).collect::<BTreeSet<_>>().len() != 1 {
        return None;
    }
    candidates
        .iter()
        .find(|g| g.key.os_version == key.os_version)
        .or_else(|| candidates.iter().max_by(|a, b| version(&a.key.os_version).cmp(&version(&b.key.os_version))))
        .copied()
}

/// An OS version as numbers, so 27.10 sorts after 27.2.
fn version(text: &str) -> Vec<u64> {
    text.split(|c: char| !c.is_ascii_digit()).filter_map(|p| p.parse().ok()).collect()
}

/// Current groups that can't be compared with the baseline, and why: no baseline group, or a
/// baseline sample the run left out or that has changed since. Rates over different samples
/// would hide exactly the regressions the comparison is for.
pub fn unmatched(current: &Summary, baseline: &Summary) -> Vec<String> {
    let mut found = Vec::new();
    if current.groups.is_empty() {
        found.push("no runs to compare".into());
    }
    // Every mode the baseline has for a provider (and, for Apple, an OS) the run covers must be
    // in the run too: leaving out shipped mode would leave the shipped behavior unchecked.
    for base in &baseline.groups {
        let same_os = |g: &Group| base.key.provider != "apple" || g.key.os == base.key.os;
        let covered = |g: &&Group| g.key.provider == base.key.provider && same_os(g);
        if current.groups.iter().any(|g| covered(&g))
            && !current.groups.iter().filter(covered).any(|g| g.key.mode == base.key.mode)
        {
            let label = format!("{} {} mode", base.key.provider, base.key.mode);
            if !found.contains(&format!("{label}: in the baseline but not run")) {
                found.push(format!("{label}: in the baseline but not run"));
            }
        }
    }
    for group in &current.groups {
        let Some(base) = baseline_for(baseline, &group.key) else {
            found.push(format!("{}: no baseline", group.key.label()));
            continue;
        };
        // The same samples on both sides: a missing one could hide a failure, and an added one
        // could dilute it.
        let ids: BTreeSet<&String> = base.samples.keys().chain(group.samples.keys()).collect();
        let differ: Vec<&str> = if base.samples.is_empty() {
            Vec::new()
        } else {
            ids.into_iter().filter(|id| group.samples.get(*id) != base.samples.get(*id)).map(String::as_str).collect()
        };
        if !differ.is_empty() {
            let shown: Vec<&str> = differ.iter().take(5).copied().collect();
            let more = if differ.len() > 5 { format!(" and {} more", differ.len() - 5) } else { String::new() };
            found.push(format!(
                "{}: {} samples missing, added or changed since the baseline ({}{more})",
                group.key.label(),
                differ.len(),
                shown.join(", ")
            ));
        }
    }
    found
}

pub fn regressions(current: &Summary, baseline: &Summary) -> Vec<Regression> {
    let mut found = Vec::new();
    for group in &current.groups {
        let Some(base) = baseline_for(baseline, &group.key) else { continue };
        let label = format!("{} (baseline: {})", group.key.label(), base.key.label());
        let mut scopes: Vec<(String, &Metrics, &Metrics)> = vec![("overall".into(), &group.overall, &base.overall)];
        for (name, metrics) in &group.categories {
            if let Some(base_metrics) = base.categories.get(name) {
                scopes.push((format!("category {name}"), metrics, base_metrics));
            }
        }
        for (name, metrics) in &group.tasks {
            if let Some(base_metrics) = base.tasks.get(name) {
                scopes.push((format!("task {name}"), metrics, base_metrics));
            }
        }
        for (scope, now, then) in scopes {
            for t in THRESHOLDS {
                let (a, b) = ((t.get)(then), (t.get)(now));
                if a.of < MIN_RUNS || b.of < MIN_RUNS {
                    continue;
                }
                let (Some(before), Some(after)) = (a.rate(), b.rate()) else { continue };
                let worse = if t.higher_is_better { before - after } else { after - before };
                if worse > t.allowed + 1e-9 {
                    found.push(Regression {
                        group: label.clone(),
                        scope: scope.clone(),
                        metric: t.metric.into(),
                        baseline: format!("{}/{} ({})", a.n, a.of, pct(a)),
                        current: format!("{}/{} ({})", b.n, b.of, pct(b)),
                    });
                }
            }
            if scope == "overall"
                && let (Some(before), Some(after)) = (then.total_p50, now.total_p50)
                && after > before + before / 2
                && after > before + LATENCY_FLOOR_MS
            {
                found.push(Regression {
                    group: label.clone(),
                    scope,
                    metric: "total_p50".into(),
                    baseline: format!("{before} ms"),
                    current: format!("{after} ms"),
                });
            }
        }
    }
    found
}

fn pct(r: Ratio) -> String {
    r.rate().map_or("–".into(), |rate| format!("{:.0}%", rate * 100.0))
}

fn cell(r: Ratio) -> String {
    if r.of == 0 { "–".into() } else { format!("{}/{}", r.n, r.of) }
}

fn ms(value: Option<u64>) -> String {
    value.map_or("–".into(), |v| format!("{v}"))
}

/// The report, in Markdown.
pub fn report(summary: &Summary, regressions: Option<&[Regression]>, unmatched: &[String]) -> String {
    let mut out = String::from("# SuperEditr AI edit eval\n\n");
    if !unmatched.is_empty() {
        let _ = writeln!(out, "Not compared with the baseline: {}.\n", unmatched.join("; "));
    }
    match regressions {
        Some([]) => out.push_str("No regressions against the baseline.\n\n"),
        Some(found) => {
            let _ = writeln!(out, "## Regressions ({})\n", found.len());
            out.push_str("| Group | Scope | Metric | Baseline | Now |\n|---|---|---|---|---|\n");
            for r in found {
                let _ = writeln!(out, "| {} | {} | {} | {} | {} |", r.group, r.scope, r.metric, r.baseline, r.current);
            }
            out.push('\n');
        }
        None => {}
    }
    if summary.groups.len() > 1 {
        out.push_str("## Overview\n\n| Group | Runs | Exact | Not an edit | Escaped | Refused | Unchanged | Damage | Total p50 ms |\n");
        out.push_str("|---|---|---|---|---|---|---|---|---|\n");
        for group in &summary.groups {
            let m = &group.overall;
            let _ = writeln!(
                out,
                "| {} | {} | {} | {} | {} | {} | {} | {} | {} |",
                group.key.label(),
                m.runs,
                pct(m.exact),
                pct(m.not_edit),
                pct(m.escaped),
                pct(m.refused),
                pct(m.unchanged),
                pct(m.damage),
                ms(m.total_p50),
            );
        }
        out.push('\n');
    }
    for group in &summary.groups {
        let _ = writeln!(out, "## {}\n", group.key.label());
        let _ = writeln!(
            out,
            "Model identity `{}`; runs {}; commit {}.\n",
            group.key.model_identity,
            group.run_ids.join(", "),
            if group.app_commits.is_empty() { "unknown".into() } else { group.app_commits.join(", ") }
        );
        out.push_str(
            "| Scope | Runs | Exact | Distance | Not an edit | Caught | Escaped | False alarms | Refused | Unchanged | Changed correct | Damage | Content lost | Errors | Total p50/p90 ms | First token p50/p90 ms | Unstable |\n",
        );
        out.push_str("|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|---|\n");
        let mut rows: Vec<(String, &Metrics)> = vec![("**overall**".into(), &group.overall)];
        rows.extend(group.categories.iter().map(|(name, m)| (name.clone(), m)));
        rows.extend(
            group.tasks.iter().filter(|(name, _)| *name != "proofread").map(|(name, m)| (format!("task: {name}"), m)),
        );
        for (name, m) in rows {
            let _ = writeln!(
                out,
                "| {name} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {} | {}/{} | {}/{} | {} |",
                m.runs,
                cell(m.exact),
                m.distance.map_or("–".into(), |d| format!("{d:.3}")),
                cell(m.not_edit),
                cell(m.caught),
                cell(m.escaped),
                cell(m.false_alarms),
                cell(m.refused),
                cell(m.unchanged),
                cell(m.changed_correct),
                cell(m.damage),
                cell(m.content_lost),
                cell(m.errors),
                ms(m.total_p50),
                ms(m.total_p90),
                ms(m.first_token_p50),
                ms(m.first_token_p90),
                cell(m.unstable),
            );
        }
        if !group.escaped_samples.is_empty() {
            let list: Vec<String> = group.escaped_samples.iter().map(|(id, n)| format!("{id} ({n})")).collect();
            let _ = writeln!(out, "\nNot an edit, and reached the user: {}.", list.join(", "));
        }
        let m = &group.overall;
        let _ = writeln!(
            out,
            "\nFormatting damage: trailing spaces {}, curly quotes {}, lines merged {}, list items lost {}, code changed {}. Exact match differed between runs of the same sample in {} samples.\n",
            cell(m.trailing_space),
            cell(m.curly_quotes),
            cell(m.lines_merged),
            cell(m.lists_lost),
            cell(m.code_changed),
            cell(m.exact_flips),
        );
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::corpus::Sample;
    use crate::run::Env;

    const QUESTION: &str = "are there any other tricks or techniques we can take (without compromising and doing really hacky thing) to improve the latency or perceived latency?";
    const FIXED: &str = "Are there any other tricks or techniques we can use (without compromising or doing really hacky things) to improve the latency or perceived latency?";
    const ANSWER: &str = "There are several techniques to improve latency. Here are a few suggestions: use a cache, prefetch data, and show progress early.";

    fn record(mode: &str, rep: usize, outcome: Outcome, reply: Option<&str>, output: Option<&str>, ms: u64) -> Record {
        Record {
            run_id: format!("apple-{mode}-1"),
            started_unix: 1,
            env: Env { os: "macos".into(), os_version: "26.5".into(), ..Default::default() },
            provider: "apple".into(),
            model: "system".into(),
            model_identity: "apple-on-device/25F71".into(),
            mode: mode.into(),
            rep,
            sample: Sample {
                id: "q01".into(),
                category: "question".into(),
                preset: "proofread".into(),
                text: QUESTION.into(),
                expected: vec![FIXED.into()],
                words: vec![],
                must_keep: vec![],
                note: String::new(),
            },
            outcome,
            output: output.map(str::to_string),
            reply: reply.map(str::to_string),
            raw: None,
            guard_flagged: None,
            message: None,
            first_token_ms: Some(ms / 2),
            total_ms: ms,
            rate_limit_retries: 0,
        }
    }

    #[test]
    fn counts_answers_caught_and_escaped() {
        let records = vec![
            record("plain", 1, Outcome::Refused, Some(ANSWER), None, 900),
            record("plain", 2, Outcome::Edited, Some(ANSWER), Some(ANSWER), 1000),
            record("plain", 3, Outcome::Edited, Some(FIXED), Some(FIXED), 500),
        ];
        let summary = summarize(&records);
        let m = &summary.groups[0].overall;
        assert_eq!(m.not_edit, Ratio { n: 2, of: 3 });
        assert_eq!(m.caught, Ratio { n: 1, of: 2 });
        assert_eq!(m.escaped, Ratio { n: 1, of: 3 });
        assert_eq!(m.false_alarms, Ratio { n: 0, of: 1 });
        assert_eq!(m.exact, Ratio { n: 1, of: 3 });
        assert_eq!(m.unstable, Ratio { n: 1, of: 1 });
        assert_eq!(m.exact_flips, Ratio { n: 1, of: 1 });
        assert_eq!(m.total_p50, Some(500), "only delivered results are timed");
        assert!(summary.groups[0].categories.contains_key("question"));
    }

    #[test]
    fn flags_a_drop_against_the_baseline() {
        let good: Vec<Record> =
            (1..=10).map(|rep| record("plain", rep, Outcome::Edited, Some(FIXED), Some(FIXED), 500)).collect();
        let mut bad = good.clone();
        for r in bad.iter_mut().take(4) {
            r.outcome = Outcome::Edited;
            r.reply = Some(ANSWER.into());
            r.output = Some(ANSWER.into());
        }
        for r in &mut bad {
            r.env.os_version = "26.6".into();
        }
        let baseline = summarize(&good);
        let found = regressions(&summarize(&bad), &baseline);
        let metrics: BTreeSet<&str> = found.iter().map(|r| r.metric.as_str()).collect();
        assert!(metrics.contains("exact") && metrics.contains("escaped"), "{found:?}");
        assert!(found[0].group.contains("baseline: apple system · plain · macos 26.5"), "{}", found[0].group);
        assert!(regressions(&baseline, &baseline).is_empty());
        let report = report(&summarize(&bad), Some(&found), &[]);
        assert!(report.contains("## Regressions"));
    }

    #[test]
    fn a_new_model_is_compared_with_the_one_it_replaces() {
        let good: Vec<Record> =
            (1..=10).map(|rep| record("plain", rep, Outcome::Edited, Some(FIXED), Some(FIXED), 500)).collect();
        let baseline = summarize(&good);
        let mut renamed = good.clone();
        for r in renamed.iter_mut().take(4) {
            r.model = "system-2".into();
            r.output = Some(ANSWER.into());
        }
        renamed.iter_mut().for_each(|r| r.model = "system-2".into());
        let current = summarize(&renamed);
        assert!(unmatched(&current, &baseline).is_empty());
        assert!(regressions(&current, &baseline).iter().any(|r| r.metric == "exact"));

        let mut other = good.clone();
        other.iter_mut().for_each(|r| r.provider = "openai".into());
        assert_eq!(
            unmatched(&summarize(&other), &baseline),
            vec!["openai system · plain · macos 26.5: no baseline".to_string()]
        );
        let mut two_models = baseline.clone();
        let mut second = two_models.groups[0].clone();
        second.key.model = "system-3".into();
        two_models.groups.push(second);
        assert_eq!(unmatched(&current, &two_models).len(), 1, "ambiguous: two baseline models");
    }

    #[test]
    fn a_run_must_cover_the_baselines_samples() {
        let good: Vec<Record> =
            (1..=10).map(|rep| record("plain", rep, Outcome::Edited, Some(FIXED), Some(FIXED), 500)).collect();
        let baseline = summarize(&good);
        assert!(unmatched(&baseline, &baseline).is_empty());
        let mut partial = good.clone();
        partial.iter_mut().for_each(|r| r.sample.id = "q02".into());
        let found = unmatched(&summarize(&partial), &baseline);
        assert!(found[0].contains("2 samples missing, added or changed since the baseline (q01, q02)"), "{found:?}");
        let mut extra = good.clone();
        extra.extend(partial.iter().cloned());
        let found = unmatched(&summarize(&extra), &baseline);
        assert!(found[0].contains("1 samples missing, added or changed since the baseline (q02)"), "{found:?}");
        let mut changed = good.clone();
        changed.iter_mut().for_each(|r| r.sample.expected.push("Another answer.".into()));
        assert_eq!(unmatched(&summarize(&changed), &baseline).len(), 1, "a changed sample isn't comparable");
    }

    #[test]
    fn a_run_must_cover_the_baselines_modes() {
        let both: Vec<Record> = ["plain", "shipped"]
            .iter()
            .flat_map(|mode| (1..=3).map(|rep| record(mode, rep, Outcome::Edited, Some(FIXED), Some(FIXED), 500)))
            .collect();
        let baseline = summarize(&both);
        assert!(unmatched(&baseline, &baseline).is_empty());
        let plain_only: Vec<Record> = both.iter().filter(|r| r.mode == "plain").cloned().collect();
        assert_eq!(
            unmatched(&summarize(&plain_only), &baseline),
            vec!["apple shipped mode: in the baseline but not run"]
        );
        assert_eq!(unmatched(&summarize(&[]), &baseline), vec!["no runs to compare"]);
    }

    #[test]
    fn os_versions_compare_as_numbers() {
        assert!(version("27.10") > version("27.2"));
        assert!(version("26.5.2") > version("26.5"));
    }
}
