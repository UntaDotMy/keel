#!/usr/bin/env bash
# Purpose: Bootstrap claude-skills from the latest GitHub release without a manual archive download.
# Caller: macOS, Linux, and WSL users running the documented one-line installer.
# Dependencies: curl, tar, uname, mktemp, and the claude-skills GitHub release assets.
# Main Functions: Detect platform, download a release archive to temp, extract it, run install, and verify status.
# Side Effects: Writes the managed claude-skills surface under ~/.claude and removes temporary download files.

set -euo pipefail

repository="${CLAUDE_SKILLS_REPOSITORY:-UntaDotMy/claude_core}"
version="${CLAUDE_SKILLS_VERSION:-latest}"

need_command() {
  if ! command -v "$1" >/dev/null 2>&1; then
    printf 'claude-skills installer requires %s\n' "$1" >&2
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
    -H 'User-Agent: claude-skills-installer' \
    "https://api.github.com/repos/${repository}/releases/latest" |
    sed -n 's/.*"tag_name"[[:space:]]*:[[:space:]]*"\([^"]*\)".*/\1/p' |
    head -n 1
}

need_command curl
need_command tar
need_command mktemp

os="$(detect_os)"
arch="$(detect_arch)"

if [ "$version" = "latest" ]; then
  release_tag="$(latest_release_tag)"
  if [ -z "$release_tag" ]; then
    printf 'Unable to resolve latest claude-skills release for %s\n' "$repository" >&2
    exit 1
  fi
else
  release_tag="$(normalize_tag "$version")"
fi

asset_version="$(asset_version_from_tag "$release_tag")"
archive_name="claude-skills_${asset_version}_${os}_${arch}.tar.gz"
download_url="https://github.com/${repository}/releases/download/${release_tag}/${archive_name}"
temporary_directory="$(mktemp -d "${TMPDIR:-/tmp}/claude-skills-install.XXXXXX")"

cleanup() {
  rm -rf "$temporary_directory"
}
trap cleanup EXIT

archive_path="${temporary_directory}/${archive_name}"
extract_directory="${temporary_directory}/extract"
mkdir -p "$extract_directory"

printf 'Downloading claude-skills %s for %s-%s...\n' "$release_tag" "$os" "$arch"
curl -fL --retry 3 --retry-delay 2 -o "$archive_path" "$download_url"

tar -xzf "$archive_path" -C "$extract_directory"

installer_binary="${extract_directory}/claude-skills"
if [ ! -x "$installer_binary" ]; then
  installer_binary="$(find "$extract_directory" -type f -name claude-skills -perm /111 | head -n 1)"
fi
if [ -z "$installer_binary" ] || [ ! -x "$installer_binary" ]; then
  printf 'Release archive did not contain an executable claude-skills binary.\n' >&2
  exit 1
fi

bundle_root="$(cd "$(dirname "$installer_binary")" && pwd)"
"$installer_binary" install --repo-root "$bundle_root"

installed_binary="${HOME}/.claude/claude-skills"
if [ ! -x "$installed_binary" ]; then
  printf 'Installed binary not found at %s\n' "$installed_binary" >&2
  exit 1
fi

"$installed_binary" status --repo-root "$bundle_root"
"$installed_binary" hook install

# Register the claude_core MCP server in Claude Code's user-scope config so the
# recall, system_map, run_command, and recall_status tools are reachable without
# manual setup. `claude mcp add` exits non-zero with the exact message
# "MCP server claude_core already exists in user config" on re-runs — we
# substring-match "already exists" so the install stays idempotent. If
# Anthropic ever changes that wording, the install still completes (the call
# falls through to the warning branch). We suppress errexit around the call so
# the assignment can capture both stdout and stderr without triggering set -e,
# then classify the result. If the `claude` CLI is not on PATH (rare; users
# without Claude Code installed yet), skip the step with a warning rather than
# failing the whole install.
if command -v claude >/dev/null 2>&1; then
  set +e
  mcp_output="$(claude mcp add claude_core --scope user -- "$installed_binary" mcp serve 2>&1)"
  mcp_exit=$?
  set -e
  if [ "$mcp_exit" -eq 0 ]; then
    printf 'Registered claude_core MCP server in Claude Code user config\n'
  elif printf '%s' "$mcp_output" | grep -q 'already exists'; then
    printf 'claude_core MCP server already registered in Claude Code user config\n'
  else
    printf 'Warning: claude mcp add failed (exit %s): %s\n' "$mcp_exit" "$mcp_output" >&2
  fi
else
  printf 'Warning: claude CLI not found on PATH; skipping MCP registration. Run `claude mcp add claude_core --scope user -- %s mcp serve` after installing Claude Code.\n' "$installed_binary" >&2
fi

printf 'claude-skills installed successfully at %s\n' "$installed_binary"
