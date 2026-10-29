#!/usr/bin/env python3
"""A local stand-in for the three clouds' identity and secret endpoints, for
testing deploy/byoc/lib/loams-byoc-secrets without an account.

    mock_cloud.py PORT_FILE SECRETS_JSON

SECRETS_JSON: {"aws": {"<secret id>": "<value>"}, "gcp": {"<project>/<name>": ...},
"azure": {"<name>": ...}}. Writes the chosen port to PORT_FILE once listening.
It checks what the real services would check: IMDSv2's token header, SigV4's
Authorization header and the session token, the bearer token on Secret Manager
and Key Vault, the Metadata headers. Each failed check answers 401/403.
"""
import base64
import json
import sys
from http.server import BaseHTTPRequestHandler, HTTPServer

secrets = json.load(open(sys.argv[2]))
TOKEN = "imds-token-1"
SESSION = "session-token-1"
BEARER = "Bearer access-token-1"


class H(BaseHTTPRequestHandler):
    def log_message(self, *a):
        pass

    def send(self, code, body=b"", ctype="application/json"):
        if isinstance(body, (dict, list)):
            body = json.dumps(body).encode()
        elif isinstance(body, str):
            body = body.encode()
        self.send_response(code)
        self.send_header("Content-Type", ctype)
        self.send_header("Content-Length", str(len(body)))
        self.end_headers()
        self.wfile.write(body)

    def do_PUT(self):
        if self.path == "/aws/latest/api/token" and self.headers.get("X-aws-ec2-metadata-token-ttl-seconds"):
            return self.send(200, TOKEN, "text/plain")
        self.send(404)

    def do_GET(self):
        p = self.path
        h = self.headers
        if p.startswith("/aws/latest/meta-data/iam/security-credentials/"):
            if h.get("X-aws-ec2-metadata-token") != TOKEN:
                return self.send(401)
            if p.endswith("security-credentials/"):
                return self.send(200, "loams-role", "text/plain")
            return self.send(200, {"AccessKeyId": "ASIAMOCK", "SecretAccessKey": "mock-secret", "Token": SESSION})
        if p.startswith("/gcp/computeMetadata/v1/"):
            if h.get("Metadata-Flavor") != "Google":
                return self.send(403)
            if p.endswith("/service-accounts/default/token"):
                return self.send(200, {"access_token": "access-token-1", "expires_in": 3599, "token_type": "Bearer"})
            if p.endswith("/project/project-id"):
                return self.send(200, "mock-project", "text/plain")
        if p.startswith("/gcp-sm/v1/projects/"):
            if h.get("Authorization") != BEARER:
                return self.send(401)
            # /gcp-sm/v1/projects/<proj>/secrets/<name>/versions/latest:access
            parts = p.split("?")[0].split("/")
            key = f"{parts[4]}/{parts[6]}"
            if key in secrets["gcp"]:
                data = base64.b64encode(secrets["gcp"][key].encode()).decode()
                return self.send(200, {"name": "x", "payload": {"data": data}})
            return self.send(404, {"error": {"code": 404, "status": "NOT_FOUND"}})
        if p.startswith("/azure/metadata/identity/oauth2/token"):
            if h.get("Metadata") != "true" or "resource=https%3A%2F%2Fvault.azure.net" not in p:
                return self.send(400)
            return self.send(200, {"access_token": "access-token-1", "token_type": "Bearer"})
        if p.startswith("/azure-kv/secrets/"):
            if h.get("Authorization") != BEARER or "api-version=" not in p:
                return self.send(401)
            name = p.split("?")[0].split("/")[3]
            if name in secrets["azure"]:
                return self.send(200, {"value": secrets["azure"][name], "id": "x"})
            return self.send(404, {"error": {"code": "SecretNotFound"}})
        self.send(404)

    def do_POST(self):
        n = int(self.headers.get("Content-Length", 0))
        body = self.rfile.read(n)
        if self.path == "/aws-sm/":
            auth = self.headers.get("Authorization", "")
            if not auth.startswith("AWS4-HMAC-SHA256 ") or "/secretsmanager/aws4_request" not in auth:
                return self.send(403, {"__type": "InvalidSignatureException"})
            if self.headers.get("x-amz-security-token") != SESSION:
                return self.send(400, {"__type": "UnrecognizedClientException"})
            if self.headers.get("X-Amz-Target") != "secretsmanager.GetSecretValue":
                return self.send(400, {"__type": "UnknownOperationException"})
            sid = json.loads(body)["SecretId"]
            if sid in secrets["aws"]:
                return self.send(200, {"Name": sid, "SecretString": secrets["aws"][sid]})
            return self.send(400, {"__type": "ResourceNotFoundException", "Message": "Secrets Manager can't find the specified secret."})
        self.send(404)


srv = HTTPServer(("127.0.0.1", 0), H)
open(sys.argv[1], "w").write(str(srv.server_port))
srv.serve_forever()
