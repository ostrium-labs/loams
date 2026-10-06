import type { IncomingMessage, ServerResponse } from "node:http";
import type { ConnectRouter } from "@connectrpc/connect";
import type { UniversalServerRequest, UniversalServerResponse } from "@connectrpc/connect/protocol";

/**
 * Bridge Node's `http` server to Connect-ES v2's transport-agnostic handlers.
 *
 * WHY THIS EXISTS
 * ---------------
 * Connect-ES v2 deliberately does not know about Node. Every RPC handler is
 * `(UniversalServerRequest) => Promise<UniversalServerResponse>`, where the
 * request is a plain object carrying a `Headers` and an async-iterable body, and
 * the response is `{ status, header, body }`. This server runs on Node's
 * `http.createServer`, so something has to translate. That lives here rather
 * than inlined in the request handler so the REST routes and this bridge stay
 * independently readable.
 */

function toHeaders(raw: IncomingMessage["headers"]): Headers {
  const headers = new Headers();
  for (const [key, value] of Object.entries(raw)) {
    if (value === undefined) continue;
    if (Array.isArray(value)) {
      for (const v of value) headers.append(key, v);
    } else {
      headers.set(key, value);
    }
  }
  return headers;
}

/** Stream the request body as an async iterable of bytes. */
async function* streamBody(req: IncomingMessage): AsyncGenerator<Uint8Array> {
  for await (const chunk of req) {
    yield typeof chunk === "string"
      ? new TextEncoder().encode(chunk)
      : new Uint8Array(chunk as Buffer);
  }
}

export interface NodeConnectBridge {
  /**
   * Try to handle a request with the Connect router.
   *
   * Returns `true` when the request was an RPC for a registered method (whether
   * or not the call itself succeeded), and `false` when it was not ours, so the
   * caller can fall through to its other routes. Returning `false` never writes
   * to the response.
   */
  (req: IncomingMessage, res: ServerResponse): Promise<boolean>;
}

/**
 * Wrap a Connect router as a Node request handler.
 *
 * Connect-ES v2 hands back one HTTP handler per RPC, each tagged with the request
 * path it serves and the methods it allows, with no aggregate handler to
 * delegate to. They are indexed by path and dispatched on that path.
 *
 * Dispatch keys on PATH ONLY, deliberately. A known RPC path reached with the
 * wrong method is still ours: the handler answers with Connect's own 405 and
 * proper protocol framing. Falling through on a method mismatch instead would
 * hand the caller a generic 404, which a client reads as "this RPC does not
 * exist" -- a materially wrong answer to "that method is not allowed here".
 */
export function createNodeConnectBridge(router: ConnectRouter): NodeConnectBridge {
  const byPath = new Map(router.handlers.map((handler) => [handler.requestPath, handler]));

  return async function handle(req, res) {
    const requestPath = new URL(req.url || "/", `http://${req.headers.host || "localhost"}`)
      .pathname;
    const handler = byPath.get(requestPath);
    if (!handler) {
      return false;
    }

    // Aborting the request must abort the RPC, so a client disconnect cancels
    // the work rather than leaving a handler running against a dead socket.
    const controller = new AbortController();
    req.once("aborted", () => controller.abort());

    const universalRequest: UniversalServerRequest = {
      httpVersion: req.httpVersion,
      // The full path INCLUDING the query string: Connect uses it for protocol
      // negotiation (`?encoding=`), so truncating to the pathname breaks it.
      url: req.url || "/",
      method: req.method || "GET",
      header: toHeaders(req.headers),
      body: streamBody(req),
      signal: controller.signal,
    };

    let response: UniversalServerResponse;
    try {
      response = await handler(universalRequest);
    } catch (err) {
      // The handler rejected outside its own protocol envelope; still answer with
      // a Connect-shaped error so the client parses it instead of seeing HTML.
      const message = err instanceof Error ? err.message : String(err);
      res.writeHead(500, { "Content-Type": "application/json" });
      res.end(JSON.stringify({ code: "internal", message }));
      return true;
    }

    response.header?.forEach((value: string, key: string) => res.setHeader(key, value));
    res.writeHead(response.status);
    if (response.body) {
      for await (const chunk of response.body) {
        res.write(Buffer.from(chunk));
      }
    }
    res.end();
    return true;
  };
}
