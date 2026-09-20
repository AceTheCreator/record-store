# Proof Bundle Format

A proof bundle is a signed JSON document describing one immutable object
version. Somebody holding the object file and the bundle — and nothing else, no
server, no network, no credential — can check that the file is the object the
bundle describes.

This page is the format's contract. It is written so that an independent
implementation can produce and check bundles without reading Record Store's
source.

Related: [Object Lock and Trust](../security/object-lock.md) for the wider
question of what this deployment can and cannot prove.

## What a bundle proves

| | Established by a bundle |
| --- | --- |
| The file matches the digest recorded when the object was written | **Yes** |
| The bundle has not been altered since it was signed | **Yes** |
| Which deployment signed it | **Only if you already hold that deployment's public key** |
| What happened to the object, and when | **Only if the bundle carries audit history** |
| That this state existed before now | **Only if an anchor receipt is present and you validate it** |

A bundle is not a certificate of authenticity. The signing key derives from the
deployment's own master key, so anyone holding that key can sign any bundle.
The signature is evidence against alteration in transit and against a bundle
fabricated by a third party; it is not evidence against the deployment's own
operator.

The verifier prints every check it performed **and every one it could not**, so
a passing result never implies more than it established.

## Producing and checking

```bash
# On the deployment: emit a bundle for an object version.
record-store verify object records statement.pdf \
  --proof ./statement.proof.json \
  --endpoint https://management.example.com

# Anywhere else, offline: check a file against it.
record-store verify proof ./statement.proof.json --object ./statement.pdf

# With the deployment's published key, which also establishes who signed it.
record-store verify proof ./statement.proof.json \
  --object ./statement.pdf \
  --public-key 2f05a1711823123bc4a23702f47bf17b6e84a863fae5ef05dafd9a7fd01a1cc1
```

`verify proof` exits non-zero when any check fails, so a script cannot mistake
failure for success.

## Document structure

```json
{
  "format": "record-store.proof-bundle",
  "format_version": 1,
  "object": { "bucket": "…", "key": "…", "version_id": "…", "size": 0,
              "content_type": "…", "created_at": "RFC 3339" },
  "payload": { "sha256": "64 hex characters" },
  "history": { "status": "unavailable" | "present", "…": "…" },
  "deployment": { "algorithm": "ed25519", "public_key": "hex", "key_id": "hex" },
  "signature": { "algorithm": "ed25519", "canonical_version": 1, "value": "hex" }
}
```

A bundle contains **no** capability tokens, **no** credentials, and **not the
object payload**. It is safe to send to whoever needs to check the file.

### `object`

| Field | Type | Notes |
| --- | --- | --- |
| `bucket` | string | Bucket name |
| `key` | string | Object key |
| `version_id` | string | The immutable version this bundle describes |
| `size` | integer | Payload length in bytes |
| `content_type` | string, optional | Omitted when the object has none |
| `created_at` | RFC 3339 | When the version was committed |

### `payload`

`sha256` is the lowercase hex SHA-256 of the object's bytes, as recorded at
write time. It is the bare digest rather than a `sha256:…` prefixed form, so it
can be compared directly against `sha256sum` output.

### `history`

A tagged union, never an omitted field. A missing section reads as "nothing
happened"; an explicit `unavailable` reads as "this was not checked", and those
are different claims.

**`status: "unavailable"`** carries a machine-readable `reason` and a
human-readable `detail`. Reasons:

| `reason` | Meaning |
| --- | --- |
| `chain_not_enabled` | The deployment does not maintain a tamper-evident audit chain |
| `version_predates_chain` | The chain exists, but this version was written before it |
| `not_yet_checkpointed` | Records exist but are not yet covered by a checkpoint |

**`status: "present"`** carries `records`, a `checkpoint`, and an optional
`anchor`:

```json
{
  "status": "present",
  "records": [
    { "sequence": 100, "timestamp": "…", "operation": "s3:PUT",
      "principal": "service_account:…", "result": "success",
      "previous_hash": "hex", "record_hash": "hex",
      "inclusion_path": { "index": 0, "steps": [ { "side": "right", "hash": "hex" } ] } }
  ],
  "checkpoint": { "sequence": 7, "from_sequence": 100, "to_sequence": 140,
                  "root": "hex", "previous_checkpoint_hash": "hex" },
  "anchor": { "kind": "rfc3161", "receipt": "base64", "asserted_at": "RFC 3339" }
}
```

