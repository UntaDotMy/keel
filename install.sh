#!/usr/bin/env bash
# Purpose: Bootstrap keel from the latest GitHub release without a manual archive download.
# Caller: macOS, Linux, and WSL users running the documented one-line installer.
# Dependencies: curl, tar, uname, mktemp, and the keel GitHub release assets.
# Main Functions: Detect platform, download a release archive to temp, extract it, run install, and verify status.
# Side Effects: Writes keel's host-neutral home under ~/.keel (binary, data, state), the Claude harness engagement files under ~/.claude (skills, agents, commands, settings.json), and removes temporary download files.

set -euo pipefail

repository="${CLAUDE_SKILLS_REPOSITORY:-UntaDotMy/keel}"
version="${CLAUDE_SKILLS_VERSION:-latest}"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'keel installer requires %s\n' "$1" >&2
    exit 1
  fi
}

normalize_tag() {
  case "$1" in
    v*|bootstrap-*) printf '%s\n' "$1" ;;
    [0-9]*) printf 'v%s\n' "$1" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

asset_version_from_tag() {
  case "$1" in
    v[0-9]*) printf '%s\n' "${1#v}" ;;
    *) printf '%s\n' "$1" ;;
  esac
}

detect_os() {
  case "$(uname -s)" in
    Darwin) printf 'darwin\n' ;;
    Linux) printf 'linux\n' ;;
    *) printf 'Unsupported operating system: %s\n' "$(uname -s)" >&2; exit 1 ;;
  esac
}

detect_arch() {
  case "$(uname -m)" in
    x86_64|amd64) printf 'amd64\n' ;;
    arm64|aarch64) printf 'arm64\n' ;;
    *) printf 'Unsupported architecture: %s\n' "$(uname -m)" >&2; exit 1 ;;
  esac
}

latest_release_tag() {
  curl -fsSL \
    -H 'Accept: application/vnd.github+json' \
    -H 'User-Agent: keel-installer' \
    "https://api.github.com/repos/${repository}/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

need_command curl
need_command tar
need_command mktemp

verify_sha256() {
  local checksum_path=$1
  local target_path=$2
  local expected actual
  expected="$(awk 'NF { print tolower($1); exit }' "$checksum_path")"
  if [[ ! "$expected" =~ ^[0-9a-f]{64}$ ]]; then
    printf 'Invalid SHA-256 checksum for %s\n' "$target_path" >&2
    exit 1
  fi
  if command -v sha256sum >/dev/null 2>&1; then
    actual="$(sha256sum "$target_path" | awk '{print tolower($1)}')"
  elif command -v shasum >/dev/null 2>&1; then
    actual="$(shasum -a 256 "$target_path" | awk '{print tolower($1)}')"
  else
    printf 'No SHA-256 verifier (sha256sum or shasum) is available.\n' >&2
    exit 1
  fi
  if [ "$actual" != "$expected" ]; then
    printf 'SHA-256 mismatch for %s\n' "$target_path" >&2
    exit 1
  fi
}


os="$(detect_os)"
arch="$(detect_arch)"

if [ "$version" = "latest" ]; then
  release_tag="$(latest_release_tag)"
  if [ -z "$release_tag" ]; then
    printf 'Unable to resolve latest keel release for %s\n' "$repository" >&2
    exit 1
  fi
else
  release_tag="$(normalize_tag "$version")"
fi

asset_version="$(asset_version_from_tag "$release_tag")"
archive_name="keel_${asset_version}_${os}_${arch}.tar.gz"
download_url="https://github.com/${repository}/releases/download/${release_tag}/${archive_name}"
temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/keel-install.XXXXXX")"

cleanup() {
  rm -rf "$temporary_directory"
}
trap cleanup EXIT

archive_path="${temporary_directory}/${archive_name}"
checksum_path="${temporary_directory}/${archive_name}.sha256"
extract_directory="${temporary_directory}/extract"
mkdir -p "$extract_directory"

printf 'Downloading keel %s for %s-%s...\n' "$release_tag" "$os" "$arch"
curl -fL --retry 3 --retry-delay 2 -o "$archive_path" "$download_url"
curl -fL --retry 3 --retry-delay 2 -o "$checksum_path" "${download_url}.sha256"
verify_sha256 "$checksum_path" "$archive_path"
printf 'Verified SHA-256 for %s.\n' "$archive_name"

tar -xzf "$archive_path" -C "$extract_directory"

installer_binary="${extract_directory}/keel"
if [ ! -x "$installer_binary" ]; then
  # why: `-perm /111` is GNU-only and BSD find (macOS) errors on it; the `-x`
  # test below is the real executability check anyway.
  installer_binary="$(find "$extract_directory" -type f -name keel | head -n 1)"
fi
if [ -z "$installer_binary" ] || [ ! -x "$installer_binary" ]; then
  printf 'Release archive did not contain an executable keel binary.\n' >&2
  exit 1
fi

bundle_root="$(cd "$(dirname "$installer_binary")" && pwd)"
"$installer_binary" install --repo-root "$bundle_root"

# The binary publishes to the host-neutral ~/.keel home; fall back to the
# legacy ~/.claude placement for pre-migration installs. KEEL_HOME overrides.
keel_home="${KEEL_HOME:-${HOME}/.keel}"
installed_binary="${keel_home}/keel"
if [ ! -x "$installed_binary" ]; then
  legacy_binary="${HOME}/.claude/keel"
  if [ -x "$legacy_binary" ]; then
    installed_binary="$legacy_binary"
  else
    printf 'Installed binary not found at %s (or legacy ~/.claude/keel)\n' "$installed_binary" >&2
    exit 1
  fi
fi

"$installed_binary" status --repo-root "$bundle_root"
# Native `keel install` (above) already wires the lifecycle hooks into
# settings.json, but does so best-effort (a failure is reported, not fatal). This
# explicit re-run is the bootstrap's verification gate: it is idempotent, and it
# turns a hook-wiring failure into a hard install error (set -e) so a broken
# engagement surface never ships silently.
"$installed_binary" hook install

# MCP registration is handled natively by `keel install` above, which
# writes the keel entry into ~/.claude.json *with* `alwaysLoad: true`
# (see rust/.../manager/mcp_register.rs). That flag pins the recall, system_map,
# run_command, and recall_status tools into context instead of leaving them
# deferred behind ToolSearch. We deliberately do NOT shell out to `claude mcp
# add` here: that command cannot set `alwaysLoad`, so it would register the
# server in a degraded (deferred) state, and it requires the `claude` CLI on
# PATH. The native path needs neither and is the single source of truth. Run
# `keel doctor` to confirm the entry and `alwaysLoad`, or
# `keel repair` to re-register if anything looks off.

printf 'keel installed successfully at %s\n' "$installed_binary"
