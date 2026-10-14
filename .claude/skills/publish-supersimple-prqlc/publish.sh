#!/usr/bin/env bash
# Builds prqlc/bindings/js from the gosupersimple/prql fork and publishes it to
# GitHub Packages as @gosupersimple/prqlc. See SKILL.md next to this file.
#
# Usage: publish.sh [--dry-run] [--ref <git-ref>] [--version <version>]

set -euo pipefail

PACKAGE='@gosupersimple/prqlc'
REGISTRY='https://npm.pkg.github.com'

dry_run=false
ref=''
version=''

while [[ $# -gt 0 ]]; do
  case "$1" in
    --dry-run) dry_run=true ;;
    --ref) ref="$2"; shift ;;
    --version) version="$2"; shift ;;
    -h|--help) sed -n '2,5p' "$0"; exit 0 ;;
    *) echo "Unknown argument: $1" >&2; exit 2 ;;
  esac
  shift
done

fail() {
  echo "error: $*" >&2
  exit 1
}

for cmd in git gh node npm wasm-pack; do
  command -v "$cmd" >/dev/null || fail "$cmd is not installed"
done
rustup target list --installed | grep -q '^wasm32-unknown-unknown$' \
  || fail 'missing Rust target: rustup target add wasm32-unknown-unknown'

npm whoami --registry="$REGISTRY" >/dev/null 2>&1 \
  || fail "not authenticated to $REGISTRY. ~/.npmrc needs
  @gosupersimple:registry=$REGISTRY/
  //npm.pkg.github.com/:_authToken=\${GITHUB_TOKEN}
with a GITHUB_TOKEN that has write:packages."

root="$(git rev-parse --show-toplevel)"
remote="$(git -C "$root" remote -v \
  | awk '$2 ~ /gosupersimple\/prql(\.git)?$/ && $3 == "(fetch)" { print $1; exit }')"
[[ -n "$remote" ]] || fail 'no git remote points at gosupersimple/prql'

git -C "$root" fetch --quiet "$remote"
ref="${ref:-$remote/main}"
sha="$(git -C "$root" rev-parse --short "$ref^{commit}")"

base_version="$(git -C "$root" show "$sha:Cargo.toml" \
  | sed -n 's/^version = "\(.*\)"$/\1/p' | head -1)"
[[ -n "$base_version" ]] || fail "could not read workspace version at $sha"

published="$(gh api --paginate '/orgs/gosupersimple/packages/npm/prqlc/versions' \
  --jq '.[].name')"

if [[ -z "$version" ]]; then
  last_rc="$(grep -E "^${base_version//./\\.}-rc\.[0-9]+$" <<<"$published" \
    | sed 's/.*-rc\.//' | sort -n | tail -1 || true)"
  version="$base_version-rc.$(( ${last_rc:--1} + 1 ))"
fi

grep -qxF "$version" <<<"$published" && fail "$PACKAGE@$version is already published"

worktree="$(mktemp -d "${TMPDIR:-/tmp}/prqlc-publish.XXXXXX")"
cleanup() {
  git -C "$root" worktree remove --force "$worktree" >/dev/null 2>&1 || true
  rm -rf "$worktree"
}
trap cleanup EXIT

label=''
$dry_run && label=' [dry run]'
echo "==> $PACKAGE@$version from $ref ($sha)$label"
git -C "$root" worktree add --quiet --detach "$worktree" "$sha"

cd "$worktree/prqlc/bindings/js"

PKG_NAME="$PACKAGE" PKG_VERSION="$version" PKG_REGISTRY="$REGISTRY" node -e '
  const fs = require("fs");
  const pkg = JSON.parse(fs.readFileSync("package.json", "utf8"));
  pkg.name = process.env.PKG_NAME;
  pkg.version = process.env.PKG_VERSION;
  pkg.publishConfig = { registry: process.env.PKG_REGISTRY };
  fs.writeFileSync("package.json", JSON.stringify(pkg, null, 2) + "\n");
'

echo '==> Installing build dependencies'
npm ci --ignore-scripts --no-audit --no-fund --loglevel=error

echo '==> Building wasm (node, web, bundler)'
CARGO_TARGET_DIR="${CARGO_TARGET_DIR:-$root/target}" PROFILE=release npm run --silent build

echo '==> Smoke testing the node build'
node -e '
  const prqlc = require("./dist/node/prqlc_js.js");
  const compile = (target) => {
    const opts = new prqlc.CompileOptions();
    opts.format = false;
    opts.signature_comment = false;
    return prqlc.compile(`prql target:sql.${target}\nfrom t | derive x = [1, 2, 3]`, opts);
  };
  const expected = {
    redshift: "SELECT *, ARRAY(1, 2, 3) AS x FROM t",
    snowflake: "SELECT *, ARRAY_CONSTRUCT(1, 2, 3) AS \"x\" FROM \"t\"",
    postgres: "SELECT *, ARRAY[1, 2, 3] AS x FROM t",
  };
  for (const [target, sql] of Object.entries(expected)) {
    const actual = compile(target).trim();
    if (actual !== sql) {
      console.error(`${target}: expected\n  ${sql}\ngot\n  ${actual}`);
      process.exit(1);
    }
    console.log(`  ${target}: ${actual}`);
  }
'

echo "==> Publishing to $REGISTRY"
publish_args=(--ignore-scripts)
$dry_run && publish_args+=(--dry-run)
npm publish "${publish_args[@]}"

if $dry_run; then
  echo "==> Dry run finished; nothing was published."
  exit 0
fi

gh api --paginate '/orgs/gosupersimple/packages/npm/prqlc/versions' --jq '.[].name' \
  | grep -qxF "$version" || fail "$version does not show up in GitHub Packages"

cat <<EOF
==> Published $PACKAGE@$version ($sha)

Next, in supersimple-api:
  yarn up $PACKAGE@^$version
EOF
