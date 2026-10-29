// Request-counting HTTPS proxy for LC0 Task 2 (spike only, never shipped).
//
// TiDB Cloud Filesystem has no per-file-system request counter in its API
// or CLI, so this proxy counts the requests ti-drive9 makes. Point a process
// at it with HTTPS_PROXY=http://127.0.0.1:<port> and SSL_CERT_FILE=<bundle
// with the spike CA>. Connections to *.drive9.ai are terminated with a leaf
// certificate signed by the throwaway spike CA and forwarded upstream; every
// other host (e.g. presigned object-store URLs) is tunnelled blind and
// counted as one CONNECT.
//
// It logs, one JSON line per request: time, method, host, path (query
// string dropped: it can carry signatures), status, request and response
// byte counts. It never logs headers or bodies, so tokens never reach the
// log.
//
// Usage: node countproxy.ts <port> <leaf-cert.pem> <leaf-key.pem> <log.jsonl>

import { appendFileSync, readFileSync } from "node:fs";
import http from "node:http";
import https from "node:https";
import net from "node:net";
import tls from "node:tls";

const [portArg, certPath, keyPath, logPath] = process.argv.slice(2);
if (!portArg || !certPath || !keyPath || !logPath) {
  console.error("usage: countproxy.ts <port> <cert> <key> <log>");
  process.exit(2);
}
const cert = readFileSync(certPath);
const key = readFileSync(keyPath);
const MITM = /(^|\.)drive9\.ai$/;

function log(rec: Record<string, unknown>): void {
  appendFileSync(logPath, JSON.stringify({ t: Date.now() / 1000, ...rec }) + "\n");
}

function forward(host: string, req: http.IncomingMessage, res: http.ServerResponse): void {
  const path = (req.url ?? "/").split("?")[0];
  let reqBytes = 0;
  let resBytes = 0;
  const headers = { ...req.headers, host };
  const up = https.request(
    { host, port: 443, method: req.method, path: req.url, headers, servername: host },
    (upRes) => {
      res.writeHead(upRes.statusCode ?? 502, upRes.headers);
      upRes.on("data", (c: Buffer) => { resBytes += c.length; });
      upRes.pipe(res);
      upRes.on("end", () => log({ method: req.method, host, path, status: upRes.statusCode, reqBytes, resBytes }));
      upRes.on("error", () => log({ method: req.method, host, path, status: upRes.statusCode, reqBytes, resBytes, aborted: true }));
    },
  );
  up.on("error", (e: Error) => {
    log({ method: req.method, host, path, status: 0, error: e.message });
    if (!res.headersSent) res.writeHead(502);
    res.end();
  });
  req.on("data", (c: Buffer) => { reqBytes += c.length; });
  req.pipe(up);
  res.on("close", () => up.destroy());
}

const server = http.createServer((_req, res) => { res.writeHead(400); res.end("CONNECT only\n"); });

server.on("connect", (req: http.IncomingMessage, client: net.Socket, head: Buffer) => {
  const [host, portStr] = (req.url ?? "").split(":");
  const port = Number(portStr || 443);
  if (MITM.test(host) && port === 443) {
    client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    if (head.length) client.unshift(head);
    const tsock = new tls.TLSSocket(client, { isServer: true, cert, key, ALPNProtocols: ["http/1.1"] });
    const inner = http.createServer((r, s) => forward(host, r, s));
    inner.emit("connection", tsock);
    tsock.on("error", () => client.destroy());
    return;
  }
  let up = 0;
  let down = 0;
  const upstream = net.connect(port, host, () => {
    client.write("HTTP/1.1 200 Connection Established\r\n\r\n");
    if (head.length) upstream.write(head);
    client.pipe(upstream);
    upstream.pipe(client);
  });
  client.on("data", (c: Buffer) => { up += c.length; });
  upstream.on("data", (c: Buffer) => { down += c.length; });
  const done = () => { log({ method: "CONNECT", host, port, upBytes: up, downBytes: down }); };
  upstream.on("close", done);
  upstream.on("error", () => client.destroy());
  client.on("error", () => upstream.destroy());
});

server.listen(Number(portArg), "127.0.0.1", () => console.error(`countproxy on 127.0.0.1:${portArg}`));
