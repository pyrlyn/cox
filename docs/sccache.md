# sccache in CI

## How it works

CI compiles Rust through [sccache](https://github.com/mozilla/sccache) with the shared
Cloudflare R2 bucket. The setup lives in pyrlyn/ci (`ci-rust.yml` input `sccache: true`, or
`.github/actions/sccache` for this repo's own jobs); full guide: pyrlyn/ci `docs/sccache.md`.
It sets `RUSTC_WRAPPER=sccache` and `CARGO_INCREMENTAL=0` (sccache cannot cache incremental
builds). Pull requests only read the cache; pushes to main write it. Credentials are the org
secrets `R2_ACCESS_KEY_ID`/`R2_SECRET_ACCESS_KEY` and the org variables
`SCCACHE_BUCKET`/`SCCACHE_ENDPOINT`.

## Enable

Already enabled in `.github/workflows/ci.yml` (`sccache: true` plus the two R2 secrets, or the
`pyrlyn/ci/.github/actions/sccache` step).

## Disable

Remove `sccache: true` (or the action step). Without the secrets (fork PRs, Dependabot) the
wrapper is turned off automatically and the build runs without a cache.

## Test

Open the job log: the `sccache stats` / `sccache --show-stats` step prints `Compile requests`
and `Cache hits`. A re-run of the same commit should show hits for most crates. 0 requests
means rustc bypassed the wrapper.

## Limitations

- Windows jobs run without the wrapper in `ci-rust.yml`.
- Build-script outputs, proc-macros/bin linking and `cdylib` crates are not cached.
- Incremental compilation is off in jobs that use sccache.
