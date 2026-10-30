#!/usr/bin/env python3
"""A raw Postgres wire probe for scripts/nf1/pgdog-majors.sh (NF2 Task 12).

libpq does not show the protocol version it negotiated nor the length of the
cancel key it got, so this probe speaks the protocol itself (stdlib only):

    pgdog_wire.py --host H --port P --user U --password W --db D \
        --protocol 3.0|3.2 [--tls-ca FILE] [--cancel tls|plain|none]

It sends a StartupMessage at the asked version (over TLS when --tls-ca is
given), logs in with SCRAM-SHA-256, and prints one JSON object:

    {"connected": true, "asked": "3.2", "negotiated": "3.0",
     "negotiate_message": true, "key_len": 4, "server_version": "18.6",
     "cancel": "57014", "error": null}

`negotiated` is the version the server's NegotiateProtocolVersion message
offered (or the asked one when none came), `key_len` the length of the secret
key in BackendKeyData, and `cancel` the SQLSTATE of `SELECT pg_sleep(20)` after
a CancelRequest with that key (57014 when the cancel worked, "completed" when
the statement finished, "timeout" when nothing came back in 30 s).
"""

from __future__ import annotations

import argparse
import base64
import hashlib
import hmac
import json
import os
import socket
import ssl
import struct
import sys
import threading
import time

VERSIONS = {"3.0": (3 << 16) | 0, "3.2": (3 << 16) | 2}
SSL_REQUEST = 80877103
CANCEL_REQUEST = 80877102


class WireError(Exception):
    pass


def _open(host: str, port: int, ca: str | None) -> socket.socket:
    raw = socket.create_connection((host, port), timeout=30)
    if ca is None:
        return raw
    raw.sendall(struct.pack("!ii", 8, SSL_REQUEST))
    answer = raw.recv(1)
    if answer != b"S":
        raw.close()
        raise WireError(f"SSLRequest refused ({answer!r})")
    ctx = ssl.create_default_context(cafile=ca)
    ctx.minimum_version = ssl.TLSVersion.TLSv1_2
    ctx.check_hostname = False
    ctx.verify_flags &= ~ssl.VERIFY_X509_STRICT
    return ctx.wrap_socket(raw)


def _read_exact(sock: socket.socket, n: int) -> bytes:
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            raise WireError("connection closed")
        buf += chunk
    return buf


def _read_msg(sock: socket.socket) -> tuple[bytes, bytes]:
    head = _read_exact(sock, 5)
    kind, length = head[:1], struct.unpack("!i", head[1:])[0]
    return kind, _read_exact(sock, length - 4)


def _send(sock: socket.socket, kind: bytes, body: bytes) -> None:
    sock.sendall(kind + struct.pack("!i", len(body) + 4) + body)


def _fields(body: bytes) -> dict[str, str]:
    out = {}
    for part in body.split(b"\0"):
        if part:
            out[chr(part[0])] = part[1:].decode(errors="replace")
    return out


def _scram(sock: socket.socket, user: str, password: str) -> None:
    nonce = base64.b64encode(os.urandom(18)).decode()
    first_bare = f"n=,r={nonce}"
    first = ("n,," + first_bare).encode()
    mech = b"SCRAM-SHA-256\0"
    _send(sock, b"p", mech + struct.pack("!i", len(first)) + first)
    kind, body = _read_msg(sock)
    if kind == b"E":
        raise WireError(_fields(body).get("C", "error"))
    code = struct.unpack("!i", body[:4])[0]
    if code != 11:
        raise WireError(f"unexpected SASL step {code}")
    server_first = body[4:].decode()
    attrs = dict(kv.split("=", 1) for kv in server_first.split(","))
    salt = base64.b64decode(attrs["s"])
    salted = hashlib.pbkdf2_hmac("sha256", password.encode(), salt, int(attrs["i"]))
    client_key = hmac.new(salted, b"Client Key", hashlib.sha256).digest()
    stored = hashlib.sha256(client_key).digest()
    final_bare = f"c=biws,r={attrs['r']}"
    auth_msg = f"{first_bare},{server_first},{final_bare}".encode()
    sig = hmac.new(stored, auth_msg, hashlib.sha256).digest()
    proof = base64.b64encode(bytes(a ^ b for a, b in zip(client_key, sig))).decode()
    _send(sock, b"p", f"{final_bare},p={proof}".encode())
    kind, body = _read_msg(sock)
    if kind == b"E":
        raise WireError(_fields(body).get("C", "error"))


