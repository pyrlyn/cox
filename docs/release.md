# Releases

How a release is cut, what a failed one leaves behind, and how to recover.

## Cutting a release

A normal release only goes through **Actions → Bump and release** (`bump.yml`), or
`scripts/release.sh [patch|minor|major]`, which starts that workflow. `bump.yml` runs
`scripts/release.sh --local` to make the version commit, lands it through a pull request with
green required checks, tags the commit on `main`, creates a *draft* release with the
`CHANGELOG.md` section, and dispatches `release.yml` with `--ref <tag> -f tag=<tag>`.

`release.yml` then runs, in order:

1. `version`: checks the run is on the tag's commit and the tag is `v<Cargo.toml version>`
   (or `v<version>-test.<n>`).
2. `verify`: fmt, clippy and nextest on that commit.
3. `cli` (`build-cli.yml`) for `aarch64-apple-darwin` (Developer ID-signed) and both Linux
   targets, and `desktop` (`desktop-build.yml`) for the macOS app's DMG.
4. `publish`: uploads the archives and checksums to the draft, then publishes it.
5. `tap`: regenerates the Homebrew cask from the published release.

## When a release fails

- The tag and the draft stay. A draft is never `/releases/latest`, so `install.sh` and
  `cox self update` never see it.
- `notify-failure` opens or comments on a `release-failure` issue.
- A dispatch runs the workflow files **at the tag**, not on `main`, and `version` refuses any
  commit other than the tag's. Re-running `release.yml` with the same tag
  (`gh workflow run release.yml --ref <tag> -f tag=<tag>`) only helps when the cause is outside
  the tagged tree: a flaky runner, or a secret added since.
- A failure in the tag's own workflows, scripts or code is fixed on `main` and ships as the next
  version. `scripts/release.sh` raises the version whenever `v<Cargo.toml version>` is already
  tagged, so **Bump and release (patch)** picks the next number by itself, and its
  `CHANGELOG.md` section lists every commit since the failed tag. Never move or re-create a
  pushed tag: someone may already have fetched it.

## v0.1.2 (issue #86)

What failed. Tag `v0.1.2` is on `0694a8b` ("ci: notify on release failure (#85)"). In run
[36886248981](https://github.com/pyrlyn/cox/actions/runs/36886248981), both macOS `build` jobs
failed (`aarch64-apple-darwin` and `x86_64-apple-darwin`, signed, on `macos-latest`). `verify`
had passed on the same commit and the same runner image, and the Linux build jobs did not fail.
So the break is in a step only the signed macOS build jobs ran at that tag: the inline import of
`MACOS_CERTIFICATE` into a job keychain, `codesign` in `scripts/package.sh`, or the smoke test's
`Developer ID Application` check. Which of the three is **unverified**: the job logs could not be
read from where this note was written.

Why it is not re-run. Every one of those steps has been replaced on `main` since the tag:
`x86_64-apple-darwin` is gone (#132, Intel macOS is unsupported), the macOS jobs run on the
`xcode-27` image with a pinned Xcode 27 (#130), the CLI build moved into `build-cli.yml` (#135),
and that imports the certificate through pyrlyn/ci's `macos-keychain` action, the same one the
DMG uses (#355). Re-running `release.yml` on `v0.1.2` would run the tag's old workflow, not what
`main` ships.

Recovery. `v0.1.2` is left as it is: tag in place, draft unpublished. The next release is
**Bump and release (patch)** from `main`, which makes `v0.1.3`. Whether to delete the `v0.1.2`
draft is the maintainer's call; nothing depends on it.

Checked locally on Apple silicon (macOS 27) against `main`: `scripts/package.sh
aarch64-apple-darwin <dir>` builds the `dist` profile and writes `cox-aarch64-apple-darwin.tar.xz`,
and that `dist` binary prints `cox 0.1.2`. Signing was not run locally: the Developer ID import
and `codesign` need the release certificate, which only CI has.