A bundle carries only the records touching one object version, so it holds a
subset of the log. Consecutive entries are expected to link only when their
sequence numbers are adjacent; a verifier must not claim to have checked the
chain across a gap, because the records in between are not present.

### `deployment` and `signature`

`algorithm` is `ed25519` for format version 1. `public_key` is the 32-byte key,
hex. `key_id` is the first 8 bytes of `SHA-256(public_key)`, hex — a convenience
for telling bundles apart at a glance, never a substitute for the key.

`canonical_version` names the encoding the signature covers, so a future
encoding can be introduced without invalidating existing bundles.

## Canonical encoding

The signature is computed over an explicit byte encoding, **not** over the
serialized JSON. Two JSON writers will not agree on key order, whitespace, or
string escaping, and a signature that only verifies under the writer that
produced it is no use to a third party.

Encoding version 1, in order. All integers are **big-endian**. `bytes(v)` is a
4-byte length followed by the raw bytes. `str(v)` is `bytes(utf8(v))`.
`opt_str(v)` is `0x00` when absent, or `0x01` followed by `str(v)`. `time(v)` is
the 8-byte signed microseconds since the Unix epoch.

```text
"record-store/proof-bundle/v1"     literal, 28 bytes, no length prefix
u16   canonical version            0x0001
str   format
u16   format_version
str   object.bucket
str   object.key
str   object.version_id
u64   object.size
opt   object.content_type
time  object.created_at
str   payload.sha256
u8    history tag                  0x00 unavailable, 0x01 present
      ── when 0x00 ──
str     reason label               see the table below
str     detail
      ── when 0x01 ──
u32     record count
        per record:
u64       sequence
time      timestamp
str       operation
str       principal
str       result
str       previous_hash
str       record_hash
u64       inclusion_path.index
u32       step count
          per step:
u8          side                   0x00 left, 0x01 right
bytes       hash                   32 bytes, length-prefixed
u64     checkpoint.sequence
u64     checkpoint.from_sequence
u64     checkpoint.to_sequence
str     checkpoint.root
str     checkpoint.previous_checkpoint_hash
u8      anchor present             0x00 absent, 0x01 present
str       anchor.kind
str       anchor.receipt
u8        asserted_at present      0x00 absent, 0x01 present
time      asserted_at
str   deployment.algorithm
str   deployment.public_key
str   deployment.key_id
```

The `signature` object is **not** part of the encoding. Everything else is,
including the public key — a key sitting outside the signed bytes could be
swapped for another one.

The `reason` labels used in the encoding are the human forms, not the JSON
enum values:

| JSON `reason` | Encoded label |
| --- | --- |
| `chain_not_enabled` | `chain not enabled` |
| `version_predates_chain` | `version predates the chain` |
| `not_yet_checkpointed` | `not yet covered by a checkpoint` |

The signature is Ed25519 over these bytes, as specified in RFC 8032.

## Merkle construction

Needed only to check a `present` history. All hashes are SHA-256 over the
domain string `record-store/audit-merkle/v1` followed by a prefix byte:

```text
leaf(record_hash) = SHA-256( "record-store/audit-merkle/v1" ‖ 0x00 ‖ record_hash )
node(left, right) = SHA-256( "record-store/audit-merkle/v1" ‖ 0x01 ‖ left ‖ right )
```

Leaves and interior nodes use different prefixes so an interior node can never
be presented as a record.

The tree is built from the leaves of every record the checkpoint covers, in
sequence order. At each level, nodes are paired left to right. **An odd node at
any level is promoted to the next level unchanged, not duplicated** — hashing it
with itself would make a three-record range produce the same root as a
four-record range whose last entry repeats.

To check an inclusion path, start from `leaf(record_hash)` and fold each step:

```text
side = left   →  current = node(step.hash, current)
side = right  →  current = node(current, step.hash)
```

The result must equal `checkpoint.root`.

## Verification procedure

1. `format` equals `record-store.proof-bundle`, else fail.
2. `format_version` is understood. A **newer** version is reported as not
   verifiable rather than accepted, since it may carry fields you cannot check.
3. Recompute the canonical bytes and verify the Ed25519 signature against
   `deployment.public_key`.
4. If the verifier was given an expected public key, compare it with
   `deployment.public_key`. If it was not, report that the deployment's identity
   is **not established** — do not report success for this check.
