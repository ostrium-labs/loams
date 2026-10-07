/**
 * A minimal Connect streaming client for the Loams app protos.
 *
 * Deliberately not `@connectrpc/connect`. That library needs generated
 * descriptors for every message, and the Loams app protos are not generated
 * into this workspace -- the three services this needs are the whole reason
 * this file exists, and hand-decoding `application/connect+json` is a few
 * dozen lines against a documented, stable envelope. If the Loams protos are
 * ever generated here, this file is what gets deleted.
 *
 * The envelope (connect protocol spec, "Envelope Frame"):
 *
 *   byte  0     flags
 *   bytes 1-4   payload length, big-endian uint32
 *   bytes 5..   payload
 *
 * `flags & 0x02` marks the end-of-stream frame, whose payload is `{}` on
 * success or an `Error` shape on failure.
 */

const HEADER = 5;
const FLAG_END_STREAM = 0x02;

export interface WatchOptions {
  /** Resume point from a previous stream; omit for a fresh snapshot. */
  resumeCursor?: string;
  /** Narrows the stream, e.g. approval states or an owning user. */
  request?: Record<string, unknown>;
  signal?: AbortSignal;
}

export class LoamsStreamError extends Error {
  readonly code: string;
  constructor(code: string, message: string) {
    super(message);
    this.name = "LoamsStreamError";
    this.code = code;
  }
}

/** Frames a request message the way a Connect streaming request body must be. */
function frameRequest(message: Record<string, unknown>): Uint8Array {
  const payload = new TextEncoder().encode(JSON.stringify(message));
  const out = new Uint8Array(HEADER + payload.length);
  new DataView(out.buffer).setUint32(1, payload.length, false);
  out.set(payload, HEADER);
  return out;
}

export interface Frame {
  end: boolean;
  payload: Record<string, unknown>;
}

/**
 * Pulls frames out of a byte stream.
 *
 * Kept separate from the fetch so it can be tested against recorded bytes: the
 * failure this guards against is a frame arriving split across two TCP reads,
 * which silently truncates a snapshot if the length header is read before the
 * whole frame has arrived.
 */
export class FrameReader {
  private _buffer: Uint8Array = new Uint8Array(0);

  push(chunk: Uint8Array): Frame[] {
    const merged = new Uint8Array(this._buffer.length + chunk.length);
    merged.set(this._buffer, 0);
    merged.set(chunk, this._buffer.length);
    this._buffer = merged;

    const frames: Frame[] = [];
    let offset = 0;
    for (;;) {
      if (merged.length - offset < HEADER) break;
      const flags = merged[offset];
      const length = new DataView(merged.buffer, merged.byteOffset).getUint32(offset + 1, false);
      if (merged.length - offset - HEADER < length) break;
      const body = merged.subarray(offset + HEADER, offset + HEADER + length);
      const end = (flags & FLAG_END_STREAM) !== 0;
      const text = new TextDecoder().decode(body);
      frames.push({ end, payload: text ? JSON.parse(text) : {} });
      offset += HEADER + length;
    }
    this._buffer = merged.slice(offset);
    return frames;
  }
}

export interface WatchHandle {
  /** Resolves when the stream ends, normally or by error. */
  done: Promise<void>;
  stop(): void;
}

/**
 * Opens a server-streaming RPC and calls `onMessage` for each message.
 *
 * `servicePath` is the fully-qualified service, package segment lowercased and
 * service segment capitalised -- `loams.operations.v1.OperationsService`. Both
 * halves matter: capitalising the package segment produces a 501 "method not
 * found" from a server that has the method.
 *
 * Reconnection and cursor resume are the caller's business: this returns the
 * stream and lets go, because the cursor to resume from is a property of the
 * store built on top, not of the transport.
 */
export async function watch(
  baseUrl: string,
  servicePath: string,
  method: string,
  token: string,
  onMessage: (message: Record<string, unknown>) => void,
  options: WatchOptions = {},
): Promise<WatchHandle> {
  const controller = new AbortController();
  if (options.signal) {
    options.signal.addEventListener("abort", () => controller.abort(), { once: true });
  }

  const response = await fetch(`${baseUrl}/${servicePath}/${method}`, {
    method: "POST",
    headers: {
      "Content-Type": "application/connect+json",
      "Connect-Protocol-Version": "1",
      Authorization: `Bearer ${token}`,
    },
    body: frameRequest({
      ...options.request,
      ...(options.resumeCursor ? { resumeCursor: options.resumeCursor } : {}),
    }),
    signal: controller.signal,
    // Node's fetch requires this for a streaming request body; harmless
    // elsewhere.
    duplex: "half",
  } as RequestInit & { duplex: "half" });

  if (!response.ok || !response.body) {
    const detail = await response.text().catch(() => "");
    throw new LoamsStreamError(
      `http_${response.status}`,
      `${servicePath}/${method} failed: ${response.status}${detail ? ` ${detail}` : ""}`,
    );
  }

  const reader = response.body.getReader();
  const frames = new FrameReader();

  const done = (async () => {
    try {
      for (;;) {
        const { done: finished, value } = await reader.read();
        if (finished) return;
        for (const frame of frames.push(value)) {
          if (frame.end) {
            const error = frame.payload.error as { code?: string; message?: string } | undefined;
            if (error) {
              throw new LoamsStreamError(error.code ?? "stream_error", error.message ?? "stream failed");
            }
            return;
          }
          onMessage(frame.payload);
        }
      }
    } finally {
      controller.abort();
    }
  })();

  return { done, stop: () => controller.abort() };
}