def startup(args, protocol: str) -> tuple[socket.socket, dict]:
    sock = _open(args.host, args.port, args.tls_ca)
    params = b"".join(
        k.encode() + b"\0" + v.encode() + b"\0"
        for k, v in (("user", args.user), ("database", args.db))
    ) + b"\0"
    body = struct.pack("!i", VERSIONS[protocol]) + params
    sock.sendall(struct.pack("!i", len(body) + 4) + body)
    info = {"negotiate_message": False, "negotiated": protocol, "key": None}
    while True:
        kind, body = _read_msg(sock)
        if kind == b"R":
            code = struct.unpack("!i", body[:4])[0]
            if code == 10:
                _scram(sock, args.user, args.password)
            elif code == 3:
                _send(sock, b"p", args.password.encode() + b"\0")
            elif code != 0:
                raise WireError(f"unsupported auth {code}")
        elif kind == b"v":
            minor = struct.unpack("!i", body[:4])[0] & 0xFFFF
            info["negotiate_message"] = True
            info["negotiated"] = f"3.{minor}"
        elif kind == b"K":
            info["pid"] = struct.unpack("!i", body[:4])[0]
            info["key"] = body[4:]
        elif kind == b"S":
            k, _, v = body.partition(b"\0")
            if k == b"server_version":
                info["server_version"] = v.rstrip(b"\0").decode()
        elif kind == b"E":
            raise WireError(_fields(body).get("C", "error"))
        elif kind == b"Z":
            return sock, info


def cancel(args, info: dict, mode: str) -> str:
    sock = _open(args.host, args.port, args.tls_ca if mode == "tls" else None)
    key = info["key"]
    body = struct.pack("!ii", CANCEL_REQUEST, info["pid"]) + key
    sock.sendall(struct.pack("!i", len(body) + 4) + body)
    try:
        sock.recv(1)
    except OSError:
        pass
    sock.close()
    return "sent"


def run(args) -> dict:
    out = {"connected": False, "asked": args.protocol, "error": None}
    try:
        sock, info = startup(args, args.protocol)
    except (WireError, OSError) as exc:
        out["error"] = str(exc)
        return out
    out.update(
        connected=True,
        negotiated=info["negotiated"],
        negotiate_message=info["negotiate_message"],
        key_len=len(info["key"]) if info["key"] is not None else None,
        server_version=info.get("server_version"),
    )
    if args.cancel == "none" or info["key"] is None:
        out["cancel"] = None
        sock.close()
        return out
    _send(sock, b"Q", b"SELECT pg_sleep(20)\0")
    timer = threading.Timer(1.5, lambda: cancel(args, info, args.cancel))
    started = time.monotonic()
    timer.start()
    result = "timeout"
    try:
        while time.monotonic() - started < 30:
            kind, body = _read_msg(sock)
            if kind == b"E":
                result = _fields(body).get("C", "error")
            elif kind == b"C" and result == "timeout":
                result = "completed"
            elif kind == b"Z":
                break
    except (WireError, OSError) as exc:
        result = f"closed: {exc}"
    timer.cancel()
    out["cancel"] = result
    sock.close()
    return out


def main(argv: list[str]) -> int:
    p = argparse.ArgumentParser()
    p.add_argument("--host", default="127.0.0.1")
    p.add_argument("--port", type=int, required=True)
    p.add_argument("--user", required=True)
    p.add_argument("--password", required=True)
    p.add_argument("--db", required=True)
    p.add_argument("--protocol", choices=sorted(VERSIONS), default="3.0")
    p.add_argument("--tls-ca")
    p.add_argument("--cancel", choices=["tls", "plain", "none"], default="none")
    args = p.parse_args(argv)
    print(json.dumps(run(args), sort_keys=True))
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
