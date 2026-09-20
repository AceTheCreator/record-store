# Contributing to Record Store

Record Store stores other people's records. That shapes how changes get made here
more than any style rule: a bug in this repository is a bug in whatever a deployment
holds, and a feature that overstates what it guarantees is worse than no feature.

Contributions are welcome — issues, documentation, tests, and code. This page is what
you need to make one land.

**Found a security problem? Do not open an issue.** See [SECURITY.md](SECURITY.md).

## Before you start

For anything beyond a small fix, **open an issue first**. A short description of the
problem you hit is enough. It costs you ten minutes and can save you a weekend spent
on an approach that was never going to be merged.

Some things are settled and a pull request will not change them:

- **No clustering, replication, or erasure coding.** Single-node is the supported
  shape. The `record-store-erasure` crate exists and is wired into nothing.
- **No new external services.** No database server, broker, or coordination service
  alongside the process. Durable state is `redb` under the existing data directory.
- **Nothing is silently accepted.** An unsupported operation, header, or parameter
  returns S3 XML `NotImplemented` or the correct S3 error code. Ignoring a header a
  client sent is a bug, not a simplification.

## Getting set up

Rust 1.97.1 is selected by `rust-toolchain.toml`. A system `protoc` is not required.
Node 24 is needed only for the console.

```bash
git clone https://github.com/OpenElementsLabs/record-store
cd record-store
cargo test --workspace --all-features
```

See [Development Setup](docs/contributing/development-setup.md) for running a server
locally, and [Repository Structure](docs/contributing/repository-structure.md) for
what lives where.

## What must pass

Every one of these runs in CI, and a pull request that fails any of them will not be
merged:

```bash
cargo fmt --all --check
cargo clippy --workspace --all-targets --all-features -- -D warnings
cargo test --workspace --all-features --locked
bash tests/rust-audit.sh          # cargo audit --deny warnings, no exceptions
```

If your change touches documentation, also:

```bash
pip install --require-hashes -r requirements-docs.txt
mkdocs build --strict
```

If it touches the S3 surface, run the real-client suite. It starts a server and
drives it with boto3, the AWS SDK for JavaScript v3, the AWS SDK for Go, and the AWS
SDK for Java v2:

```bash
bash tests/compatibility/run.sh
```

### About the dependency audit

`tests/rust-audit.sh` runs `cargo audit --deny warnings` with **no exceptions**, and a
yanked crate fails it the same as an advisory. If your change needs a dependency that
trips it, that is a design conversation, not a flag to add — say so in the issue and
we will work out the alternative. There usually is one.

## How changes are expected to look

### Tests are the deliverable, not the evidence

A test here is expected to say *why it exists*, not just what it does. Look at the
existing ones: most carry a docstring explaining the failure they prevent. A test
named `test_delete` that asserts a status code tells a future maintainer nothing
about whether it is safe to change the behaviour.

Two habits that matter more than coverage numbers:

- **Write the test so it fails against the bug.** If you are fixing something,
  confirm the test fails before your fix and passes after. A regression test that
  passes either way is decoration.
- **Do not paste a failing assertion's actual output back in as the expectation.**
  If a pinned value is wrong, work out independently what it should be. Copying the
  output makes the test agree with the code by construction, which is exactly what it
  was supposed to check.

### Comments explain the decision, not the syntax

The code says what it does. A comment is for the thing the next person would
otherwise undo: why a check lives in the transaction rather than the handler, why an
odd node is promoted instead of duplicated, why a value is refused rather than
clamped. If a comment restates the line below it, delete it.

### Claims must be true

This project's documentation is deliberately careful about what it does and does not
guarantee. Object Lock is enforced by Record Store, not by the filesystem. A proof
bundle signed with a key derived from the deployment's own master key is not evidence
against that deployment's operator. Where a limit exists, the docs say so plainly.

If your change adds a guarantee, say precisely what it covers **and what it does
not**. If you find a claim in the documentation that is stronger than the code, that
is a bug worth reporting on its own.

## Commits and pull requests

- Work on a branch; `main` is protected.
- Write commit messages in the imperative mood, with a body explaining *why* when the
  reason is not obvious from the diff. `git log` here is a reasonable model.
- Keep a pull request to one concern. Two unrelated fixes are two pull requests.
- Update `CHANGELOG.md` under `## [Unreleased]`, following
  [Keep a Changelog](https://keepachangelog.com/en/1.1.0/). Write the entry for the
  person upgrading, not for the person who wrote the patch.
- Update the documentation in the same pull request as the behaviour. Documentation
  that lands a release later is documentation nobody trusts.

The pull request template asks what the change does, what it deliberately does not
do, and which limits a user needs to know. Those are not box-ticking: they are the
three things a reviewer cannot recover from the diff.

## Review

A maintainer will review. Expect questions about edge cases, error paths, and what
happens on crash or restart — this is a storage system, and "it works" is a weaker
claim here than elsewhere. Disagreement is fine; say why.

## Licence

Record Store is Apache-2.0. By contributing you agree your contribution is licensed
under it. There is no separate CLA.

## Conduct

By taking part you agree to the [Code of Conduct](CODE_OF_CONDUCT.md).
