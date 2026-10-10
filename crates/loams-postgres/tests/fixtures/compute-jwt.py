#!/usr/bin/env python3
"""Re-recording helper for loams-postgres's compute_ctl fixtures (PG2 Task 2).

Writes OUT/config.json: deploy/loams-postgres-dev's compute config with one more Ed25519
key in compute_ctl_config.jwks. Prints a token signed with it, for the
compute id `compute-capture` (compose.capture.yaml sets that hostname), as
pg-control signs per-compute tokens (claims: compute_id, exp).

    python3 -I compute-jwt.py <deploy/loams-postgres-dev/compute/config.json> <OUT dir>
"""
import base64
import json
import sys
import time

from cryptography.hazmat.primitives.asymmetric.ed25519 import Ed25519PrivateKey
from cryptography.hazmat.primitives.serialization import Encoding, PublicFormat


def b64(raw: bytes) -> str:
    return base64.urlsafe_b64encode(raw).rstrip(b"=").decode()


src, out = sys.argv[1], sys.argv[2]
key = Ed25519PrivateKey.generate()
public = key.public_key().public_bytes(Encoding.Raw, PublicFormat.Raw)
config = json.load(open(src))
config["compute_ctl_config"]["jwks"]["keys"].append(
    {"use": "sig", "key_ops": ["verify"], "alg": "EdDSA", "kid": "loams-capture",
     "kty": "OKP", "crv": "Ed25519", "x": b64(public)}
)
json.dump(config, open(f"{out}/config.json", "w"), indent=2)
header = {"alg": "EdDSA", "typ": "JWT", "kid": "loams-capture"}
claims = {"compute_id": "compute-capture", "exp": int(time.time()) + 3600}
signing = b64(json.dumps(header).encode()) + "." + b64(json.dumps(claims).encode())
print(signing + "." + b64(key.sign(signing.encode())))
