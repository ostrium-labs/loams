#!/usr/bin/env python3
"""A minimal OTLP/HTTP JSON trace sink for diagnosis: appends every span the
House sends (`--otlp-endpoint http://127.0.0.1:PORT`) to a JSON-lines file,
one span per line: trace, span, parent, name, start/end ns, attributes.
Usage: otlp_sink.py <port> <out.jsonl>"""
import json, sys
from http.server import BaseHTTPRequestHandler, HTTPServer

port, out = int(sys.argv[1]), sys.argv[2]

class H(BaseHTTPRequestHandler):
    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        try:
            doc = json.loads(body)
            with open(out, "a") as f:
                for rs in doc.get("resourceSpans", []):
                    for ss in rs.get("scopeSpans", []):
                        for s in ss.get("spans", []):
                            attrs = {a["key"]: list(a["value"].values())[0] for a in s.get("attributes", [])}
                            f.write(json.dumps({
                                "trace": s.get("traceId"), "span": s.get("spanId"),
                                "parent": s.get("parentSpanId"), "name": s.get("name"),
                                "start": int(s.get("startTimeUnixNano", 0)),
                                "end": int(s.get("endTimeUnixNano", 0)), "attrs": attrs}) + "\n")
        except Exception as e:  # keep serving
            sys.stderr.write(f"bad body: {e}\n")
        self.send_response(200)
        self.send_header("Content-Type", "application/json")
        self.end_headers()
        self.wfile.write(b"{}")

    def log_message(self, *a):
        pass

HTTPServer(("127.0.0.1", port), H).serve_forever()
