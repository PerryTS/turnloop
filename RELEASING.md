# Releasing turnloop

The public repository is **PerryTS/turnloop**, default branch **main**. CI runs on
every PR and main push. Owners make the initial crates.io publications; subsequent
releases use OIDC and the protected `crates-io` GitHub environment. No crates.io
API token belongs in repository secrets.

## Owner bootstrap

These are owner actions, not actions performed by the CI lane. Execute commands
from a clean, committed checkout, on a machine that can run its native tests.

1. Protect `main` with required PR review and the single required Actions check
   **`ci-gate`**. Allow Actions to create pull requests. Require the check from
   GitHub Actions, not a similarly named status supplied by another integration.
2. In Settings → Environments, create **`crates-io`**. Add at least one required
   reviewer, prevent self-review, disable administrator bypass where available,
   and permit deployments only from `main`. Merely naming an environment in YAML
   does **not** install these protections. Review the exact release SHA before
   approving a deployment.
3. Install a GitHub App on **only PerryTS/turnloop**, granting Contents: write and
   Pull requests: write. Store its ID as repository variable `RELEASE_APP_ID` and
   its private key as `RELEASE_APP_PRIVATE_KEY`. The workflow mints a short-lived,
   repository-scoped installation token. This lets release-plz PR pushes trigger
   ordinary CI; PRs opened with `GITHUB_TOKEN` do not trigger it automatically.
   This key authorizes GitHub PR work; it is not a crates.io credential.
4. Verify every intended library's package metadata (`description`, MIT license,
   repository URL and README), versioned path dependencies, and publication flag.
   Contract/bench helper crates should have `publish = false`. `spikes/*` must
   not be workspace members. Names and dependency order come from Cargo metadata:

   ```bash
   rustup toolchain install nightly-2026-08-20 --profile minimal --component rustfmt,clippy
   rustup toolchain install nightly-2026-09-07 --profile minimal
   python3 scripts/ci/release.py order
   python3 scripts/ci/soak.py
   bash scripts/ci/no-tokio.sh
   cargo +nightly-2026-08-20 test --locked --workspace -- --test-threads=1
   cargo +nightly-2026-08-20 publish --registry crates-io --locked --workspace --dry-run
   ```

   The printed order is topological: core and TLS before clients depending on
   them, and HTTP before any client depending on HTTP. There is no stale crate
   list to maintain after a rename. Cargo's workspace dry run packages and builds
   **each** publishable member, using a temporary registry for unpublished sibling
   versions. Do not replace it with separate per-crate dry runs or `--no-verify`.
5. On crates.io create a short-lived API token scoped to creating/publishing the
   intended crate names. **The first publish of every new crate name is manual**;
   OIDC cannot bootstrap an unowned name. In a Bash shell:

   ```bash
   read -r -s -p 'Temporary crates.io bootstrap token: ' CARGO_REGISTRY_TOKEN
   echo
   export CARGO_REGISTRY_TOKEN
   cargo +nightly-2026-08-20 publish --registry crates-io --locked --workspace
   unset CARGO_REGISTRY_TOKEN
   ```

   Cargo publishes the metadata-discovered members in dependency order and waits
   for registry availability. `publish = false` helpers are excluded. The same
   process applies to newly added libraries later; if some workspace versions
   already exist, use `-p NAME` for each unpublished name from `release.py order`
   **in one Cargo invocation**, instead of `--workspace`. Keep new sibling
   dependencies in that invocation. Never disable the repository soak.
6. For **each published crate**, open crates.io → crate settings → Trusted
   Publishing → add GitHub publisher. Enter owner **PerryTS**, repository
   **turnloop**, workflow filename **`release.yml`**, and environment
   **`crates-io`**. Repeat for every new crate added later. The OIDC action is
   `rust-lang/crates-io-auth-action`; it obtains a temporary token just before the
   upload and revokes it during its post step.
7. Revoke the bootstrap API token in crates.io → Account settings → API tokens
   immediately, even if bootstrap failed after a partial publish. Remove any
   local credential store entry if you used `cargo login` (`cargo logout`).
8. Create initial `<crate>-v<version>` tags and GitHub Releases at the source
   commit, with the crate's initial changelog. Only create these after confirming
   publication. This example uses the dynamically generated list, excluding
   non-publishable helpers; run once after a complete initial bootstrap:

   ```bash
   release_sha=$(git rev-parse HEAD)
   python3 scripts/ci/release.py order > .tools/bootstrap-order.tsv
   while IFS=$'\t' read -r crate version manifest; do
     gh release create "$crate-v$version" --repo PerryTS/turnloop \
       --target "$release_sha" --title "$crate-v$version" \
       --notes-file "$(dirname "$manifest")/CHANGELOG.md"
   done < .tools/bootstrap-order.tsv
   ```

## Automated releases

`release.yml` follows successful **CI** main-push runs. It rejects PR runs and
foreign repositories. Its read-only verification job queries the Actions API for
that workflow run's exact SHA, path, event, latest attempt and **successful
`ci-gate` job**. A successful/skipped workflow by itself is insufficient.

The release-plz job updates one release PR from main's commit history. Use
Conventional Commit subjects (`fix:`, `feat:`, `feat!:` and `BREAKING CHANGE:`)
so its changelog and version proposals are useful. Review the resulting APIs and
changelog before merging. Only merging a branch named `release-plz-*` into main
is eligible to publish, verified through the associated-PR API and merge SHA.

