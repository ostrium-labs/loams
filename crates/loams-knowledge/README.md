# loams-knowledge

The read-only project knowledge cache (MS1 Task 7; decision D1813; plan
`docs/plans/2026-10-25-ms1-marketplace-seed.md`). Agents on many machines
read the same public project context without cloning the repository, and
every byte they read is checked against one signed commit.

## How it works

- **Publishing** (`Publisher`). It reads a **public** repository's knowledge
  set at one commit and stores it on Walrus as one quilt, through
  `loams-walrus`. Each file is a patch, and its identifier is its path.
  - The manifest maps each path to its quilt patch id, SHA-256 and size.
  - The manifest is signed with Sigstore, keyless
    (`cosign sign-blob --bundle`), the way the marketplace index is signed.
  - The manifest and its bundle are written to the bucket, which stays the
    source of truth.
- **Reading** (`KnowledgeReader`). It reads the manifest and its bundle from
  the bucket and verifies the bundle against the repository's pinned signer
  before it parses anything. Then it reads entries from Walrus by patch id
  and checks each one's SHA-256 before using it.
  - `get` returns one verified file.
  - `search` scores path matches, plus matches in the text of the verified
    entries.
  - There is no write path.
- **Agents** use the reader through agentd's MCP shim, with two read-only
  tools: `knowledge_get` and `knowledge_search`.

## Public data only

Walrus blobs are public, and they can't be made private later. So
`Publisher::publish` needs two things:

- the caller's `Visibility::Public`, which is what the forge says;
- `public = true` in `.loams/knowledge.toml` at the commit being published.

If either is missing, the publish fails with `KnowledgeError::NotPublic`
before anything is sent anywhere.

## The knowledge set (Q1331)

A file is included if it meets all of these:

- **Where it is.** It's tracked and under `docs/`, `schemas/` or `proto/`,
  or it's any `*.proto` file, or it's one of the root files `README*`,
  `AGENTS.md`, `CLAUDE.md` and `.goosehints`.
- **Size.** It's at most 1 MiB.
- **Text.** It's valid UTF-8 and has no NUL byte.
- **Not vendored or generated.** It isn't under `vendor/`, `third-party/`,
  `third_party/`, `node_modules/` or `generated/`, and it has no
  `@generated` marker in its first 1 KiB.
- **A real file.** It isn't a symlink or a submodule.

`.loams/knowledge.toml` can narrow the set but never widen it:

```toml
public = true
include = ["docs/design/", "README.md"]   # optional: only these path prefixes
exclude = ["docs/plans/"]                  # optional
```

There's also one synthetic entry, `.loams/knowledge/files.txt`. It lists
every tracked path at the commit, one per line.

## Bucket layout

```text
<prefix>/<repo>/<commit>/manifest.json
<prefix>/<repo>/<commit>/manifest.json.sigstore.json
<prefix>/<repo>/latest              # the newest published commit
```

`prefix` defaults to `knowledge`. The `latest` file isn't signed. It can
only name a commit, and that commit's manifest must still verify.

## The reader's config

agentd's MCP shim loads this file from `LOAMS_AGENTD_KNOWLEDGE_CONFIG`:

```toml
[walrus]
enabled = true
network = "testnet"

[knowledge]
bucket = "s3://loams-bucket/ns/default"
prefix = "knowledge"

[[knowledge.signers]]
repo = "ostrium-labs/loams"
identity = "https://github.com/ostrium-labs/loams/.github/workflows/knowledge.yml@refs/heads/dev"
issuer = "https://token.actions.githubusercontent.com"
```
