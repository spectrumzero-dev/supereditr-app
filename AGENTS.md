# AGENTS.md

SuperEditr: a Rust/Tauri macOS proofreading app. `README.md` describes the layout and how to build.

## Checks

Run `./scripts/check.sh` before every commit.

## Everything here is public

Every file in this repository is public, and so is every commit message, branch name, pull request
and issue. Write every line, comment and test so a stranger can read it:

- **Example data is invented.** Use made-up names (Rhyan, Brynlee, Kyndal), addresses
  ("1234 Glenmoor Ct"), organisations (QuillPoint) and generic acronyms (KPI, OKR); real people,
  places, employers and accounts stay out.
- **Describe what the code does today.** SuperEditr's own prices, paid tiers, licensing plans and
  roadmap, and comparisons with competing products, stay out of code, comments, tests, commit
  messages and pull requests. Facts about AI providers, such as their published pricing, are fine.
- **Name infrastructure generically**: "a CI runner", "the signing machine", "a maintainer's Mac".
  Non-public hostnames, machine names and IP addresses, home-directory paths, internal tool names
  and private repository names stay out. Public service endpoints, `localhost` and invented
  examples (`db.internal`) are fine.
- **Credentials appear only as obviously fake fixtures**, such as `sk-proj-0123456789`. A test that
  needs a full-length key builds it from pieces at run time, as `crates/core/src/sensitive/tests.rs`
  does, so no complete key appears in the source for secret scanners to flag.
- **Commit messages say what changed in plain words.** Context that isn't public goes in no commit
  or pull request.

Before committing, read `git diff --staged` as if it were already published.
