# Changelog

Notable changes to Record Store. The format follows
[Keep a Changelog](https://keepachangelog.com/en/1.1.0/), and Record Store uses
[semantic versioning](https://semver.org/spec/v2.0.0.html).

The section for a released version is what the GitHub Release for that version
publishes, so keep it factual and written for the people upgrading.

## [Unreleased]

### Changed

- redb moves from 2.6.3 to 4.x, which reads only file format v3. **Upgrade to
  0.1.3 and start it once before upgrading to this release**: 0.1.3 converts
  each database to v3, and the conversion is unavailable here because redb 4
  removed `Database::upgrade` along with the ability to read v2. A database that
  is still v2 is refused at startup with a message naming 0.1.3 rather than a
  file format number.
- **This release cannot be rolled back to 0.1.2 or earlier.** Those releases
  cannot read the v3 file that 0.1.3 produced. 0.1.3 itself remains a safe
  rollback target.

- Every redb database is migrated from file format v2 to v3 when it is opened.
  redb 3.0 dropped the ability to read v2, and every release up to 0.1.2 wrote
  it, so a later redb 4 upgrade would otherwise meet a file it cannot open. The
  migration runs once per database on first start, is a no-op afterwards, and
  the redb version shipped here still reads a migrated file — this release
  remains one you can go back to. **Upgrade to this release before any release
  that carries redb 4.**

## [0.1.2] - 2026-09-16

A patch release. The S3 layer accepts the AWS SDK for Java v2's defaults, and a
rustls advisory is closed. No configuration or data changes are required.

### Added

- AWS SDK for Java v2 compatibility tests (Java 21, SDK 2.54.12), run by
  `tests/compatibility/run.sh` and in CI alongside the boto3, JavaScript, and Go
  suites, and a Java client setup page. Java was the only major SDK without
  coverage, and the only one whose defaults the suite could not otherwise exercise.

- Fuzz targets for the parsers that run before a request is authenticated, in the
  new `fuzz/` workspace: the S3 XML request bodies, the `Authorization` header, the
  presigned-URL query, the `Range` header, the ListObjectsV2 query, and bucket-name
  and object-key validation. Each target asserts an invariant rather than only the
  absence of a panic. CI builds and briefly runs all of them.

### Changed

- `tests/rust-audit.sh` now runs `cargo audit --deny warnings` with no exceptions.
  The RUSTSEC-2026-0235 exception is gone, and so is the finding it covered:
  `rust_decimal` 1.43.0 dropped the optional `rkyv` 0.7 backend that had put the
  crate in `Cargo.lock`, and `chacha20` moved off a yanked 0.10.1. `--deny warnings` makes a yanked crate a
  failure rather than a note.
- The documentation toolchain is pinned by hash. `requirements-docs.txt` now records
  an exact version and every artifact SHA-256 for each package, direct and
  transitive, and is installed with `pip install --require-hashes`.

### Fixed

- A PUT authenticated with a SigV4 `Authorization` header was refused when it
  carried `x-amz-content-sha256: UNSIGNED-PAYLOAD`, which is a valid SigV4 value
  and the AWS SDK for Java v2 default. Such requests are now accepted; the
  signature, credentials, and any supplied checksum are still verified, and only
  the body is left uncovered by the signature.
- Unsupported AWS streaming payloads — `aws-chunked` framing and trailing
  checksums — returned a generic `400 InvalidRequest`. They now return
  `501 NotImplemented`, as the documentation promises for unsupported operations,
  with a message naming the encoding and the setting to change. A malformed
  `x-amz-content-sha256` likewise names the header it rejected instead of
  reporting `Invalid Request`.

- A `Range` header naming a last byte past the end of the object returned a range
  reaching past EOF from `parse_range`. Responses were unaffected — the range was
  truncated again before the body or the `Content-Range` header were built — but the
  value was valid only because of that later call. It is now clamped where it is
  parsed, as the `bytes=-N` suffix form already was. Found by the `s3_range_header`
  fuzz target.

### Security

- `rustls` moves from 0.23.43 to 0.23.45, closing RUSTSEC-2026-0285: releases
  before 0.23.45 accept TLS 1.3 handshake messages across encryption level
  boundaries. It reaches Record Store transitively through `reqwest`, so only the
  lockfiles changed.

### Documentation

- Added `SECURITY.md`: which versions receive security fixes, how to report a
  vulnerability privately through GitHub private vulnerability reporting, what a
  report should contain, and what is in and out of scope. The security and
  contributing pages now link to it instead of naming an unspecified contact.

## [0.1.1] - 2026-08-29

First release published as container images. Everything before this was built
from a repository checkout.

### Added

- Object sharing: share links, capability tokens, and unlock tickets, in the new
  `record-store-sharing` crate, with a share viewer and embed links in the console.
- Safe inline object preview for images, text, PDFs, and media, in the management
  API and the console.
- Per-bucket CORS configuration across the domain model and the S3 protocol layer.
- A documentation site built with MkDocs Material, covering getting started,
  concepts, guides, SDKs, administration, deployment, cluster operation, security,
  operations, reference, and troubleshooting, published to GitHub Pages.
- Console screens for metrics, durability, rebalance, service account detail, and
  bucket lifecycle rules; a command palette with entity commands and keyboard
  navigation; audit filtering by source IP and request ID; and a collapsible sidebar.
- A Compose file for Coolify deployments at `deploy/docker/docker-compose.yaml`.
- Container images published to the GitHub Container Registry for `linux/amd64`
  and `linux/arm64`, with SPDX SBOMs per image and architecture, and SHA-256
  checksums covering every release asset. Images are published unsigned; see
  [Verifying a Release](https://openelementslabs.github.io/record-store/deployment/verifying-releases/).

### Changed

- Renamed the product from OES to Record Store throughout: crate and binary names,
  the `RECORD_STORE_` environment variable prefix, Protobuf packages under
  `proto/record-store/`, Dockerfiles, Compose files, the example configuration file
  (now `record-store.example.toml`), documentation, and the compatibility tests.
  Deployments carrying the old environment variable prefix must be updated.
- Reworked object storage onto a streaming local filesystem backend.
- Rebuilt the console's visual language on design tokens, with accessibility and
  focus-visible improvements throughout, and a redesigned login page.
- The console now labels the deployment mode and checks cluster capability before
  offering cluster-only views.

### Fixed

- Cluster membership no longer fails outright when quorum is momentarily
  unavailable; the membership barrier waits instead.
- The console tolerates a browser that refuses `localStorage` access rather than
  failing to render the theme toggle.

### Documentation

- README documents AWS response checksum validation and path-style addressing.
- Added installation, container image, release verification, and maintainer
  release documentation for the published images.

## [0.1.0] - 2026-08-22

First tagged release, distributed as source.

[unreleased]: https://github.com/OpenElementsLabs/record-store/compare/v0.1.2...HEAD
[0.1.2]: https://github.com/OpenElementsLabs/record-store/compare/v0.1.1...v0.1.2
[0.1.1]: https://github.com/OpenElementsLabs/record-store/compare/v0.1.0...v0.1.1
[0.1.0]: https://github.com/OpenElementsLabs/record-store/releases/tag/v0.1.0
