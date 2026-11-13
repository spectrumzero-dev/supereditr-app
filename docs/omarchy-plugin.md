# The Omarchy bar widget

SuperEditr's Omarchy bar widget lives in three places:

- **Source**: `packaging/omarchy/` in this repository, with its tests in `tests/omarchy/`. All work on the widget happens here.
- **Published plugin**: [spectrumzero-dev/supereditr-omarchy](https://github.com/spectrumzero-dev/supereditr-omarchy). Its `main` branch is `packaging/omarchy/` as of the newest release that changed it, and it is what `omarchy plugin add` and `omarchy plugin update` install. Nobody commits to it directly.
- **Listing**: the [Omarchy plugin marketplace](https://plugins.omarchy.org), as plugin `app.supereditr`. The marketplace verifies exact commits of the published repository.

## Publishing a new version: the checklist

1. Change the widget in a pull request to this repository, like any other change.
2. In the same pull request, bump `version` in `packaging/omarchy/manifest.json` (SemVer: a fix bumps the patch number, a new button or status the minor number).
3. Merge it. Nothing reaches users yet: merges to `main` are never published.
4. Release as usual. The widget change makes the release ship Linux, and publishing the release publishes the widget and asks the marketplace to verify it.
5. Check the release's run of the Omarchy plugin workflow (`gh run list --workflow omarchy-plugin.yml --limit 1`). It ends green, having pushed the new commit to the published repository and filed the marketplace request, or says in its log why it skipped one.

There is nothing to do in the published repository or on the marketplace's site. The rest of this page explains each step and what to do when one goes wrong.

## Changing the widget

Change it in a pull request here like any other code; `scripts/check.sh` runs its tests.

- **Bump `version` in `packaging/omarchy/manifest.json`** in the same pull request. It is the widget's own version, separate from the app's, and the marketplace shows it.
- **Never change the manifest's `id`.** Omarchy and the marketplace key everything on `app.supereditr`; a new id is a new plugin, and installed copies would stop updating.
- **Everything in `packaging/omarchy/` is published, and nothing else.** Its `README.md` becomes the published repository's README, written for people installing the widget. Tests and notes for developers belong outside the folder.
- **A widget change ships Linux.** `packaging/omarchy/` is in the Linux row of `docs/versioning.md`, so the next release ships Linux and publishes the widget. A change to the folder's Markdown alone ships nothing, so it goes out with the next release that ships Linux.

## Publishing

Publishing is part of releasing; there is no separate step. When a stable (not pre-release) release is published, `.github/workflows/omarchy-plugin.yml` runs on its tag:

1. It runs the widget's tests.
2. `git subtree split --prefix=packaging/omarchy` turns the folder's history into a branch whose root is the folder. The split is deterministic: the same history always gives the same commits.
3. It pushes that branch to the published repository's `main`. Because the split is deterministic, each push fast-forwards, and a release that didn't change the widget pushes nothing.
4. It asks the marketplace to verify the new commit by filing a "Verify and publish a newer upstream commit" request, unless the plugin isn't listed yet, the marketplace already lists that commit, or a request for it is open. The run's log says which.

Users get the new version the next time they run `omarchy plugin update app.supereditr`, which shows them the diff first. The listing shows "Update unverified" until the marketplace's checks pass, usually within minutes; a check that flags something waits for a marketplace maintainer.

## When a run goes wrong

- **The run failed.** Fix the cause, then run it again on the release's tag: `gh workflow run omarchy-plugin.yml --ref vX.Y.Z`.
- **The push was rejected as not a fast-forward.** The published repository has a commit that didn't come from here, or the tag is older than what is published. Don't force-push; find out how the commit got there.
- **No verification request was filed.** The run's log says why. Without the marketplace token the step is skipped; file the request by hand with the marketplace's "Verify or update a listed plugin" issue form.

## Setup

This is in place already; it is here for when something has to be recreated.

- **The published repository** is public, with issues, wiki and projects off, so reports come here. Two rulesets, one for branches and one for tags, refuse creating, updating and deleting refs, and force pushes, from everyone except the workflow's credential, administrators included.
- **The `omarchy-plugin` environment** in this repository holds the secrets, and only `v*` tags can use it, so branches and pull requests never see them:
  - `OMARCHY_PLUGIN_DEPLOY_KEY`: the private half of a deploy key with write access to the published repository alone.
  - `OMARCHY_MARKETPLACE_TOKEN`: a maintainer's classic token with only the `public_repo` scope, which files the verification requests in that maintainer's name. A fine-grained token can't open issues in another organization's repository. It expires; replace it before it does.

## Why it works this way

- **A repository of its own.** Omarchy installs a plugin by cloning a repository's default branch, with `manifest.json` at its root, and the marketplace marks a listing unverified whenever that branch moves. Published from this repository, the widget would be unverified after almost every commit, and installing it would clone the whole app.
- **Published from releases.** `main` can be between releases. Publishing from release tags means users only ever get a widget from a release, and one that matches a released app.
- **No `AGENTS.md` in the published repository.** Nobody works there, its README sends people here, and every file in it is installed on users' machines.