crates.io Trusted Publishing refuses OIDC tokens to `workflow_run` runs, so when
the verified commit is a merged release PR, the `dispatch-publish` job dispatches
`release.yml` itself (`workflow_dispatch` on `main`) with that SHA and CI run ID.
The dispatched run verifies both inputs again with main's own verifier script
(never the input commit's scripts) before its publish job can start. The Trusted
Publisher configuration therefore stays `release.yml` + environment `crates-io`.
If a dispatch is lost, an owner can start it manually with the same two inputs
from the Actions tab; the same verification applies.

After environment approval, the publishing job checks the exact commit's CI again,
checks locked dependency ages and all eight runtime dependency graphs, runs
`cargo semver-checks --all-features` against registry baselines, and performs a
workspace `cargo publish --dry-run` covering every publishable crate. All of this
finishes **before** requesting a crates.io token. Failures stop the release.

The publisher uses Cargo's native multi-package publication instead of
`release-plz release`: release-plz's per-package upload sequence cannot pre-stage
all unpublished sibling versions for the all-crates dry run. Cargo stages those
versions together, publishes in dependency order and retains the seven-day
third-party soak. Release-plz still owns PRs, versioning and changelogs.

After publication, the script verifies the registry archive SHA-256 and
`.cargo_vcs_info.json` source commit, then creates each crate's tag and GitHub
Release with that version's changelog entry. A retry can recover an upload from
**the same commit** whose tag was not yet created. A different already-published
source commit is rejected. Tags and crate archives are immutable release records.

Main CI and release concurrency use `queue: max`, with no in-progress cancellation.
GitHub currently caps each pending queue at 100; beyond that GitHub cancels new
entries. Monitor that limit and rerun affected main SHAs. This platform limit
cannot be removed by workflow YAML.

## Versions and Perry's seven-day soak

Stay on `0.x` while Perry is the first consumer. Breaking public API changes bump
the **minor** version (`0.1.x` → `0.2.0`); compatible fixes/additions bump the
patch. Review semver-checks and generated version bumps; never bypass a failing
check simply to ship. Perry pins exact versions, for example `turnloop = "=0.1.0"`.

Perry's standing policy follows the perex 0.1.4 precedent recorded in DESIGN.md
§13. Prefer waiting seven days. For an immediate approved turnloop bump, make a
one-command exception for that version, verify the downloaded archive and record
provenance. In Perry's checkout, first edit the desired exact version in its
manifest, then (Bash; set the three release values explicitly):

```bash
crate=turnloop
version=0.1.0
source_commit=FULL_40_CHARACTER_RELEASE_COMMIT
CARGO_RESOLVER_INCOMPATIBLE_PUBLISH_AGE=allow \
  cargo +nightly-2026-08-20 update -p "$crate" --precise "$version"
git diff -- Cargo.toml Cargo.lock
python3 /path/to/turnloop/scripts/ci/verify-crate.py \
  --package "$crate" --version "$version" --source-commit "$source_commit" \
  --lockfile Cargo.lock
```

The exception lives only in that command's environment; never edit either
repository's soak configuration or set it in CI. Review the complete lockfile
diff: unrelated dependencies must not acquire younger versions. Repeat with an
explicit package/version for new sibling libraries when necessary. The verifier
checks crates.io, the downloaded `.crate`, Cargo.lock's registry checksum, and
the archive's source commit. Record its JSON fields (version, publish timestamp,
source commit, SHA-256) in Perry's bump commit message. The values above are
examples, not a claim that turnloop 0.1.0 has been published.

## Failed releases, rollback and yanking

There is no overwrite or deletion of an uploaded crate version. If a batch stops,
inspect crates.io and GitHub before rerunning the failed publishing job at the
**same SHA**. If a just-published sibling is rejected by the seven-day resolver
soak during a partial-batch retry, wait for its eligibility; never disable the
soak to recover a release. It rechecks gates and recovers same-commit uploads. If all required
versions are already tagged, the operation has no further uploads.

For a defective release, pin Perry back to its previous exact versions and
restore its reviewed lockfile, then ship a forward fix with a new patch/minor
version. Yanking discourages new resolution; existing lockfiles may still use
the yanked version. An owner may use a temporary, narrowly scoped token locally:

```bash
cargo +nightly-2026-08-20 yank --vers VERSION CRATE
# Only if the yank itself was mistaken:
cargo +nightly-2026-08-20 yank --undo --vers VERSION CRATE
```

Revoke that token immediately afterwards. Explain the affected versions and fix
in their GitHub Release notes. Do not move existing release tags or delete the
history. The automation never yanks by itself.

References: [Cargo workspace publishing](https://doc.rust-lang.org/cargo/commands/cargo-publish.html),
[crates.io Trusted Publishing](https://doc.rust-lang.org/cargo/reference/registry-authentication.html#trusted-publishing),
[OIDC action](https://github.com/rust-lang/crates-io-auth-action),
[release-plz](https://release-plz.dev/docs/usage/release-pr),
[GitHub concurrency](https://docs.github.com/en/actions/how-tos/write-workflows/choose-when-workflows-run/control-workflow-concurrency).
