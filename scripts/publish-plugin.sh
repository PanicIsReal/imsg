#!/usr/bin/env bash
# Copy plugin/ to PanicIsReal/omarchy-imessage as a fast-forward commit.
# omarchy plugin add clones that repo and requires manifest.json at the root.
# Never force-push: installed plugins update with git merge --ff-only.
set -euo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
plugin="$root/plugin"
dest_repo="${PLUGIN_SYNC_REPO:-PanicIsReal/omarchy-imessage}"
branch="${PLUGIN_SYNC_BRANCH:-main}"

fail() {
  echo "publish-plugin: $*" >&2
  exit 1
}

[[ -f "$plugin/manifest.json" ]] || fail "missing $plugin/manifest.json"
"$root/scripts/validate-plugin.sh" "$plugin"

keyfile=""
known=""
workdir=""
cleanup() {
  rm -rf "${workdir:-}"
  rm -f "${keyfile:-}" "${known:-}"
}
trap cleanup EXIT

if [[ -n "${PLUGIN_SYNC_SSH_KEY:-}" ]]; then
  keyfile="$(mktemp)"
  chmod 600 "$keyfile"
  printf '%s\n' "$PLUGIN_SYNC_SSH_KEY" >"$keyfile"
  if [[ -n "${PLUGIN_SYNC_SSH_KNOWN_HOSTS:-}" ]]; then
    known="$(mktemp)"
    printf '%s\n' "$PLUGIN_SYNC_SSH_KNOWN_HOSTS" >"$known"
    export GIT_SSH_COMMAND="ssh -i $keyfile -o UserKnownHostsFile=$known -o StrictHostKeyChecking=yes"
  else
    export GIT_SSH_COMMAND="ssh -i $keyfile -o StrictHostKeyChecking=accept-new"
  fi
  remote="git@github.com:${dest_repo}.git"
else
  remote="https://github.com/${dest_repo}.git"
fi

sha="$(git -C "$root" rev-parse --short HEAD)"
workdir="$(mktemp -d)"

git clone --depth 1 --branch "$branch" -- "$remote" "$workdir"

# Keep .git; replace everything else with plugin/.
find "$workdir" -mindepth 1 -maxdepth 1 ! -name .git -exec rm -rf {} +
cp -a "$plugin"/. "$workdir"/

if git -C "$workdir" diff --quiet && git -C "$workdir" diff --cached --quiet && [[ -z "$(git -C "$workdir" ls-files --others --exclude-standard)" ]]; then
  echo "publish-plugin: $dest_repo already matches plugin/ @$sha"
  exit 0
fi

if [[ -n "${GITHUB_ACTIONS:-}" ]]; then
  git -C "$workdir" config user.name "github-actions[bot]"
  git -C "$workdir" config user.email "41898282+github-actions[bot]@users.noreply.github.com"
fi

git -C "$workdir" add -A
git -C "$workdir" commit -m "chore: sync plugin/ from imsg $sha"
git -C "$workdir" push origin "HEAD:$branch"
echo "publish-plugin: pushed $dest_repo @$sha"
