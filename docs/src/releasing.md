# Releasing

A release is a tag on `main`. Pushing the tag runs the release workflow, which builds the command
line and the desktop app and publishes them as a GitHub Release, then publishes the command line
to crates.io and its formula to the Homebrew tap. Only a maintainer pushes tags.

## Preparing the release

First, a `chore(release): prepare vX.Y.Z` pull request, as
[#38](https://github.com/zhenga8533/cleanse/pull/38) was for v0.5.0. It:

- sets the version under `[workspace.package]` in `Cargo.toml`, and on each `cleanse-*` entry
  under `[workspace.dependencies]`, then updates `Cargo.lock` with a build. The desktop app takes
  its version from the workspace, so its installers are named for it too.
- turns the changelog's Unreleased section into `## [X.Y.Z] - YYYY-MM-DD`, with a short summary
  above the entries, and updates the links at the bottom. dist makes this section the release's
  notes, so its links must be absolute.
- updates the README's status, and its roadmap if a milestone is done.

Before merging it, run `cargo xtask ci` and `cargo xtask desktop`, and `dist plan --tag vX.Y.Z`,
which creates nothing and shows the release's title and notes.

Merging it releases nothing. Hold other merges until the release is out, so that what's tagged is
what the changelog describes, even if the tag has to move.

## Tagging

Once the pull request is merged and CI on `main` is green:

```sh
git switch main
git pull --ff-only
git tag vX.Y.Z
git push origin vX.Y.Z
```

The tag is `v` followed by the version in `Cargo.toml`.

## What runs

The tag starts [`release.yml`](https://github.com/zhenga8533/cleanse/blob/main/.github/workflows/release.yml).
dist generates it from `dist-workspace.toml`, so change the config and run `dist generate` rather
than editing the workflow; dist's plan fails while they disagree. Its jobs:

1. **`plan`** works out what to build from the tag.
2. **`build-local-artifacts`** builds the CLI for each of its five targets, archives it with a
   `.sha256` beside it, and attests the archive.
3. **`custom-desktop-installers`** runs
   [`desktop-installers.yml`](https://github.com/zhenga8533/cleanse/blob/main/.github/workflows/desktop-installers.yml).
   It builds the MSI on Windows, the universal disk image on macOS, ad-hoc signed, and the AppImage
   and deb on Ubuntu 22.04, writes a `.sha256` beside each, and attests them in a job of its own.
4. **`build-global-artifacts`** makes the shell and PowerShell installers, the Homebrew formula,
   `source.tar.gz`, and `sha256.sum`.
5. **`host`** creates the GitHub Release, with the changelog section as its notes, and uploads
   every file. It runs only if every build succeeded, so a failed build publishes nothing.
6. **`custom-publish-crates`** runs
   [`publish-crates.yml`](https://github.com/zhenga8533/cleanse/blob/main/.github/workflows/publish-crates.yml)
   once the release exists, and not for prereleases. It publishes `cleanse-cli` and the five
   library crates it's built from to crates.io, leaving out any whose version crates.io already
   has. See [crates.io](#cratesio).
7. **`custom-publish-homebrew`** runs
   [`publish-homebrew.yml`](https://github.com/zhenga8533/cleanse/blob/main/.github/workflows/publish-homebrew.yml)
   beside it, and also not for prereleases. It commits the release's formula to the tap. See
   [Homebrew](#homebrew).

On pull requests, `release.yml` runs only `plan`, to check the config. `desktop-installers.yml`
also runs on pull requests that change how the app is packaged, and you can run it by hand
(`gh workflow run desktop-installers.yml`) to check the installers build before tagging. Neither
attests anything; only a tag does. `publish-crates.yml` runs as a dry run on pull requests that
change it, and CI's Linux test job runs `cargo publish --workspace --dry-run` on every pull
request, which packages each crate and builds it from its package alone. `publish-homebrew.yml`
runs only for a release, since there's no formula before one.

## Secrets

There's one, for the Homebrew tap:

| Secret | What it is | What it can write to | Used by |
|---|---|---|---|
| `HOMEBREW_TAP_TOKEN` | A fine-grained personal access token | Only `zhenga8533/homebrew-tap`: Contents, read and write | `custom-publish-homebrew` |

Everything else uses the `GITHUB_TOKEN` GitHub gives each run, which reaches only this
repository and expires when the job ends. Publishing to crates.io needs no secret: crates.io
trades the job's OIDC token for one of its own, which the job revokes when it ends. Each job's
`GITHUB_TOKEN` gets only what that job needs:

| Job | Permissions |
|---|---|
| `build-local-artifacts` | `contents: read`, plus `id-token: write` and `attestations: write` to attest what it builds |
| The desktop installers' builds | `contents: read` |
| The desktop installers' attest job | `contents: read`, `id-token: write`, `attestations: write` |
| `custom-publish-crates` | `contents: read`, plus `id-token: write` for its crates.io token |
| `custom-publish-homebrew` | `contents: read`. It writes to the tap with `HOMEBREW_TAP_TOKEN` |
| `plan`, `build-global-artifacts`, `host`, `announce` | `contents: write`, which `host` needs to create the release |

The builds aren't signed, so there are no signing keys or certificates either. macOS's ad-hoc
signature needs none. A publish job that needs a secret adds it to this page, with the one place
it can write to.

## crates.io

The release publishes six crates: `cleanse-cli`, which installs `cleanse`, and the crates it's
built from, `cleanse-model`, `cleanse-platform`, `cleanse-core`, `cleanse-exec`, and
`cleanse-engine`. Their descriptions say they're internal to `cleanse-cli`, with no stable API,
so their versions are the CLI's and promise nothing about their own APIs.
`cargo publish --workspace` finds them as the packages without `publish = false`, and publishes
them in the order they depend on each other.

A version on crates.io can't be replaced or deleted, only yanked. So the job leaves out each crate
whose version is already there, and rerunning it after a failure carries on where it stopped.

**Trusted publishing:** each crate on crates.io names a trusted publisher, this repository's
`release.yml`. The job's
[crates-io-auth-action](https://github.com/rust-lang/crates-io-auth-action) step trades the job's
OIDC token for a crates.io token, which crates.io gives only if the OIDC token's `workflow_ref`
is `release.yml`. That's the workflow a run started from, so it's `release.yml` even though the
steps are in `publish-crates.yml`, which `release.yml` calls. So only `release.yml`'s runs can
publish, not `publish-crates.yml`'s own runs on pull requests.

**The first publish, at v1.0.0:** a trusted publisher can only be added to a crate that exists,
so the first version is published by hand, before the tag:

1. Merge the release-prep pull request, and hold other merges, as usual.
2. On crates.io, create an API token with the `publish-new` and `publish-update` scopes, for
   crates matching `cleanse-*`, that expires soon. Then publish what's on `main`:

   ```sh
   git switch main
   git pull --ff-only
   cargo login          # paste the token
   cargo publish --workspace --locked
   cargo logout
   ```

3. In each crate's **Settings** on crates.io, add a trusted publisher: GitHub, owner `zhenga8533`,
   repository `cleanse`, workflow `release.yml`, and no environment.
4. Revoke the API token. Optionally, set each crate's publishing method restriction to trusted
   publishing only, so that no API token can publish it.
5. Tag as usual. `custom-publish-crates` finds all six crates already published, and publishes
   nothing.

From the next version on, the release publishes them.

## Homebrew

The tap is [`zhenga8533/homebrew-tap`](https://github.com/zhenga8533/homebrew-tap), so
`brew install zhenga8533/tap/cleanse-cli` installs the command line on macOS and Linux, and
`brew upgrade` updates it. dist writes the formula, `cleanse-cli.rb`, with the release's other
installers. It downloads the release's archive for the system it's on, and checks it against the
archive's SHA-256.

`custom-publish-homebrew` copies the formula to the tap's `Formula/cleanse-cli.rb`, and commits it
as `github-actions[bot]`, with the message `cleanse-cli X.Y.Z`. If the tap already has that
formula, as when the job is rerun, it commits nothing.

The job is this repository's own, rather than dist's Homebrew job, which would run with
`release.yml`'s `contents: write` on this repository, after the release's files are uploaded.
This one's `GITHUB_TOKEN` can only read, and the tap's token can write only to the tap.

**Setting it up, once, before the first release that has the formula:**

1. Create `zhenga8533/homebrew-tap` on GitHub, public, with a README, so that it has a branch to
   commit to.
2. Create a fine-grained personal access token for it: resource owner `zhenga8533`, only the
   `homebrew-tap` repository, and the Contents permission set to read and write. Give it an
   expiry, and note when to renew it.
3. Add the token to this repository as the Actions secret `HOMEBREW_TAP_TOKEN`. This asks for the
   value, so it doesn't end up in your shell's history:

   ```sh
   gh secret set HOMEBREW_TAP_TOKEN --repo zhenga8533/cleanse
   ```

Without the secret, the job fails before it does anything. With an expired token, it fails to
check out the tap. Either way, set the secret, then rerun the failed job.

## Checking a release

Anyone can check a download in two ways:

- **Build provenance:** `gh attestation verify <file> --repo zhenga8533/cleanse` checks that this
  repository's release workflow built the file. It covers the CLI's archives and the desktop
  installers. The installer scripts, `sha256.sum`, and `source.tar.gz` aren't attested.
- **Checksums:** every file has a `.sha256` beside it, and `sha256.sum` lists the CLI's archives
  and the source together. A checksum comes from the same place as the file, so it shows the
  download is complete and undamaged, not where it was built.

For example, on Linux:

```sh
gh release download v0.5.0 --repo zhenga8533/cleanse --pattern 'cleanse-cli-x86_64-unknown-linux-gnu.tar.xz*'
gh attestation verify cleanse-cli-x86_64-unknown-linux-gnu.tar.xz --repo zhenga8533/cleanse
sha256sum -c cleanse-cli-x86_64-unknown-linux-gnu.tar.xz.sha256
```

On macOS, `shasum -a 256 -c` checks a `.sha256`. On Windows, in PowerShell, this prints `True`
when they match:

```powershell
(Get-FileHash Cleanse_0.5.0_x64_en-US.msi).Hash -eq (Get-Content Cleanse_0.5.0_x64_en-US.msi.sha256).Split(' ')[0]
```

After each release, check that its page has the five CLI archives, the two installer scripts, and
the four desktop installers, each with a `.sha256`; that a CLI archive and a desktop installer
verify; that the README's installer one-liners install the new version; that crates.io has
the version of all six crates; and that the tap's `Formula/cleanse-cli.rb` names it.

## When a release fails

**A tag is moved only if nothing was published from it.** Once a GitHub Release exists, even an
incomplete one, people may already have its files, and its checksums and attestations name them.
Moving the tag would put different files under the same version.

`gh release view vX.Y.Z` says whether anything was published: it reports "release not found" if
`host` never ran.

**If nothing was published:**

- If the failure was GitHub's, such as a runner outage, re-run the failed jobs with
  `gh run rerun <run-id> --failed`. They run on the same commit, and the tag doesn't move.
- Otherwise, fix the cause in a pull request, then tag again on the fix:

  ```sh
  git push origin --delete refs/tags/vX.Y.Z
  git switch main
  git pull --ff-only
  git tag -f vX.Y.Z
  git push origin vX.Y.Z
  ```

  The failed run's attestations stay, but they name files that were never released.

v0.2.0 and v0.5.0 were both tagged twice this way. v0.2.0's run was cancelled when GitHub retired
a macOS runner image ([#20](https://github.com/zhenga8533/cleanse/pull/20)), and v0.5.0's Linux
builds tried to compile the desktop app ([#39](https://github.com/zhenga8533/cleanse/pull/39)).
Neither had reached `host`.

**If something was published:** leave the tag and the release as they are. Fix the cause in a pull
request, then release the next patch version with the steps above. If the release is broken or
incomplete, say so at the top of its notes, and point to the fixed release.

**If only `custom-publish-crates` or `custom-publish-homebrew` failed,** the GitHub Release is
already out. If the failure was GitHub's, crates.io's, or a missing trusted publisher or tap
token, rerun the failed jobs once it's fixed: `custom-publish-crates` leaves out the crates it
already published, and `custom-publish-homebrew` commits nothing if the tap is up to date. If the
crates or the formula themselves need a fix, release the next patch version; what's already
published at this version stays as it is.