5. Stream the object file through SHA-256 and compare with `payload.sha256`.
   The payload must never be loaded into memory whole; a bundle has to work for
   objects larger than the machine checking them.
6. If `history.status` is `present`, check every record's inclusion path against
   `checkpoint.root`, and check that records with adjacent sequence numbers
   link (`later.previous_hash == earlier.record_hash`).
7. Report every check and its status. A check that could not be performed is
   reported as **not proved**, never omitted and never counted as a pass.

## Worked example

An object written to a deployment whose master key is
`worked-example-master-key-at-least-32-bytes`, with the contents
`quarterly statement, Q1 2026` (28 bytes, no trailing newline):

```json
{
  "format": "record-store.proof-bundle",
  "format_version": 1,
  "object": {
    "bucket": "records",
    "key": "statement.pdf",
    "version_id": "e9a1f157-e984-4b55-917c-1b792b8b928f",
    "size": 28,
    "content_type": "application/pdf",
    "created_at": "2026-09-20T06:29:25.843834Z"
  },
  "payload": {
    "sha256": "de911d19c9e2fcbb51e973e5b877af8b87483a80a17017037f86fc3e12229887"
  },
  "history": {
    "status": "unavailable",
    "reason": "chain_not_enabled",
    "detail": "this deployment does not yet maintain a tamper-evident audit chain, so no audit records, checkpoint root, or anchor receipt are included"
  },
  "deployment": {
    "algorithm": "ed25519",
    "public_key": "2f05a1711823123bc4a23702f47bf17b6e84a863fae5ef05dafd9a7fd01a1cc1",
    "key_id": "3eff56fb594e6328"
  },
  "signature": {
    "algorithm": "ed25519",
    "canonical_version": 1,
    "value": "02280349fe87660a897877ea098c6639437e42db3e07a6b782bcef35c7cb3eb86feb3899f169a4003ccf259dd86235db1cc44238d4c5a9ed186908b071f31509"
  }
}
```

The payload digest is reproducible independently:

```console
$ printf 'quarterly statement, Q1 2026' | sha256sum
de911d19c9e2fcbb51e973e5b877af8b87483a80a17017037f86fc3e12229887
```

Checking it prints the checks and, importantly, the gaps:

```console
$ record-store verify proof ./statement.proof.json --object ./statement.pdf
records/statement.pdf version e9a1f157-e984-4b55-917c-1b792b8b928f
  [ok] bundle format: record-store.proof-bundle
  [ok] format version: version 1
  [ok] bundle signature: the bundle has not been altered since it was signed
  [not proved] deployment identity: … No expected key was supplied, so this does not
      establish which deployment produced it.
  [ok] payload digest: the file matches the SHA-256 recorded at write time (de911d19…)
  [not proved] audit history: this bundle carries no audit history (chain not enabled) …
  [not proved] external anchor: without audit history there is no checkpoint to anchor …

VERIFIED: every check that could be performed passed.
This does NOT establish:
  - deployment identity
  - audit history
  - external anchor
```

## Key derivation

The signing key is derived, not stored, so a deployment restored from backup
keeps its identity:

```text
seed = HKDF-SHA256(
    salt = "record-store/proof-signing/v1",
    ikm  = RECORD_STORE_CREDENTIAL_MASTER_KEY,
    info = "proof-bundle-signing-key",
    len  = 32)
key  = Ed25519 key pair from that 32-byte seed (RFC 8032)
```

The salt separates this key from the credential, capability, webhook, and object
encryption keys derived from the same master key.

Without a master key configured, the deployment reports proof bundles as
unavailable rather than emitting an unsigned document — an unsigned bundle would
be mistaken for a signed one by anyone not reading closely.

Publish `deployment.public_key` somewhere your counterparties can reach
independently of the bundles themselves. Until they hold it out of band, the
`deployment identity` check cannot pass, and every bundle you send is
self-consistent but unattributed.

## Limits

- **A bundle describes one object version.** Overwriting a key produces a new
  version, which needs its own bundle.
- **A bundle is a snapshot.** It says what the deployment recorded at the moment
  it was produced. It does not update, and it does not notice a later deletion.
- **The digest is the one recorded at write time.** A bundle proves the file
  matches what Record Store recorded, not that what Record Store recorded was
  what the uploader intended.
- **History and anchoring are absent in this release.** The sections exist in
  the format and are reported as `unavailable`, so bundles issued now stay
  parseable by verifiers built later.
