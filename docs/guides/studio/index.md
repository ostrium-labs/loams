# Loams Studio

Loams Studio is the agentic development environment (ADE) in the Loams console. It runs coding agents (Claude Code, Codex, opencode, Pi, the ACP agents and Loams' own agent) in threads, each in its own git worktree. Around them it gives you a chat timeline, a composer, a diff panel with review comments, git actions, per-turn checkpoints, terminals, a file viewer, a preview and parallel runs you compare side by side.

The design is [§53](../../design/53-studio-ade.md) (D900–D922) and the plan is [SD1](../../plans/2026-10-13-sd1-studio-ade.md). These guides describe the code on `dev` as of 2026-10-15; anything not built yet is marked **Pending**.

## One console, two modes

The console has two modes, **Console** and **Studio** (D900). They share one window, one sign-in, one header and one command palette. Switching between them never reloads the page, so running agents, open terminals and unsent drafts survive the switch.

- **Switch** with the segmented control in the header, with **Mod+Shift+M** (Ctrl on Linux and Windows, Cmd on macOS), or with "Switch to Studio" / "Switch to Console" in the palette (**Mod+K**). The switcher shows only when both modes are available.
- **Studio's addresses** all start with `/studio` (`#/studio/t/<threadId>` for a thread), so a link opens its own mode.
- **Which mode opens first:** the address decides when it names one. At the console's root, the last mode you used on this device decides (it is kept under `loams.shell.mode`). Without a stored mode, the desktop opens Studio and the browser opens Console (D902, Q801).
- **The factory apps** (Forgejo, Zulip, Plane, GlitchTip, OpenPanel, Langfuse, Matomo and SigNoz, the last as a preview on Loams House) live under Studio on the desktop. See [The factory in Studio](factory.md).

## Where Studio runs

Studio does all its work through `loams-agentd`, the per-user agent daemon (D906, D907). It talks to the daemon over `loams.agentd.v1`, a Connect API.

| | Desktop (Loams Desktop) | Browser |
|---|---|---|
| The daemon | The local `loams-agentd`, started by the app | A **remote** `loams-agentd` you run on a machine you control ([Remote agentd](remote-agentd.md)) |
| How the console reaches it | Electron main's protocol proxy, which adds the daemon's token; the page never sees the token (D908) | Directly, over TLS, with your own access token for the audience `loams-agentd` (D909) |
| Projects | Local folders, or clones | Clones on the remote machine |
| Availability | On by default | Behind the instance feature `studio` until SD1 Task 32 (see [Getting started](getting-started.md#in-a-browser)) |

## The guides

| Guide | What it covers |
|---|---|
| [Getting started](getting-started.md) | Studio on the desktop and in a browser; the dev preview |
| [Projects and threads](projects-and-threads.md) | Projects, the thread list, worktrees, the thread page |
| [The composer](composer.md) | Writing prompts: mentions, attachments, queueing, steering, drafts |
| [Diffs and review](diffs-and-review.md) | The diff panel, review comments, sending a review to the agent or a pull request |
| [Git actions](git.md) | Commit, push, pull requests, merge, branches and worktrees |
| [Checkpoints](checkpoints.md) | Per-turn checkpoints and restoring a turn |
| [Terminals](terminal.md) | The terminal drawer |
| [Files](files.md) | The file tree, the viewer and content search |
| [Preview](preview.md) | Dev-server previews on the desktop and through a remote agentd |
| [Parallel runs](parallel-runs.md) | Run groups: one prompt, several agents, compare and pick |
| [Approvals](approvals.md) | Supervised and full-access threads |
| [Notifications](notifications.md) | The bell, and OS notifications on the desktop |
| [Keybindings and the palette](keybindings-and-palette.md) | Shortcuts, the command palette, rebinding |
| [Settings](settings.md) | Studio settings |
| [The factory in Studio](factory.md) | The software factory apps, and starting threads from issues and errors |
| [Remote agentd](remote-agentd.md) | Running `loams-agentd serve --remote` for Studio in a browser |

## What is pending

These parts of SD1 are not on `dev` yet:

- **An approval follow-up** (SD1 ruling T13-16): no mode switch in the thread header. See [Approvals](approvals.md#not-built-yet).
- **A sign-in that issues the agentd token** (MT1). Until then the browser asks you to paste a token per host.
- **Studio in the browser by default** (SD1 Task 32), and the factory apps in the browser (SF1, D922).
- **End-to-end tests on both editions** (SD1 Task 29). The security review is [Studio's threat model](../../security/studio-threat-model.md) (SD1 Task 30).
