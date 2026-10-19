---
name: release
description: Release Drift deterministically — build every artifact (iOS App Store .ipa, Mac App Store .pkg, Drift and Drift Host Developer ID DMGs), notarize, upload to TestFlight, publish a GitHub DMG release. Use when asked to release, ship, cut a release, push a TestFlight build, publish a dmg, or bump the version.
---

# Release Drift

One command does everything: `cargo xtask release` (ADR `docs/adr/RELEASE-pipeline.md`).
It never uploads or publishes unless asked with a flag.

**Lockstep rule:** iOS and macOS ship in lockstep — always released together, with the same `yy.mm` version
and the same build number N (App Store Connect's highest on any platform + 1), and that N is
also in both DMGs. Never upload one platform alone and never pick numbers by hand; there is
no single-platform command. If App Store Connect shows the platforms on different builds
(for example iOS 2, macOS 3), the next release uses the highest + 1 on both. If a release
uploaded one platform and failed on the other, run `--resume`: it uploads the missing one
with the same N.

**Drift and Drift Host ship together:** every release builds all four artifacts (iOS `.ipa`,
Mac App Store `.pkg`, Drift DMG, Drift Host DMG) with the same version and N, even when one of
the apps has not changed since the last release. Never skip or hold back an unchanged app, and
never decide what counts as a material change; `sync-check` fails if any artifact is missing.

## Ask first

Get the user's explicit go-ahead in this conversation before each of these; never infer it:

- **Version bump commit** (`--set-version`, then the commit and push to `main`).
- **TestFlight upload** (`--testflight`): visible to testers.
- **GitHub publish** (`--github`): the GitHub repository is public; tags and releases cannot be
  taken back quietly.

A dry run, preflight and building artifacts need no permission.

## Steps

1. Credentials and keychain (never print the values):
   ```sh
   set -a; . ~/code/term.hut/.env; set +a
   signing-unlock -v
   ```
2. Version (only for a new month or when asked): `cargo xtask release --set-version 26.11`,
   then edit `release/notes/26.11.md` (from `release/notes/TEMPLATE.md`), commit
   `chore(release): 26.11`, push `main`. Build numbers are automatic (App Store Connect's
   highest + 1, shared by iOS and Mac); do not edit them.
3. Notes: `release/notes/<yy.mm>.md` must exist without TODOs. Its `## What to Test` section
   becomes TestFlight's "What to Test"; the whole file plus checksums becomes the GitHub notes.
4. Gate: the pipeline reuses `target/ci-receipts/<HEAD>` from a passing `cargo xtask ci` on a
   clean tree, or runs the gate itself (about 40 minutes).
5. Rehearse: `cargo xtask release --dry-run` (builds and validates all four artifacts; branch
   and push checks are warnings only).
6. Release, with the flags the user approved:
   ```sh
   cargo xtask release                       # build, notarize, checksums
   cargo xtask release --testflight          # + upload to TestFlight, What to Test
   cargo xtask release --testflight --github # + tag v<yy.mm>.<build>, GitHub release
   ```
   Output: `target/release-out/<yy.mm>-<build>/` with the artifacts, `SHA256SUMS`,
   `manifest.json` and `release.log`.

## When a step fails

- Read the failed step and error in `manifest.json` / `release.log`, fix the cause, then
  `cargo xtask release --resume <same flags>`. Finished steps whose outputs still match their
  recorded hashes are skipped; the build number is reused. Network steps already retry three
  times with backoff.
- A new commit means a new release: `--resume` refuses a different commit or version.
- Uploads are idempotent: a build number App Store Connect already has is not uploaded again,
  an existing GitHub release is not recreated.

## Verify

```sh
D=target/release-out/<yy.mm>-<build>
(cd $D && shasum -a 256 -c SHA256SUMS)
xcrun stapler validate "$D"/Drift_*_aarch64.dmg "$D"/Drift-Host_*_aarch64.dmg
spctl -a -t open --context context:primary-signature -vv "$D"/*.dmg
hdiutil attach -nobrowse "$D"/Drift_*_aarch64.dmg   # then:
codesign --verify --deep --strict -vv /Volumes/Drift/Drift.app && spctl -a -vv /Volumes/Drift/Drift.app
```
iOS and Mac App Store packages are validated by `altool --validate-app` inside the pipeline.

## Clean up / roll back

- Disk: `rm -rf target/release-out/<dir> src-tauri/gen/apple/build target/*/release/bundle`
  after the release (builds need ~40 GB free).
- A TestFlight build cannot be deleted; expire it in App Store Connect and ship the next build.
- A wrong GitHub release: `gh release delete v<yy.mm>.<build> --repo HutsonLabs/drift` and
  `git push github :refs/tags/v<yy.mm>.<build>` (and `origin`) — only with the user's consent.
