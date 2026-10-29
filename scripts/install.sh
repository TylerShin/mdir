#!/bin/sh
set -eu

usage() {
  echo 'Usage: install.sh [--local] [--repo OWNER/REPO] [--version vVERSION] [--dir DIRECTORY]'
}

local_install=false
repository=${MDIR_REPO:-}
version=latest
install_dir=${MDIR_INSTALL_DIR:-"$HOME/.local/bin"}
print_target=false
while [ "$#" -gt 0 ]; do
  case "$1" in
    --local) local_install=true; shift ;;
    --repo|--version|--dir)
      [ "$#" -ge 2 ] || { usage >&2; exit 2; }
      case "$1" in
        --repo) repository=$2 ;;
        --version) version=$2 ;;
        --dir) install_dir=$2 ;;
      esac
      shift 2 ;;
    --print-target) print_target=true; shift ;;
    -h|--help) usage; exit 0 ;;
    *) usage >&2; exit 2 ;;
  esac
done

case "$(uname -s)/$(uname -m)" in
  Darwin/arm64) target=aarch64-apple-darwin ;;
  Darwin/x86_64) target=x86_64-apple-darwin ;;
  Linux/x86_64) target=x86_64-unknown-linux-musl ;;
  Linux/aarch64|Linux/arm64) target=aarch64-unknown-linux-musl ;;
  *) echo 'Unsupported platform. On Windows use install.ps1.' >&2; exit 1 ;;
esac
if "$print_target"; then echo "$target"; exit 0; fi

temporary=$(mktemp -d)
trap 'rm -rf "$temporary"' EXIT HUP INT TERM
if "$local_install"; then
  project=$(CDPATH= cd -- "$(dirname -- "$0")/.." && pwd)
  source_binary="$project/target/release/mdir"
  [ -f "$source_binary" ] || { echo 'Build first: mise run build' >&2; exit 1; }
  cp "$source_binary" "$temporary/mdir"
else
  # The repository is explicit until this project has a published release.
  printf '%s\n' "$repository" | LC_ALL=C awk '/^[A-Za-z0-9_.-]+\/[A-Za-z0-9_.-]+$/ {ok=1} END {exit !ok}' || {
    echo 'Supply --repo OWNER/REPO for a published mDir release.' >&2; exit 2;
  }
  case "$version" in
    latest) base="https://github.com/$repository/releases/latest/download" ;;
    v[0-9]*)
      case "$version" in *[!A-Za-z0-9._-]*) echo 'Invalid version' >&2; exit 2 ;; esac
      base="https://github.com/$repository/releases/download/$version" ;;
    *) echo 'Version must be latest or vVERSION' >&2; exit 2 ;;
  esac
  asset="mdir-$target"
  curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 "$base/$asset" -o "$temporary/mdir"
  curl --fail --show-error --silent --location --proto '=https' --tlsv1.2 "$base/SHA256SUMS" -o "$temporary/checksums"
  expected=$(awk -v name="$asset" '$2 == name { print $1 }' "$temporary/checksums")
  [ "${#expected}" -eq 64 ] || { echo 'Missing or ambiguous release checksum' >&2; exit 1; }
  case "$expected" in *[!0-9a-fA-F]*) echo 'Invalid checksum' >&2; exit 1 ;; esac
  if command -v sha256sum >/dev/null 2>&1; then
    actual=$(sha256sum "$temporary/mdir" | awk '{print $1}')
  else
    actual=$(shasum -a 256 "$temporary/mdir" | awk '{print $1}')
  fi
  [ "$actual" = "$expected" ] || { echo 'Checksum mismatch; nothing installed' >&2; exit 1; }
fi
if [ -d "$install_dir/mdir" ]; then
  echo 'Install destination is a directory; nothing installed' >&2
  exit 1
fi
mkdir -p "$install_dir"
staged=$(mktemp "$install_dir/.mdir-install.XXXXXX")
trap 'rm -rf "$temporary"; rm -f "$staged"' EXIT HUP INT TERM
cp "$temporary/mdir" "$staged"
chmod 755 "$staged"
mv -f "$staged" "$install_dir/mdir"
printf 'Installed: %s/mdir\n' "$install_dir"
case ":$PATH:" in
  *":$install_dir:"*) echo 'Run: mdir' ;;
  *) printf 'Run: "%s/mdir"\nAdd this directory to PATH to use mdir anywhere.\n' "$install_dir" ;;
esac
