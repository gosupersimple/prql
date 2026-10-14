---
name: publish-supersimple-prqlc
description:
  Build and publish Supersimple's custom `@gosupersimple/prqlc` npm package (the
  JS/wasm bindings from the gosupersimple/prql fork) to GitHub Packages. Use
  when asked to release, publish or cut a new version of our prqlc / PRQL
  package for supersimple-api.
argument-hint: "[--dry-run] [--ref <git-ref>] [--version <version>]"
---

# Publish `@gosupersimple/prqlc`

supersimple-api depends on `@gosupersimple/prqlc`. It is `prqlc/bindings/js`
built from the `main` branch of the
[gosupersimple/prql](https://github.com/gosupersimple/prql) fork (upstream PRQL
plus our dialect patches), published to GitHub Packages
(`https://npm.pkg.github.com`). There is no CI for it; releases are published
from a developer machine with [`publish.sh`](publish.sh).

The script:

1. Finds the git remote for gosupersimple/prql, fetches it and checks out
   `<remote>/main` (or `--ref`) into a temporary worktree. The local `main`
   branch usually tracks upstream prql/prql and must not be published.
2. Sets `name`, `version` and `publishConfig` in `prqlc/bindings/js/package.json`
   in that worktree only. Nothing is committed.
3. Builds the node, web and bundler wasm targets and smoke tests the node build
   against our fork's array literal output for Redshift, Snowflake and Postgres.
4. Runs `npm publish` and checks that the version shows up in GitHub Packages.

## Versioning

`<workspace version from Cargo.toml>-rc.<N>`, e.g. `0.13.15-rc.0`. The script
picks the next free `N` for the current workspace version. List published
versions with:

```sh
gh api /orgs/gosupersimple/packages/npm/prqlc/versions --jq '.[].name'
```

## Steps

1. Make sure the fork's `main` contains what should ship. If upstream has moved
   on, sync first: rebase the fork-only commits onto upstream `main`, run
   `task prqlc:pull-request` and force-push to the fork's `main`.
2. Dry run, from the repository root:

   ```sh
   .claude/skills/publish-supersimple-prqlc/publish.sh --dry-run
   ```

3. Show the user the package version and fork commit from the dry run output
   and get an explicit go-ahead. Publishing is visible to the whole team and a
   version can't be reused.
4. Publish:

   ```sh
   .claude/skills/publish-supersimple-prqlc/publish.sh
   ```

5. In supersimple-api, `yarn up @gosupersimple/prqlc@^<version>` and run its
   PRQL and engine tests. A caret range on a prerelease does not pick up a
   newer patch version on its own, so the bump has to be explicit. Expect
   snapshot updates where upstream changed SQL formatting.

The build takes a few minutes on a cold cache; it reuses the repository's
`target/` directory unless `CARGO_TARGET_DIR` is set.

## Prerequisites

- `wasm-pack`, the `wasm32-unknown-unknown` Rust target, Node.js/npm and an
  authenticated `gh`.
- `~/.npmrc` with:

  ```ini
  @gosupersimple:registry=https://npm.pkg.github.com/
  //npm.pkg.github.com/:_authToken=${GITHUB_TOKEN}
  ```

  and a `GITHUB_TOKEN` with `write:packages`.

## Troubleshooting

- **401/403 on publish**: the token lacks `write:packages`, or the publisher has
  no write access to the package. The package must be linked to the
  gosupersimple/prql repository (package page → Package settings → link
  repository) so that repository writers can publish.
- **Smoke test fails**: the fork's array literal output changed, or a fork
  patch got lost while syncing with upstream. Compare `prqlc/prqlc/src/sql/dialect.rs`
  with upstream before publishing. If the change is intended, update the
  expected SQL in `publish.sh`.
- **Version already published**: pass a higher `--version`; npm versions can't be
  overwritten.
