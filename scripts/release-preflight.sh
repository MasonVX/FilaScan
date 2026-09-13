#!/usr/bin/env bash
set -euo pipefail

repo_dir="$(cd "$(dirname "$0")/.." && pwd)"
version="$(sed -n 's/^version = "\(.*\)"/\1/p' "$repo_dir/core/Cargo.toml" | head -1)"
expected_version="${1:-$version}"
tag="filascan-v$expected_version"

if [[ ! "$expected_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Release version must use MAJOR.MINOR.PATCH: $expected_version" >&2
  exit 1
fi
if [[ "$version" != "$expected_version" ]]; then
  echo "core/Cargo.toml contains $version, expected $expected_version." >&2
  exit 1
fi

lock_version="$(awk '$0 == "name = \"FilaScan\"" { getline; sub(/^version = \"/, ""); sub(/\"$/, ""); print; exit }' "$repo_dir/core/Cargo.lock")"
if [[ "$lock_version" != "$version" ]]; then
  echo "core/Cargo.lock contains FilaScan $lock_version, expected $version." >&2
  exit 1
fi
if ! grep -Eq "^## $version - [0-9]{4}-[0-9]{2}-[0-9]{2}$" "$repo_dir/CHANGELOG.md"; then
  echo "CHANGELOG.md needs a dated heading for version $version." >&2
  exit 1
fi
if [[ ! -f "$repo_dir/.github/releases/$tag.md" ]]; then
  echo "Missing release notes: .github/releases/$tag.md" >&2
  exit 1
fi
if git -C "$repo_dir" rev-parse -q --verify "refs/tags/$tag" >/dev/null; then
  echo "Release tag already exists locally: $tag" >&2
  exit 1
fi

dirty_release="$(git -C "$repo_dir" status --porcelain --untracked-files=all -- core shared formats CHANGELOG.md ".github/releases/$tag.md" .github/workflows/firmware.yml scripts/build-firmware.sh scripts/package-firmware.sh scripts/release-preflight.sh)"
if [[ -n "$dirty_release" ]]; then
  echo "Release inputs must be committed before tagging:" >&2
  printf '%s\n' "$dirty_release" >&2
  exit 1
fi

if git -C "$repo_dir" rev-parse -q --verify refs/remotes/origin/main >/dev/null; then
  head_commit="$(git -C "$repo_dir" rev-parse HEAD)"
  main_commit="$(git -C "$repo_dir" rev-parse refs/remotes/origin/main)"
  if [[ "$head_commit" != "$main_commit" ]]; then
    echo "HEAD must match the locally known origin/main before release tagging." >&2
    exit 1
  fi
fi

cargo test --manifest-path "$repo_dir/formats/Cargo.toml" --locked
"$repo_dir/scripts/build-firmware.sh"

echo "Release preflight passed for $tag"
