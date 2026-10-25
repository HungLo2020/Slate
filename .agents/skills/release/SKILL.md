---
name: release
description: Bump Slate's version, update CHANGELOG.md, commit, push to main and wait until every GitHub workflow for that commit succeeds. Use when asked to cut, bump or publish a Slate release, or to "release version X".
---

# Slate release

Cuts a Slate release: bump the workspace version, record it in `CHANGELOG.md`,
commit, push to `main`, and stay with the run until every workflow for that
commit is green. Stop and report rather than guessing whenever a precondition
fails.

## 1. Preconditions

```bash
git rev-parse --abbrev-ref HEAD        # must be main
git status --porcelain                 # must be empty, or contain only the release edits you are about to make
git fetch origin
git rev-parse HEAD origin/main         # must be identical
gh auth status                         # gh must be authenticated with repo scope
```

Fix any failure first: a dirty tree, a detached HEAD, a branch behind or ahead of
`origin/main`, or missing `gh` auth. Never force-push and never amend a commit
that is already pushed.

## 2. Choose the version

`[workspace.package].version` in `Cargo.toml` is the single source of truth.
All four crates (`slate`, `slate-cli`, `slate-core`, `slate-gui`) inherit it, and
`DevUtils/BuildScripts/build-linux-amd64.sh` stamps it into the `.deb`.

```bash
python3 -c 'import tomllib; print(tomllib.load(open("Cargo.toml","rb"))["workspace"]["package"]["version"])'
head -5 CHANGELOG.md
```

- Use the version the task names. Otherwise bump the patch component.
- The new version must be greater than the newest `CHANGELOG.md` heading.
  MattPackages refuses to publish a version that already exists, so never reuse
  one.

## 3. Bump version and lockfile

Replace the one `version = "X.Y.Z"` line in the `[workspace.package]` table:

```bash
python3 - <<'PY'
import pathlib, re
old, new = "0.1.17", "0.1.18"          # set both
path = pathlib.Path("Cargo.toml")
text = path.read_text()
text, count = re.subn(rf'(?m)^version = "{re.escape(old)}"$', f'version = "{new}"', text)
assert count == 1, count
path.write_text(text)
PY
```

Refresh `Cargo.lock` from the manifest (it records the version for each workspace
crate) and confirm all four entries moved:

```bash
cargo metadata --format-version 1 > /dev/null
grep -A1 '^name = "slate' Cargo.lock
```

## 4. Record the release

Add a heading at the top of `CHANGELOG.md`, directly under `# Changelog`:

```markdown
## 0.1.18 — 2026-10-25
```

Use the em dash as existing entries do, and today's date. Add one bullet per
user-visible change, in the existing style: short bold feature names, backticked
paths, no changelog entries for pure refactors. Derive them from the commits
since the previous release:

```bash
git log --oneline "$(git describe --tags --abbrev=0 2>/dev/null || echo HEAD~20)"..HEAD
git log --oneline -20
```

Keep the summary in step 6 consistent with these bullets.

## 5. Keep the docs honest

If the release changes behaviour, commands, settings, paths or requirements,
update `README.md` (and `docs/` or `DESIGN.md` where they document it) in the
same commit, and check that every path the README names still exists. Do not
rewrite old `CHANGELOG.md` entries to match code that moved.

## 6. Validate

Run the full local gate, which is also what CI runs after the smoke tests:

```bash
bash DevUtils/Build.sh
```

It needs an amd64 Linux host with the Qt build dependencies and
`dpkg-deb`. On a machine that cannot build Qt, run the reduced gate and say so in
the final report:

```bash
python3 DevUtils/dependency-policy.py
python3 -m unittest discover -s tests -v
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-targets --all-features
```

Then confirm the tree contains only intended changes (`git status --short`) and
that no build output is staged — `builds/`, `target/` and
`DevUtils/.downloaded/` are ignored and must stay that way.

## 7. Commit

```bash
git add Cargo.toml Cargo.lock CHANGELOG.md README.md   # plus any doc files changed
git commit -m "Release Slate 0.1.18 with <short summary of the headline changes>"
```

Match the history: `Release Slate <version> with <summary>`, imperative, no
trailers unless the repository uses them.

## 8. Push

```bash
git push origin main
sha=$(git rev-parse HEAD)
```

## 9. Wait for every workflow on that commit

Pushing runs **Slate checks** (`.github/workflows/checks.yml`, on `push` and
`pull_request`). **Build Linux amd64 DEB** is `workflow_dispatch` only, so it
does not run on a push; do not wait for it, and mention that it was not
triggered.

Poll until no run for `$sha` is still queued or in progress:

```bash
gh run list --commit "$sha" --limit 20 \
  --json databaseId,name,status,conclusion \
  --jq '.[] | [.databaseId,.name,.status,.conclusion] | @tsv'
```

Runs can take a minute to appear after the push. Repeat the listing until at
least **Slate checks** is present and every listed run reports
`status = completed`.

Then confirm the conclusions, and follow any run that is still going:

```bash
gh run watch <databaseId> --exit-status     # per in-progress run
```

Success means every run for `$sha` has `conclusion = success` (skipped or
neutral jobs are acceptable only if they were already skipping before the
release). A `cancelled` run is a failure unless the cancellation was yours.

If anything fails:

```bash
gh run view <databaseId> --log-failed
```

Fix the cause, re-run the gate from step 6, and push a **new** commit (never
amend a pushed commit). Update the version only if the fix changes released
behaviour enough to justify it. Re-check every run for the new `$sha`.

## 10. Report

Give the version, the commit SHA, the link to the **Slate checks** run, the gate
results, anything skipped (for example the manual DEB workflow, or a reduced
gate on a host without Qt), and any workflow that is still running.

## Do not

- Publish to MattPackages. `DevUtils/PublishMattOSPackage.py publish` is a
  separate, explicitly requested step; it requires clean, committed, pushed
  `main`, a passing latest **Slate checks** push run for that exact commit, and a
  package embedding that commit. `--dry-run` is safe when asked for.
- Tag or create a GitHub release unless the task asks for it.
- Push to a branch other than `main`, or force-push.
- Bump the version for unreleased work that the task did not ask to release.
