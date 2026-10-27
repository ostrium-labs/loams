# Email-domain lists: pins and licences

The sign-in and waitlist policy (design 78 §25.4, D1753) reads these files. The pins are recorded in plan LC1, ruling R-LC2a-10. `update.sh` fetches a pinned revision and checks its SHA-256; a monthly PR bumps the pins.

| File | Upstream | Revision | Licence | SHA-256 |
|---|---|---|---|---|
| `disposable_email_blocklist.conf` | [disposable-email-domains/disposable-email-domains](https://github.com/disposable-email-domains/disposable-email-domains) | `8c05972cf08375d540d50b96b15b7db360fb0a29` (2026-10-22) | CC0-1.0 (`LICENSE-disposable-email-domains.txt`) | `569254edf8f02454f73555e421abab0e78a28f670c9f9e52fb7ca1b91374c7a0` |
| `email-domains.toml` | Ours | — | Apache-2.0 | — |

**Not vendored yet:** [Kikobeats/free-email-domains](https://github.com/Kikobeats/free-email-domains) (MIT) at `18d04cbaa077cca754e129a417fbdf780dea332a`. Its only free-mail source is HubSpot's CSV, which carries no licence (R-LC2a-10), so it waits on counsel (Q1276): accept the HubSpot entries, or build our own free-mail set. Until then the email-code rule refuses the `block` families of `email-domains.toml`, the disposable list, and domains without MX.
