#!/usr/bin/env python3
"""Seed a scratch Headscale database with one node per tag and one per user.

`headscale policy check` can only evaluate a policy's `tests` block against
nodes that exist. The checker (check.sh) therefore builds a throwaway database
and registers a fake node for every `tag:*` and every operator email found in
the policy file. Nothing here touches a real Headscale.

Usage: seed.py POLICY_FILE DB_FILE
"""
import ipaddress
import json
import re
import sqlite3
import sys

policy_path, db_path = sys.argv[1:3]
text = open(policy_path, encoding="utf-8").read()
# Drop // line comments (the policy contains no "//" inside strings).
text = re.sub(r"//[^\n]*", "", text)

tags = sorted(set(re.findall(r"tag:[a-z0-9][a-z0-9-]*", text)))
emails = sorted(set(re.findall(r"[A-Za-z0-9._-]+@[A-Za-z0-9.-]+", text)))

db = sqlite3.connect(db_path)
cur = db.cursor()

def user_id(name):
    row = cur.execute("select id from users where name = ?", (name,)).fetchone()
    if row:
        return row[0]
    cur.execute("insert into users (name, created_at, updated_at) values (?, datetime('now'), datetime('now'))", (name,))
    return cur.lastrowid

tagged = user_id("tagged-devices")
n = 0

def add_node(host, uid, node_tags):
    global n
    n += 1
    key = f"{n:064x}"
    v4 = str(ipaddress.ip_address("100.64.0.0") + n)
    v6 = str(ipaddress.ip_address("fd7a:115c:a1e0::") + n)
    cur.execute(
        "insert into nodes (machine_key, node_key, disco_key, hostname, given_name, user_id,"
        " register_method, tags, ipv4, ipv6, approved_routes, created_at, updated_at)"
        " values (?,?,?,?,?,?,?,?,?,?,?, datetime('now'), datetime('now'))",
        (f"mkey:{key}", f"nodekey:{key}", f"discokey:{key}", host, host, uid,
         "authkey", json.dumps(node_tags), v4, v6, "[]"),
    )

for t in tags:
    add_node("t-" + t.removeprefix("tag:"), tagged, [t])
for e in emails:
    add_node("u-" + e.split("@")[0], user_id(e), [])

db.commit()
print(f"seeded {len(tags)} tagged nodes and {len(emails)} user nodes")
