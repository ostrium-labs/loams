#!/usr/bin/env python3
"""LC0 Task 5 (partial): give OpenFGA its own database and SQL user on a
Starter instance, and write the datastore settings to a 0600 env file for
`podman run --env-file`. The password is generated here and never printed.

Usage: openfga_setup.py <cluster-id> <user-prefix> <env-file> [--drop]
"""

import os
import sys

sys.path.insert(0, os.path.join(os.path.dirname(os.path.abspath(__file__)), "..", "lc0-cells"))
import cells  # noqa: E402

cluster_id, prefix, env_path = sys.argv[1:4]
drop = "--drop" in sys.argv
cell = cells.load_cell(cluster_id, "gateway01.us-east-1.prod.aws.tidbcloud.com", 4000, prefix)
adm = cell.admin()
user = f"{prefix}.lc0pfga"
if drop:
    cells.try_sql(adm, "DROP DATABASE IF EXISTS lc0p_openfga")
    cells.try_sql(adm, "DROP USER IF EXISTS %s@'%%'", (user,))
    print("dropped")
    sys.exit(0)
pw = cells.gen_password()
cells.q(adm, "CREATE DATABASE IF NOT EXISTS lc0p_openfga")
cells.try_sql(adm, "DROP USER IF EXISTS %s@'%%'", (user,))
cells.q(adm, "CREATE USER %s@'%%' IDENTIFIED BY %s", (user, pw))
cells.q(adm, "GRANT ALL PRIVILEGES ON lc0p_openfga.* TO %s@'%%'", (user,))
fd = os.open(env_path, os.O_WRONLY | os.O_CREAT | os.O_TRUNC, 0o600)
os.write(
    fd,
    (
        "OPENFGA_DATASTORE_ENGINE=mysql\n"
        f"OPENFGA_DATASTORE_URI={user}:{pw}@tcp({cell.host}:4000)/lc0p_openfga?parseTime=true&tls=true\n"
    ).encode(),
)
os.close(fd)
print("ok")
