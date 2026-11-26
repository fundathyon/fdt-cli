#!/bin/sh
# Copies the accounts-integration skill from a checkout of fundathyon/accounts into
# skills/accounts-integration, so `fdt skills` ships the same version as the Accounts docs.
#
#   scripts/sync-accounts-skill.sh ../../services/accounts/accounts [ref]
#
# ref defaults to origin/develop. The skill's README.md (how to copy it by hand) is left out:
# with fdt it is installed with `fdt skills add accounts-integration`.
set -eu

accounts="${1:?uso: scripts/sync-accounts-skill.sh <checkout de accounts> [ref]}"
ref="${2:-origin/develop}"
here="$(cd "$(dirname "$0")/.." && pwd)"
dest="${here}/skills/accounts-integration"

git -C "$accounts" rev-parse --verify --quiet "${ref}^{commit}" >/dev/null ||
  { echo "no existe ${ref} en ${accounts}; haz git fetch primero." >&2; exit 1; }

tmp="$(mktemp -d 2>/dev/null || mktemp -d -t fdt)"
trap 'rm -rf "$tmp"' EXIT
git -C "$accounts" archive "$ref" .agents/accounts-integration | tar -x -C "$tmp"
src="${tmp}/.agents/accounts-integration"
[ -f "${src}/SKILL.md" ] || { echo "${ref} no tiene .agents/accounts-integration/SKILL.md." >&2; exit 1; }
rm -f "${src}/README.md"

rm -rf "$dest"
mkdir -p "$(dirname "$dest")"
cp -R "$src" "$dest"
echo "accounts-integration $(awk '/^  version:/ { gsub(/"/, "", $2); print $2; exit }' "${dest}/SKILL.md") ← ${ref} ($(git -C "$accounts" rev-parse --short "$ref"))"
