---
name: filascan-release
description: Prepare, validate, tag, publish, or verify a FilaScan firmware release and GitHub-hosted OTA channel. Use for version bumps, release notes, release CI, GitHub Releases, and OTA publication; do not use for ordinary local flashing.
---

# FilaScan Release

Read `.github/workflows/firmware.yml`, `CHANGELOG.md`, and the current release-note pattern before preparing a release. GitHub Actions is the sole publisher of release and OTA artifacts.

## Prepare

- Choose a new semantic version unless the user explicitly says an unpublished version may be retained.
- Keep `core/Cargo.toml`, the root `FilaScan` entry in `core/Cargo.lock`, `CHANGELOG.md`, and `.github/releases/filascan-v<VERSION>.md` consistent.
- Release notes describe user-visible behavior and relevant upgrade/recovery details. Do not claim tests that were not run.
- Commit release inputs before tagging. Preserve unrelated worktree changes.
- Ensure the release commit is the commit reachable as `origin/main`; never publish from an unmerged feature-only commit.

## Validate

- Fetch `origin/main` and tags before checking remote state.
- Run `./scripts/release-preflight.sh <VERSION>`. It validates metadata, committed inputs, the locally known `origin/main`, decoder tests, and a complete firmware build/package.
- Confirm `filascan-v<VERSION>` does not exist locally or remotely.
- Read [references/publishing.md](references/publishing.md) before creating or troubleshooting a release.

## Publish

- Tagging and pushing are external mutations. Perform them only when the user asked to publish/release or separately authorized them.
- Create an annotated `filascan-v<VERSION>` tag on the validated commit and push only that tag. Never move, delete, or recreate an existing release tag without explicit user direction.
- Do not call `gh release create` manually. The tag-triggered workflow builds artifacts, creates the GitHub Release, and deploys the OTA channel.
- After pushing the tag, check once that the tag-triggered GitHub workflow has started or is queued. Report its link and finish; do not watch or repeatedly poll for completion. Do not set up background monitoring. Only check completion, release assets, and the live OTA manifest when the user explicitly requests that verification.
