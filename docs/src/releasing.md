# Releasing

A release is a tag on `main`. Pushing the tag runs the release workflow, which builds the command
line and the desktop app and publishes them as a GitHub Release. Only a maintainer pushes tags.

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
4. **`build-global-artifacts`** makes the shell and PowerShell installers, `source.tar.gz`, and
   `sha256.sum`.
5. **`host`** creates the GitHub Release, with the changelog section as its notes, and uploads
   every file. It runs only if every build succeeded, so a failed build publishes nothing.

On pull requests, `release.yml` runs only `plan`, to check the config. `desktop-installers.yml`
also runs on pull requests that change how the app is packaged, and you can run it by hand
(`gh workflow run desktop-installers.yml`) to check the installers build before tagging. Neither
attests anything; only a tag does.

## Secrets

There are none. The release uses only the `GITHUB_TOKEN` GitHub gives each run, which reaches
only this repository and expires when the job ends. Each job gets only what it needs:

| Job | Permissions |
|---|---|
| `build-local-artifacts` | `contents: read`, plus `id-token: write` and `attestations: write` to attest what it builds |
| The desktop installers' builds | `contents: read` |
| The desktop installers' attest job | `contents: read`, `id-token: write`, `attestations: write` |
| `plan`, `build-global-artifacts`, `host`, `announce` | `contents: write`, which `host` needs to create the release |

The builds aren't signed, so there are no signing keys or certificates either. macOS's ad-hoc
signature needs none. A publish job that needs a secret adds it to this page, with the one place
it can write to.

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
verify; and that the README's installer one-liners install the new version.

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
