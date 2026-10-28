#!/usr/bin/env python3
"""End to end: an agent's browser through agentd's MCP shim (D2115, D2117).

Starts `loams-agentd mcp` (the server the daemon injects into every agent run)
with LOAMS_BROWSER_PROVIDER set, serves two fixture pages on 127.0.0.1, and
drives the browser tools the way an agent does: browser_open, browser_navigate,
browser_snapshot, browser_screenshot, browser_extract, a refused navigation to
the cloud metadata endpoint, and browser_close.

The result is written as normalised JSON (uids, the fixture's port and the
screenshot's path taken out), so the `local` and `service` runs can be compared
byte for byte: the same tools and the same policy must give the same answers
whichever provider runs the engine.

    agentd-mcp-e2e.py --agentd target/debug/loams-agentd --provider local --out local.json
    agentd-mcp-e2e.py --compare local.json service.json

Provider settings come from the environment, as they would in a deployment:
LOAMS_OBSCURA_BIN for local; LOAMS_BROWSER_SERVICE_URL and
LOAMS_WEB_BRIDGE_SECRET_BROWSER_TOKEN for service. Stdlib only.
"""

import argparse
import http.server
import json
import os
import re
import subprocess
import sys
import tempfile
import threading

INDEX = b"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Loams fixture</title></head>
<body>
<h1>Browser parity fixture</h1>
<p>This page is served by the end-to-end test.</p>
<label for="q">Search</label> <input id="q" name="q" type="text">
<button type="button">Go</button>
<a href="/next.html">Next page</a>
</body></html>
"""

NEXT = b"""<!doctype html>
<html lang="en"><head><meta charset="utf-8"><title>Next page</title></head>
<body>
<h1>You arrived</h1>
<p>The second fixture page.</p>
</body></html>
"""

PAGES = {"/": INDEX, "/index.html": INDEX, "/next.html": NEXT}


class Fixture(http.server.BaseHTTPRequestHandler):
    def do_GET(self):  # noqa: N802 (the stdlib's name)
        body = PAGES.get(self.path.split("?")[0])
        if body is None:
            self.send_response(404)
            self.end_headers()
            return
        self.send_response(200)
        self.send_header("Content-Type", "text/html; charset=utf-8")
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def log_message(self, *_args):
        pass


class Shim:
    """A newline-delimited JSON-RPC client over the shim's stdio."""

    def __init__(self, argv, env):
        self.proc = subprocess.Popen(
            argv,
            stdin=subprocess.PIPE,
            stdout=subprocess.PIPE,
            stderr=sys.stderr,
            env=env,
            text=True,
            bufsize=1,
        )
        self.next_id = 0

    def request(self, method, params=None):
        self.next_id += 1
        message = {"jsonrpc": "2.0", "id": self.next_id, "method": method}
        if params is not None:
            message["params"] = params
        self.proc.stdin.write(json.dumps(message) + "\n")
        self.proc.stdin.flush()
        while True:
            line = self.proc.stdout.readline()
            if not line:
                raise SystemExit(f"the MCP shim exited during {method}")
            reply = json.loads(line)
            if reply.get("id") == self.next_id:
                if "error" in reply:
                    raise SystemExit(f"{method}: {reply['error']}")
                return reply["result"]

    def notify(self, method):
        self.proc.stdin.write(json.dumps({"jsonrpc": "2.0", "method": method}) + "\n")
        self.proc.stdin.flush()

    def call(self, name, arguments, expect_error=False):
        result = self.request("tools/call", {"name": name, "arguments": arguments})
        is_error = bool(result.get("isError"))
        text = result["content"][0]["text"] if result.get("content") else ""
        if is_error != expect_error:
            raise SystemExit(f"{name}: isError={is_error}: {text}")
        return result.get("structuredContent") if not is_error else text

    def close(self):
        self.proc.stdin.close()
        try:
            self.proc.wait(timeout=20)
        except subprocess.TimeoutExpired:
            self.proc.kill()


def normalise(value, port):
    """Take out what legitimately differs between two runs."""
    if isinstance(value, dict):
        return {key: normalise(item, port) for key, item in value.items()}
    if isinstance(value, list):
        return [normalise(item, port) for item in value]
    if isinstance(value, str):
        value = value.replace(f"127.0.0.1:{port}", "FIXTURE")
        value = re.sub(r"uid=s\d+_\d+(-\d+)?", "uid=*", value)
        return value
    return value


