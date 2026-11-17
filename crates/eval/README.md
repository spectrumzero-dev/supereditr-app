# AI edit eval

`supereditr-eval` runs SuperEditr's AI edits over a fixed corpus of invented texts and scores the results, so a model change (a new macOS release with a new Apple Intelligence model, a new cloud model, a prompt or guard change) can be measured against the last run instead of judged from a handful of tries. It is not part of `scripts/check.sh` and doesn't run on every pull request: it calls real models, takes minutes and costs money for cloud providers.

## What it runs

The runner calls the app's own code, not a copy of it:

- the prompt builder (`prompts::build`) with the sample's preset, the American English dialect and the sample's word list;
- the provider the app uses (`Providers::stream`), including the Apple Intelligence helper built from `helpers/foundation-models`;
- `prompts::clean_output` and the guard `prompts::answers_instead_of_editing`.

Each sample runs in one or more **modes**:

| Mode | What it measures |
|---|---|
| `plain` | The prompt as free text, then `clean_output` and the guard. The model alone: no retry, no offline checker, no code protection. |
| `guided` | The same with Apple's guided generation (`guided: true`). Apple Intelligence only. |
| `shipped` | The whole engine, `Engine::run`, as the app reviews a selection: whatever strategy the app ships (today, free text with one guided retry when the reply answers the text), the offline checker's final pass, code protection and the author's word list. The result is what applying the review gives. |

Apple Intelligence runs `plain,guided,shipped` by default; other providers run `plain,shipped`. Each sample runs three times per mode by default (`--runs`), because the same model can answer differently each time.

## The corpus

`corpus.jsonl` holds one sample per line. Every text is invented. Fields:

| Field | Meaning |
|---|---|
| `id` | Unique name, such as `draft-07`. |
| `category` | `draft` (ordinary writing, the common case), `formatted` (lists, line breaks, tables), `markdown_code`, `long`, `question` (a selection shaped like a question), `injection` (instructions inside the text), `correct` (nothing to fix) or `non_english`. |
| `preset` | A built-in preset id; `proofread` when absent. |
| `text` | The selection. |
| `expected` | Acceptable results; any of them is an exact match. Required for Proofread. For `correct` it defaults to the text. Rewrites (Polish, Shorter, Friendlier, Professional, Simplify, To English) have none. |
| `words` | The author's word list for the sample. |
| `must_keep` | Terms a good result keeps, compared without regard to case. Used where exact matching doesn't apply. |
| `note` | Why the sample is there. |

Add samples by appending lines. Keep each expected result to the smallest correct edit, and list more than one when a careful editor could reasonably do either (an optional comma, a capitalised list item). `cargo test -p supereditr-eval` checks the file: unique ids, known categories and presets, an expected result for every Proofread sample, and drafts as the largest category.

## Running it

```sh
scripts/eval.sh apple
scripts/eval.sh openai
scripts/eval.sh anthropic target/eval/claude -- --model claude-haiku-5-5 --runs 1
```

The script compiles the Apple Intelligence helper on a Mac (and prints its first line, which says whether the on-device model is available), runs the suite and scores it. Results land in `target/eval/<provider>-<time>/`: `results.jsonl`, `report.md` and `summary.json`. Cloud providers read their key from `ANTHROPIC_API_KEY`, `OPENAI_API_KEY` or `GEMINI_API_KEY`. Pass runner flags after `--`:

- `--modes plain,shipped` to choose modes;
- `--runs N` to change the number of runs per sample;
- `--only question,injection,draft-07` to run only some categories, presets or ids;
- `--model M` and `--base-url URL` as in the `supereditr` CLI.

To compare against a stored baseline, set `EVAL_BASELINE` to an earlier `summary.json`, or score directly:

```sh
cargo run --release -p supereditr-eval -- score new/results.jsonl --baseline old/summary.json --fail-on-regression
```

Scoring several results files together (for example the same provider on two macOS versions) puts them side by side in one report.

## Reading the report

The report has one section per provider, model, mode and OS version, with a row for all runs, one per category, and one per rewrite task. Counts are shown as hits out of the runs they apply to.

| Column | Meaning |
|---|---|
| Exact | The result matched an acceptable one exactly, out of runs with an expected result. A refusal counts as a miss. |
| Distance | Mean word-level edit distance from the closest acceptable result, as a share of its words. 0 is exact; 0.05 is about one word in twenty. |
| Not an edit | The model's reply, before the guard, was not an edit of the text: it keeps under half of the original's words (under 30% for rewrites), or is more than twice as long plus 20 words. This is the eval's own judgement, separate from the app's guard, and is known only in `plain` and `guided` modes. |
| Caught | Of those, how many the guard turned away. |
| Escaped | A reply that was not an edit reached the user, out of all runs. This is the number that matters most. |
| False alarms | The guard turned away a real edit, out of all real edits. |
| Refused | The user would see an error instead of an edit. |
| Unchanged | The text came back unchanged although it needed a fix. |
| Changed correct | Already-correct text was changed. |
| Damage | Formatting damage compared with the original: lines with new trailing spaces, straight quotes turned curly, fewer lines (Proofread only), fewer list items (not judged for Shorter and Simplify), or a code span or block changed. The line under each table counts each kind. |
| Content lost | A link, number, mention, code span or `must_keep` term is gone. |
| Total, First token | Median and 90th-percentile latency in milliseconds, over delivered results. Each mode starts with one unmeasured warm-up request. |
| Unstable | Samples whose runs gave more than one distinct result. |

With `--baseline`, the report starts with a regressions table. A baseline group is matched by provider, mode and OS, and by model; when the baseline has exactly one model for that provider and mode, a run with a new model name is compared with it, so a new cloud model is measured against the one it replaces. Among those it takes the same OS version when the baseline has one, otherwise the latest one, so a run on a new macOS release is compared with the previous release. A group with nothing to compare with is named at the top of the report. A rate is flagged when it moves the wrong way by more than: exact 10 points, escaped 3, not an edit 5, refused 5, false alarms 3, unchanged 10, changed correct 10, damage 5, content lost 5, errors 5. Rates are compared only when both sides have at least 9 runs. Median total latency is flagged when it grows by more than half and by more than 250 ms. With `--fail-on-regression` the command exits with status 1 when anything is flagged or a group has no baseline, and a baseline file that can't be read is always an error.

## Results files

Each line of `results.jsonl` is one run: the sample as it was run, the mode and repetition, the provider, model and `model_identity` (the model name for a cloud or local server; `apple-on-device/<OS build>` for Apple's model, which ships with the OS), the OS name, version and build, the app version and commit, the outcome (`edited`, `refused` or `error`), the delivered `output`, and in `plain` and `guided` modes the model's `raw` reply, the cleaned `reply` and whether the guard flagged it. Because each line carries its own sample, an old results file can be scored again after the corpus or the scorer changes.
