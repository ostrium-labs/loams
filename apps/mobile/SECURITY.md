# Security Policy

## Supported versions

Loams mobile has no release yet. Security fixes land on `main` only.

## Reporting a vulnerability

**Please do not report security vulnerabilities in public issues, pull requests or discussions.**

Report them privately through GitHub's private vulnerability reporting: open the repository's **Security** tab and choose **Report a vulnerability**. If you cannot use GitHub, email **security@ostriumlabs.com**.

Please include the platform (Android or iOS), the app version or commit, steps to reproduce, and your assessment of the impact.

## What to expect

We acknowledge a report within **3 business days**, agree on a disclosure timeline with you (by default an advisory once a fix is on `main`, within **90 days**), and credit reporters unless you ask us not to.

## Scope

Of particular interest:

- bypassing the user-presence check on the decision key (approving without biometrics or the passcode);
- bypassing TLS pinning or the instance key anchor (a swapped or intercepted server accepted);
- push payloads, tokens or keys leaking into logs, backups, screenshots or the notification text seen by Apple, Google or the push gateway;
- replaying or forging an approval decision;
- deep links or intents that perform an action without the user.

Out of scope: the local mock server in `mock/` (test-only, loopback, fake tokens), and issues that need a rooted or jailbroken device.
