# approvals

Lists approvals read-only with the SDK's pagination helper and shows typed error handling.
Read-only. `decideApproval` (idempotent, auto-minted key) and `watchApprovals`
(resumable stream) are available on `$client->approvals` for a fuller app.

```sh
php examples/approvals/main.php
```
