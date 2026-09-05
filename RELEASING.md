# Releasing

Releases are built and published by `cargo-dist`. `.github/workflows/release.yml`
is **generated** — never hand-edit it; change `dist-workspace.toml` and run
`dist generate`.

## One-time setup

1. **Install the tool.** Note the binary is `dist`, not `cargo-dist`, so
   `cargo dist ...` fails with "no such command":

   ```sh
   cargo install cargo-dist --locked
   dist --version
   ```

2. **Create the tap repository:** `github.com/JKrag/homebrew-tap`, public. It must
   exist before the first release — cargo-dist pushes to it, it does not create
   it.

   **It must have at least one commit.** A repository created with no README and
   no initial commit has no branches at all, and `actions/checkout` in the
   publish job fails with `fatal: couldn't find remote ref refs/heads/master`.
   GitHub's API reports a `default_branch` for an empty repository even though no
   such ref exists, so the error looks like a branch-name mismatch and is not one.
   This happened on the first release of `v1.0.0-beta.1`; adding a README and
   re-running the failed job was the whole fix.

   Tick "Add a README file" when creating it, or push one commit afterwards.

3. **Create `HOMEBREW_TAP_TOKEN`.** The release workflow needs a token with write
   access to the tap repo, because the default `GITHUB_TOKEN` is scoped to this
   repository only. A fine-grained PAT limited to `JKrag/homebrew-tap` with
   Contents: read & write is the least-privilege option. Add it under
   *Settings → Secrets and variables → Actions* in **this** repo, named
   `HOMEBREW_TAP_TOKEN`.

## The release workflow is generated — don't edit it

`.github/workflows/release.yml` is produced by `dist generate`, and the `plan`
job checks on every PR that the file on disk still matches what cargo-dist would
generate. Editing it by hand fails that check.

This is also why `.github/dependabot.yml` has no `github-actions` ecosystem.
Dependabot scopes by directory, not by file, so it cannot bump the actions in the
hand-written `ci.yml` without also bumping them in the generated `release.yml` —
which fails `plan`, and which `dist generate` would revert anyway. It fired on the
first Dependabot batch (PR #6). The consequence is that action versions in
`ci.yml` are now bumped by hand; in `release.yml` they move when you upgrade
cargo-dist:

```sh
cargo install cargo-dist --locked   # newer dist
dist init --yes                     # updates cargo-dist-version, regenerates
```

## Cutting a release

1. Bump `version` in `[workspace.package]` in the root `Cargo.toml`. All crates
   inherit it — they ship as one product from one tag, so they move together.
2. Update `CHANGELOG.md`: move `Unreleased` items under the new version.
3. `make check` must be green.
4. `dist plan` — sanity-check the artifact list before anything is pushed.
5. Commit, then tag and push:

   ```sh
   git tag v1.0.0-beta.1
   git push origin v1.0.0-beta.1
   ```

   The tag is what triggers the release. A **version-only** tag (`v1.0.0-beta.1`)
   announces every crate at that version together, which is what we want; a
   package-qualified tag (`swab-v1.0.0`) would release just that one.

   Note this repo's remote quirk: pushes need the explicit SSH URL,
   `git@github.com:JKrag/petridish.git`.

## What gets published

Three Homebrew formulae, because cargo-dist builds one app per package and the
crates cannot merge (ADR-0002 keeps `swab-hook` out of a dependency tree
containing ratatui, and keeps the read/write boundary compiler-enforced):

| Formula | Binaries |
| --- | --- |
| `petridish` | `petridish` |
| `swab` | `swab`, `swab-hook` |
| `petri` | `petri` |

`petridish` `depends_on` the other two, so users only ever type one command:

```sh
brew install jkrag/tap/petridish
```

That relationship is declared in `petridish-cli/Cargo.toml` under
`[package.metadata.dist.dependencies.homebrew]`. **`stage = ["run"]` is required
there** — the default stage is build-only, which produces no `depends_on` line in
the formula at all, and the failure is silent: the formula generates fine and
simply installs one third of the product.

## Verifying a release

On a machine that has never had petridish installed:

```sh
brew install jkrag/tap/petridish
petridish install
petridish doctor          # every check should pass
launchctl list | grep petridish
petri                     # should render real data within a minute
```

Then check the hook landed **alongside** any pre-existing entries rather than
replacing them:

```sh
grep -c 'petridish' ~/.claude/settings.json
```

### Test the upgrade path too

The one failure this pipeline can produce that a fresh install will not show:

```sh
brew upgrade petridish
launchctl list | grep petridish     # must still be running
petridish doctor                    # the `plist` check catches a stale path
```

### If `publish-homebrew-formula` fails

It runs last, so the tarballs and the GitHub Release have already succeeded by
then. **Do not re-tag.** Fix the cause and re-run just that job:

```sh
gh run rerun <run-id> --failed
```

Two causes seen or anticipated: an empty tap repository (above), and a
`HOMEBREW_TAP_TOKEN` scoped to the wrong repository, which fails with a 403 at
the push rather than at checkout.

`petridish install` writes an absolute `swab` path into the launchd plist. It
deliberately keeps Homebrew's stable `/opt/homebrew/bin` symlink rather than
resolving through to the version-stamped Cellar directory, so an upgrade should
not break the daemon. `doctor`'s `plist` check exists to catch it if that ever
stops being true.

## Prereleases

cargo-dist marks a release as a prerelease automatically when the version has a
prerelease component (`1.0.0-beta.1`). Confirm before promoting to `1.0.0` that
Homebrew upgrades cleanly from the beta version string — an installed formula that
will not upgrade is exactly the kind of gap a beta exists to find.
