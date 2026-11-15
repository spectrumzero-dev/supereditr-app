# The Omarchy bar widget

SuperEditr's Omarchy bar widget lives in three places:

- **Source**: `packaging/omarchy/` in this repository, with its tests in `tests/omarchy/`. All work on the widget happens here.
- **Published plugin**: [spectrumzero-dev/supereditr-omarchy](https://github.com/spectrumzero-dev/supereditr-omarchy). Its `main` branch is `packaging/omarchy/` as of the newest release that shipped Linux, and it is what `omarchy plugin add` and `omarchy plugin update` install. Nobody commits to it directly.
- **Listing**: the [Omarchy plugin marketplace](https://plugins.omarchy.org), as plugin `app.supereditr`. The marketplace verifies exact commits of the published repository.

## Publishing a new version: the checklist

1. Change the widget in a pull request to this repository, like any other change. Leave its version alone.
2. Merge it. Nothing reaches users yet: merges to `main` are never published.
3. Release as usual. The release's version pull request sets the widget's version with the app's, the widget change makes the release ship Linux, and publishing the release publishes the widget and asks the marketplace to verify it.
4. Check the release's run of the Omarchy plugin workflow (`gh run list --workflow omarchy-plugin.yml --limit 1`). It ends green, having pushed the new commit to the published repository and filed the marketplace request, or says in its log why it skipped one.

There is nothing to do in the published repository or on the marketplace's site. The rest of this page explains each step and what to do when one goes wrong.

## Changing the widget

Change it in a pull request here like any other code; `scripts/check.sh` runs its tests.

- **Leave `version` in `packaging/omarchy/manifest.json` to the version pull request.** The widget carries the app's version (`docs/versioning.md`), so the widget published with 0.1.9 is the widget at the 0.1.9 tag. `tests/versions.test.mjs` fails if it differs from the app's. It went from the widget's own 1.0.1 to the app's 0.1.8 once, in November 2026; Omarchy doesn't compare versions, so installed copies still update.
- **Never change the manifest's `id`.** Omarchy and the marketplace key everything on `app.supereditr`; a new id is a new plugin, and installed copies would stop updating.
- **Everything in `packaging/omarchy/` is published, and nothing else.** Its `README.md` becomes the published repository's README, written for people installing the widget. Tests and notes for developers belong outside the folder.
- **A widget change ships Linux.** `packaging/omarchy/` is in the Linux row of `docs/versioning.md`, so the next release ships Linux and publishes the widget. A change to the manifest's version alone is release bookkeeping and ships nothing, and neither does a change to the folder's Markdown alone; both go out with the next release that ships Linux.

## Publishing

Publishing is part of releasing; there is no separate step. When a stable (not pre-release) release is published, `.github/workflows/omarchy-plugin.yml` runs on its tag:

1. It checks that the release ships Linux, by its assets including `supereditr-<version>-x86_64.tar.gz`, and stops there with a notice when it doesn't. A release for macOS or Windows alone leaves the published widget at the last Linux release's.
2. It runs the widget's tests and the check that every version file agrees.
3. `git subtree split --prefix=packaging/omarchy` turns the folder's history into a branch whose root is the folder. The split is deterministic: the same history always gives the same commits.
4. It pushes that branch to the published repository's `main`. Because the split is deterministic, each push fast-forwards. The version pull request changes the manifest, so every Linux release pushes a new commit, even one whose only widget change is the version.
5. It asks the marketplace to verify the new commit by filing a "Verify and publish a newer upstream commit" request, unless the plugin isn't listed yet, the marketplace already lists that commit, or a request for it is open. The run's log says which.

Users get the new version the next time they run `omarchy plugin update app.supereditr`, which shows them the diff first. The listing shows "Update unverified" until the marketplace's checks pass, usually within minutes; a check that flags something waits for a marketplace maintainer.

## When a run goes wrong

- **The run failed.** Fix the cause, then run it again on the release's tag: `gh workflow run omarchy-plugin.yml --ref vX.Y.Z`.
- **The push was rejected as not a fast-forward.** The published repository has a commit that didn't come from here, or the tag is older than what is published. Don't force-push; find out how the commit got there.
- **No verification request was filed.** The run's log says why. Without the marketplace token the step is skipped; file the request by hand with the marketplace's "Verify or update a listed plugin" issue form.

## Setup

This is in place already; it is here for when something has to be recreated.

- **The published repository** is public, with issues, wiki and projects off, so reports come here. Two rulesets, one for branches and one for tags, refuse creating, updating and deleting refs, and force pushes, from everyone except the SuperEditr Omarchy publisher app, administrators included.
- **The `omarchy-plugin` environment** in this repository holds the secrets, and only `v*` tags can use it, so branches and pull requests never see them:
  - the variable `OMARCHY_PLUGIN_APP_CLIENT_ID` and the secret `OMARCHY_PLUGIN_APP_PRIVATE_KEY`: the client ID and a private key of the SuperEditr Omarchy publisher, a GitHub App owned by the organization with only Contents read and write, installed only on the published repository. The workflow turns them into a token that expires within the hour. (The organization turns deploy keys off, so a deploy key can't do this.)
  - `OMARCHY_MARKETPLACE_TOKEN`: a maintainer's classic token with only the `public_repo` scope, which files the verification requests in that maintainer's name. A fine-grained token can't open issues in another organization's repository. It expires; replace it before it does.

## Why it works this way

- **A repository of its own.** Omarchy installs a plugin by cloning a repository's default branch, with `manifest.json` at its root, and the marketplace marks a listing unverified whenever that branch moves. Published from this repository, the widget would be unverified after almost every commit, and installing it would clone the whole app.
- **Published from releases.** `main` can be between releases. Publishing from release tags means users only ever get a widget from a release, and one that matches a released app.
- **The app's version, from Linux releases.** The widget runs only on Linux, beside the Linux app, so it is published with each Linux release and carries that release's number. Someone reporting widget 0.1.9 is running the code tagged v0.1.9, and nobody has a second version number to remember.
- **No `AGENTS.md` in the published repository.** Nobody works there, its README sends people here, and every file in it is installed on users' machines.
