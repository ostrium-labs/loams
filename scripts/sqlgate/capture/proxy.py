#!/usr/bin/env python3
"""Recording MySQL proxy for the gate codec fixtures (SQ1 Task 3).

Usage: proxy.py <listen port> <upstream port> <out file> [--ssl-probe | --tls-sha2 <cert> <key>]

Accepts one client on 127.0.0.1:<listen port>, connects it to TiDB on
127.0.0.1:<upstream port> and writes every MySQL packet of the first 12 in
either direction as a JSON line {"dir": "s2c"|"c2s", "seq": n, "hex": ...}.
With --ssl-probe the server greeting is rewritten to advertise CLIENT_SSL;
the client's next packet (its SSLRequest) is recorded and both sides are
closed. With --tls-sha2 the greeting advertises CLIENT_SSL and
caching_sha2_password; the proxy terminates the client's TLS with <cert> and
<key>, records the decrypted packets (the client's SHA-2 first response and,
on full authentication, its cleartext password), and relays them to TiDB in
plaintext with CLIENT_SSL cleared and sequence ids shifted down by one (TiDB
never sees the SSLRequest). Test data only: the capture user's password is a
fixed test value.
"""
import json
import socket
import ssl
import sys
import threading

CLIENT_SSL = 0x0800
LIMIT = 12


def read_exact(sock, n):
    buf = b""
    while len(buf) < n:
        chunk = sock.recv(n - len(buf))
        if not chunk:
            return None
        buf += chunk
    return buf


def read_packet(sock):
    head = read_exact(sock, 4)
    if head is None:
        return None
    length = int.from_bytes(head[:3], "little")
    body = read_exact(sock, length)
    if body is None:
        return None
    return head[3], body


def add_ssl(greeting):
    # protocol(1) version NUL conn-id(4) auth-data-1(8) filler(1) caps-lower(2)
    i = greeting.index(b"\0", 1) + 1 + 4 + 8 + 1
    caps = int.from_bytes(greeting[i:i + 2], "little") | CLIENT_SSL
    return greeting[:i] + caps.to_bytes(2, "little") + greeting[i + 2:]


def set_plugin(greeting, plugin):
    # ... reserved(10) nonce-2(12) NUL plugin NUL: the plugin name is last.
    end = greeting.rstrip(b"\0")
    start = end.rindex(b"\0") + 1
    return greeting[:start] + plugin + b"\0"


def main():
    listen, upstream, out = int(sys.argv[1]), int(sys.argv[2]), sys.argv[3]
    probe = "--ssl-probe" in sys.argv
    tls = None
    if "--tls-sha2" in sys.argv:
        i = sys.argv.index("--tls-sha2")
        tls = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
        tls.load_cert_chain(sys.argv[i + 1], sys.argv[i + 2])
    srv = socket.socket()
    srv.setsockopt(socket.SOL_SOCKET, socket.SO_REUSEADDR, 1)
    srv.bind(("127.0.0.1", listen))
    srv.listen(1)
    print("ready", flush=True)
    client, _ = srv.accept()
    up = socket.create_connection(("127.0.0.1", upstream))
    log = open(out, "w")
    lock = threading.Lock()
    count = [0]

    def record(direction, seq, body):
        with lock:
            if count[0] < LIMIT:
                log.write(json.dumps({"dir": direction, "seq": seq, "hex": body.hex()}) + "\n")
                log.flush()
                count[0] += 1

    def send(sock, seq, body):
        sock.sendall(len(body).to_bytes(3, "little") + bytes([seq]) + body)

    seq, greeting = read_packet(up)
    if tls:
        greeting = set_plugin(add_ssl(greeting), b"caching_sha2_password")
        record("s2c", seq, greeting)
        send(client, seq, greeting)
        sslreq = read_packet(client)
        if sslreq is None:
            return
        record("c2s", sslreq[0], sslreq[1])
        client = tls.wrap_socket(client, server_side=True)
        shift = 1
    else:
        record("s2c", seq, greeting)
        send(client, seq, add_ssl(greeting) if probe else greeting)
        shift = 0
    if probe:
        pkt = read_packet(client)
        if pkt:
            record("c2s", pkt[0], pkt[1])
        client.close()
        up.close()
        return

    def pump(src, dst, direction):
        first = True
        while True:
            pkt = read_packet(src)
            if pkt is None:
                break
            seq, body = pkt
            record(direction, seq, body)
            if shift and direction == "c2s":
                if first:
                    # The response: TiDB is plaintext, so clear CLIENT_SSL.
                    caps = int.from_bytes(body[:4], "little") & ~CLIENT_SSL
                    body = caps.to_bytes(4, "little") + body[4:]
                seq -= shift
            elif shift:
                seq += shift
            first = first and direction != "c2s"
            try:
                send(dst, seq & 0xFF, body)
            except OSError:
                break
        for s in (src, dst):
            try:
                s.shutdown(socket.SHUT_RDWR)
            except OSError:
                pass

    t = threading.Thread(target=pump, args=(up, client, "s2c"), daemon=True)
    t.start()
    pump(client, up, "c2s")
    t.join(timeout=5)


if __name__ == "__main__":
    main()
