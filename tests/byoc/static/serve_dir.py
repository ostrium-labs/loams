#!/usr/bin/env python3
"""Serves a directory over HTTP on 127.0.0.1 and writes the port to a file.

    serve_dir.py DIR PORT_FILE
"""
import functools
import http.server
import sys

handler = functools.partial(http.server.SimpleHTTPRequestHandler, directory=sys.argv[1])
handler.log_message = lambda *a: None
srv = http.server.ThreadingHTTPServer(("127.0.0.1", 0), handler)
open(sys.argv[2], "w").write(str(srv.server_port))
srv.serve_forever()