def run(args):
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), Fixture)
    port = server.server_address[1]
    threading.Thread(target=server.serve_forever, daemon=True).start()
    base = f"http://127.0.0.1:{port}"

    artifacts = tempfile.mkdtemp(prefix="loams-browser-e2e-")
    env = dict(os.environ)
    env.update(
        {
            "LOAMS_BROWSER_PROVIDER": args.provider,
            # The fixture is on loopback; the metadata endpoint stays refused
            # whatever this says (egress.rs, check_always_denied).
            "LOAMS_BROWSER_ALLOW_PRIVATE_HOSTS": "1",
            "LOAMS_BROWSER_ARTIFACT_DIR": artifacts,
            "LOAMS_BROWSER_RECORD": "1",
        }
    )
    if args.provider == "service":
        env.setdefault("LOAMS_BROWSER_SERVICE_TOKEN_REF", "env:browser-token")

    shim = Shim([args.agentd, "mcp"], env)
    out = {}
    try:
        shim.request(
            "initialize",
            {
                "protocolVersion": "2025-06-18",
                "capabilities": {},
                "clientInfo": {"name": "loams-browser-e2e", "version": "1"},
            },
        )
        shim.notify("notifications/initialized")
        listed = sorted(
            tool["name"]
            for tool in shim.request("tools/list")["tools"]
            if tool["name"].startswith("browser_")
        )
        out["tools"] = listed
        if "browser_open" not in listed:
            raise SystemExit(f"no browser tools listed: {listed}")

        opened = shim.call("browser_open", {"url": f"{base}/index.html"})
        page = opened["page"]
        provider = opened.pop("provider")
        if provider != args.provider:
            raise SystemExit(f"asked for {args.provider}, got {provider}")
        opened.pop("page")
        out["open"] = opened

        out["navigate"] = shim.call("browser_navigate", {"page": page, "url": f"{base}/next.html"})
        out["snapshot"] = shim.call("browser_snapshot", {"page": page})

        shot = shim.call("browser_screenshot", {"page": page})
        with open(shot["path"], "rb") as handle:
            head = handle.read(8)
        out["screenshot"] = {
            "png": head == b"\x89PNG\r\n\x1a\n",
            "nonempty": shot["bytes"] > 0,
        }
        if not out["screenshot"]["png"]:
            raise SystemExit(f"the screenshot is not a PNG: {head!r}")

        out["extract"] = shim.call("browser_extract", {"page": page})

        refused = shim.call(
            "browser_navigate",
            {"page": page, "url": "http://169.254.169.254/latest/meta-data/"},
            expect_error=True,
        )
        out["metadata_refused"] = "egress" in refused

        shim.call("browser_close", {"page": page})
        after = shim.call("browser_extract", {"page": page}, expect_error=True)
        out["closed"] = "not open" in after

        logs = [name for name in os.listdir(artifacts) if name.endswith(".actions.jsonl")]
        out["action_log"] = len(logs) == 1
    finally:
        shim.close()
        server.shutdown()

    result = normalise(out, port)
    with open(args.out, "w", encoding="utf-8") as handle:
        json.dump(result, handle, indent=2, sort_keys=True)
        handle.write("\n")
    print(json.dumps(result, indent=2, sort_keys=True))
    failed = [key for key in ("metadata_refused", "closed", "action_log") if not result[key]]
    if failed:
        raise SystemExit(f"{args.provider}: failed {failed}")


def compare(paths):
    loaded = []
    for path in paths:
        with open(path, encoding="utf-8") as handle:
            loaded.append(json.load(handle))
    first = loaded[0]
    differ = False
    for path, other in zip(paths[1:], loaded[1:]):
        for key in sorted(set(first) | set(other)):
            if first.get(key) != other.get(key):
                differ = True
                print(f"DIFFERS {key}:\n  {paths[0]}: {first.get(key)!r}\n  {path}: {other.get(key)!r}")
    if differ:
        raise SystemExit("local and service answered differently")
    print(f"identical: {', '.join(paths)}")


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--agentd")
    parser.add_argument("--provider", choices=["local", "service"])
    parser.add_argument("--out")
    parser.add_argument("--compare", nargs="+")
    args = parser.parse_args()
    if args.compare:
        compare(args.compare)
        return
    if not (args.agentd and args.provider and args.out):
        parser.error("--agentd, --provider and --out are required")
    run(args)


if __name__ == "__main__":
    main()
