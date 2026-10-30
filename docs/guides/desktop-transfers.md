# Transfers in Loams Desktop (preview)

Loams Desktop moves large files to and from a Loams deployment, and follows remote job runs such as CI, over HTTP/3 (QUIC). It falls back to HTTP/2 over TLS when UDP is blocked. A crash, a laptop going to sleep, or a lost network costs at most the chunks in flight: the next attempt resumes from the last committed chunk, and every file is checked end to end.

Design: [§85](../design/85-desktop-resumable-quic-transfer.md). Plan: [DQ1](../plans/2026-10-29-dq1-desktop-quic-transfer.md). Status: **preview**.

## How it works

- **The sidecar.** The desktop starts `loams-transfer agent`, a small Rust process, on first use. It speaks HTTP/3 and HTTP/2 to the server. Electron only shows its progress.
- **Chunks.** A file moves in fixed-size chunks: 16 MiB by default, up to 64 MiB for files over 256 GiB. Each chunk's key carries its index and SHA-256, so a resend is harmless.
- **The checkpoint.** A chunk counts once the server's metastore pointer for the transfer has moved past it. A resume asks the server what is committed and sends the rest.
- **One writer.** Each transfer has a lease that the desktop renews every 10 s. A second device gets "Another device holds this transfer" until the lease lapses, 30 s after its holder stopped.
- **Integrity.** Every chunk is checked on the server and again on download. The whole file's SHA-256 is checked before a download is renamed into place.
- **Memory.** Files stream from disk in 1 MiB blocks. A 10 GiB upload peaks at about 100 MiB of sidecar memory, whatever the file size.

## Using it

1. Open **Transfers** in the sidebar.
2. Pick the server and a namespace. The first time, paste an API key that has access to that namespace. The key goes to the system keychain. For a dev server with a self-signed certificate, also paste the certificate's SHA-256, which `loams-transfer serve` prints at start.
3. **Upload a file…** Each transfer shows bytes done, speed (over the last 5 s), time left, the transport (HTTP/3, or HTTP/2 when it fell back), chunks in flight and retries.
4. **Pause** keeps the checkpoint. **Resume**, or restarting the app, continues from the last committed chunk ("Resumed from 4.2 GB").
5. **On the server** lists the namespace's transfers. **Download…** saves one, through a `.loams-part` file beside the destination that resumes the same way.

## Remote runs (CI and other jobs)

A runner executes named jobs on a machine you control, and streams their logs and artifacts back:

```toml
# jobs.toml on the runner's host
[jobs.ci]
argv = ["bash", "-c", "./scripts/ci-local.sh"]
cwd = "/srv/loams"
env_allow = ["REF"]          # the only params a run may pass, as env vars
timeout_s = 3600
artifacts = ["target/reports/*.xml"]
```

```sh
loams-transfer runner --server https://lab.example.ts.net:7790 --ns default \
  --token-file ~/.config/loams/runner.key --state ~/.local/state/loams-runner --jobs jobs.toml
```

In **Runs**, start `ci` with `REF=main`, then **Follow log**. The log streams live, and a reconnect continues from the last byte received. The runner runs only the jobs in its own `jobs.toml`, and a request cannot add commands or arguments. Artifacts appear on the run when it ends, and download like any transfer.

## Servers

- **The engine** serves the transfer API on its main port when it is built with the `transfer` feature, which is on by default. With `LOAMS_TLS_CERT_FILE` and `LOAMS_TLS_KEY_FILE` set, it also serves HTTP/3 on UDP at the same port number. `LOAMS_TRANSFER_H3=off` turns HTTP/3 off. Open UDP on that port in the firewall to get QUIC. Otherwise clients use HTTP/2 over TLS on TCP.
- **A local engine** without TLS is reached over plain HTTP/2 on loopback.
- **`loams-transfer serve`** runs the API alone, for tests and benchmarks: a single-node metastore and a local-folder (or S3) bucket, with a self-signed certificate made on first start.

## Transport and fallback

The sidecar tries QUIC first and starts TCP+TLS after 250 ms. It remembers per server, for 15 minutes, that UDP failed, so the next connection goes straight to HTTP/2. In the key form, **Transport** can force `HTTP/3 only` or `HTTP/2 only`.

## Limits (preview)

- Files up to 1 TiB (16,384 chunks of at most 64 MiB).
- API keys over server-authenticated TLS. Device certificates (mTLS) are later work.
- There is no cleanup of abandoned or unfinished transfers yet. Their chunks stay in the bucket until a later task adds the 7-day cleanup (Q1513).
