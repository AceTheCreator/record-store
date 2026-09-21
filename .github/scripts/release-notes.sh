#!/usr/bin/env bash
# Composes the GitHub Release body for a Record Store version.
#
# The human part comes from CHANGELOG.md so the release notes and the changelog
# cannot drift apart, and an empty section fails the release rather than
# publishing a version nobody described. Everything below it is generated from
# what the workflow actually pushed, including the digests, so the notes cannot
# name an artifact that does not exist.
#
# Required environment:
#   VERSION          release version, without a leading v
#   REPOSITORY       owner/name, used in the verification commands
#   SERVER_IMAGE     server image, without a tag
#   SERVER_DIGEST    published server index digest
#   CONSOLE_IMAGE    console image, without a tag
#   CONSOLE_DIGEST   published console index digest
# Optional:
#   ASSET_DIRECTORY  directory of release assets; a checksums section is written
#                    only when it holds a SHA256SUMS file
set -euo pipefail

: "${VERSION:?}" "${REPOSITORY:?}"
: "${SERVER_IMAGE:?}" "${SERVER_DIGEST:?}" "${CONSOLE_IMAGE:?}" "${CONSOLE_DIGEST:?}"

root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"

changelog="$(
  awk -v version="$VERSION" '
    $0 ~ "^## \\[" version "\\]" { capture = 1; next }
    capture && /^## /            { exit }
    capture                      { print }
  ' "$root/CHANGELOG.md" | sed -e '/./,$!d' | sed -e :a -e '/^\n*$/{$d;N;ba' -e '}'
)"

if [[ -z "$changelog" ]]; then
  echo "CHANGELOG.md has no section for $VERSION" >&2
  exit 1
fi

cat <<NOTES
$changelog

## Containers

Published to the GitHub Container Registry for \`linux/amd64\` and \`linux/arm64\`.
One pull resolves the architecture it is run on.

\`\`\`bash
docker pull $SERVER_IMAGE:$VERSION
docker pull $CONSOLE_IMAGE:$VERSION
\`\`\`

A version tag selects a release; a digest selects an exact artifact. Pin the
digest in production:

\`\`\`text
$SERVER_IMAGE@$SERVER_DIGEST
$CONSOLE_IMAGE@$CONSOLE_DIGEST
\`\`\`

\`$VERSION\` is immutable. A fix is released as the next patch version, never as a
rebuild of this one.

## Verification

Every image and every binary archive carries signed build provenance, naming the
workflow, the repository and the commit that produced it. The release refuses to
publish if any of it is missing.

\`\`\`bash
gh attestation verify oci://$SERVER_IMAGE@$SERVER_DIGEST --repo $REPOSITORY
gh attestation verify record-store-$VERSION-linux-amd64.tar.gz --repo $REPOSITORY
\`\`\`

The same provenance is attached here as \`record-store-$VERSION-provenance.intoto.jsonl\`,
so it can be kept alongside the bytes and checked without asking GitHub about a
file GitHub is also hosting:

\`\`\`bash
gh attestation verify record-store-$VERSION-linux-amd64.tar.gz \\
  --bundle record-store-$VERSION-provenance.intoto.jsonl \\
  --repo $REPOSITORY
\`\`\`

Confirm the image reports the version it is tagged with:

\`\`\`bash
docker run --rm --entrypoint record-store $SERVER_IMAGE:$VERSION --version
\`\`\`

Inspect the published manifest, its digest, and its platforms:

\`\`\`bash
docker buildx imagetools inspect $SERVER_IMAGE:$VERSION
\`\`\`
NOTES

if [[ -n "${ASSET_DIRECTORY:-}" && -f "${ASSET_DIRECTORY}/SHA256SUMS" ]]; then
  cat <<'NOTES'

Verify the downloadable archives against the published checksums:

```bash
sha256sum -c SHA256SUMS      # macOS: shasum -a 256 -c SHA256SUMS
```
NOTES
fi

cat <<NOTES

## Assets

Linux binary archives contain \`record-store\` and \`record-store-server\`, taken from
the published images so the archive and the container hold the same build. An
SPDX SBOM is attached per image and per architecture, the SLSA provenance for the
archives is attached as \`.intoto.jsonl\`, and \`SHA256SUMS\` covers every asset here.

The release tag is signed too. See
[Verifying a Release](https://openelementslabs.github.io/record-store/deployment/verifying-releases/).

macOS builds are not published; build from source with \`cargo build --release\`.
NOTES
