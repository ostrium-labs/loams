# 56 — Loams Sandbox: One Isolation Crate for Every Untrusted Process

> **Updated 2026-10-19 ([D1119](13-decision-log.md)): Loams has no open core.** Where this document places metering, billing, the hosted control plane, commercial APIs or other components in the private `loams-platform` repository, or calls something private for open-core reasons, read that as superseded: everything is Apache-2.0, and the metering, billing and control-plane code is moving into the open repo now (design §63, Loams Meter). Ostrium Labs offers services (infrastructure efficiency, SLAs, support), not a paid edition. The rest of the design is unchanged.

Status: **Approved direction** (owner, 2026-10-15) · details **Proposed** · 2026-10-15. Source: the owner's direction of 2026-10-15 (§1.1), which is binding. The owner's decision of the same day: the design is written now, and the implementation plan runs **after v0.1.0-preview.3**. Decisions **D990–D1009** and open questions **Q880–Q894** are declared in the [decision log](13-decision-log.md). Rows that restate the owner's direction are *Approved*. Rows that add detail to it are *Proposed* until the owner confirms them. Plan: [SB1](../plans/2026-10-15-sb1-loams-sandbox.md). **No code is written by this document.**

This document builds on five earlier designs:
- [§49](49-loams-house-production.md) §13 and the [HS1](../plans/2026-10-08-hs1-house-production.md) rulings R1.10, R1.11, R6.1 and R6.6. This is the House worker's L3 sandbox, the one sandbox Loams has built and tested against escapes.
- [§45](45-loams-live-production.md) §3.1 (D681). This is Live's isolated worker.
- [§24](24-cpu-time-runtime.md) (the function tiers T0–T3, D171–D190).
- [§15](15-agent-workspaces.md) §8 (agent workspaces and §15's `loams-sandbox` runtimes, D24).
- [§50](50-loams-desktop-daemon.md) and [§53](53-studio-ade.md) §8.2 (the desktop daemon, and Studio's sandbox levels with Q815).

§16 below reconciles each of them, says where this document amends them, and contradicts none of them silently.

Markers:
- **(verify)** means not checked against a primary source. The plan task that depends on the fact checks it first.
- **(estimate)** means computed or judged, not measured.
- **(target)** is a number this document sets as a gate.

The code facts were checked on `dev` at `c6551a69` (2026-10-15). The research was read on 2026-10-15 (§20).

---

## 1. Summary

### 1.1 The owner's direction (2026-10-15, binding)

1. **One crate, `loams-sandbox`, built around a declarative `SandboxSpec`.** Each adapter compiles the spec into its own mechanism. The spec covers five areas:
   - **fs:** read-only paths, read-write paths, and a private scratch directory;
   - **net:** either none, or through a forwarder or proxy with an allowlist (this generalises House's forwarder);
   - **syscalls:** a profile;
   - **resources:** cgroup v2 `cpu.max`, `memory.max`, `pids.max` and `io.max`, plus rlimits and wall-clock and CPU deadlines;
   - **identity:** a uid map, no capabilities, and `no_new_privs`.
2. **A `Sandbox` trait** with four methods:
   - `spawn(spec, cmd) -> SandboxedChild`;
   - `capabilities()`;
   - `kill()`;
   - `stats()`, which reads cgroup `cpu.stat`.
3. **Adapters form a ladder of strength:**
   - **`linux`** is the default and runs "close to metal". It is unprivileged and uses user, mount (`pivot_root`, read-only binds), PID and network namespaces, cgroup v2, Landlock (filesystem plus TCP), seccomp-bpf, and dropped capabilities.
   - **`wasm`** is wasmtime with fuel or epochs, and Hyperlight-Wasm later.
   - **`gvisor`** is `runsc` around a linux spec.
   - **`microvm`** is Firecracker or Cloud Hypervisor with snapshot restore and fork, for persistent agent workspaces in the style of boat.
   - **`seatbelt`** (macOS) and **`appcontainer`** (Windows AppContainer plus job objects) serve the desktop agent daemon.
4. **The threat model:**
   - For a self-hosted single tenant, `linux` is enough.
   - For multi-tenant hostile code in Loams cloud, the linux spec is stacked **inside** gvisor or microvm, for defence in depth. workerd's own documentation says it is not a hardened sandbox, and §24 already wraps it in gVisor.
5. **The build order:**
   1. Extract House's sandbox (`fabric/crates/loams-house-worker/src/sandbox/`: `cgroup.rs`, `netns.rs`, `landlock.rs`, `seccomp.rs` and `mod.rs`, with its documented step order) into `loams-sandbox::linux`. House's escape tests stay as the baseline.
   2. Move Live JS (`crates/loams-live-js/src/sandbox_linux.rs`) onto it.
   3. Write one escape-test conformance kit that every adapter must pass. Its probes include reading `/etc/shadow`, writing outside scratch, `execve`, a fork bomb, an outbound connect, `ptrace`, and reaching another tenant's cgroup.
   4. Sandbox the agentd sessions (Studio threads): Loams enforces `SandboxLevel` itself instead of trusting each harness's own sandbox (`crates/loams-agentd/src/roles.rs`, §50, §53).
   5. Add gvisor for functions (§24 tiers T0 and T2).
   6. Add microvm with snapshot forking for workspaces (§15).
6. **Limits stay in the open repository. Billing-grade metering stays in `loams-platform`** (§41).

### 1.2 Decisions

| # | Decision | Status | § |
|---|---|---|---|
| D990 | **One crate, `loams-sandbox`, with a declarative `SandboxSpec`** (fs, net, syscalls, resources, identity) that each adapter compiles to its own mechanism. The spec states *what* is allowed, never *how* it is enforced. An adapter that cannot enforce a field refuses the spawn unless the spec marks that field best-effort, so nothing is weakened silently | Approved (owner); fail-closed rule proposed | §6 |
| D991 | **The `Sandbox` trait:** `spawn(spec, cmd) -> SandboxedChild`, `capabilities()`, `kill()`, `stats()` (cgroup `cpu.stat` and its peers). The `linux` adapter adds **`seal_self(spec)`** for processes that must load their code before they are sealed (House's libchdb, Live's QuickJS runtime) | Approved (owner); `seal_self` proposed | §7 |
| D992 | **The adapter ladder:** `linux` (default) → `wasm` → `gvisor` → `microvm`, plus `seatbelt` (macOS) and `appcontainer` (Windows) for the desktop daemon. Each adapter declares what it enforces in `capabilities()`, and the conformance kit proves it | Approved (owner) | §8 |
| D993 | **The threat model.** A self-hosted single tenant runs `linux`. Multi-tenant hostile code in Loams cloud runs the same linux spec **inside** `gvisor` or `microvm`. A cloud deployment refuses to start a multi-tenant product on bare `linux`. This answers Q628: multi-tenant Live runs its isolated worker inside gvisor | Approved (owner); the refusal and Q628 proposed | §5 |
| D994 | **The build order:** (1) extract House's L3 into `loams-sandbox::linux`, keeping its escape tests; (2) move Live onto it; (3) one conformance kit; (4) agentd sessions enforce `SandboxLevel`; (5) gvisor for functions T0 and T2; (6) microvm with snapshot forking for workspaces | Approved (owner) | §15 |
| D995 | **Limits are open; billing-grade metering is private.** `stats()` returns plain operational numbers (CPU µs, throttling, memory and PID peaks). Nothing in this crate aggregates, signs, rates or bills them (D190, D541, D548) | Approved (owner) | §11 |
| D996 | **The `linux` adapter spawns through a re-exec trampoline, not `pre_exec`.** The host binary's `main` hands control to `loams_sandbox::linux::trampoline()` when it is started as `__loams-sandbox-exec`. Stage 1 enters the namespaces. Stage 2 runs as PID 1 of the new PID namespace, builds the root with `pivot_root`, seals itself and starts the payload. With this design no `unsafe` code is needed, and every step runs in a fresh single-threaded process | Proposed | §8.1 |
| D997 | **Three named syscall profiles** compiled with `seccompiler`. `worker` is House's deny-list (`EPERM`; no `execve` or new processes). `interpreter` is Live's allowlist (`KILL_PROCESS`). `shell` is for agents: exec and fork are allowed, and the escape and kernel-attack surface is denied. Every profile answers `ENOSYS` for io_uring and `clone3`, and refuses `ioctl(TIOCSTI)` and `TIOCLINUX` | Proposed | §6.3 |
| D998 | **The network is `None`, `Forward` or `HostPorts`.** `Forward` generalises House's forwarder:<br>• a listener on the sandbox's own loopback;<br>• a channel that passes file descriptors to the host;<br>• a host-side `ForwardTarget`: a fixed relay, House's `house-cache`, or the new allowlisting HTTP CONNECT/SOCKS5 proxy.<br>The host resolves names. Private, link-local and metadata addresses are refused unless they are explicitly listed. `HostPorts` is House's `pods` mode | Proposed | §9 |
| D999 | **Resources live in a delegated cgroup v2 subtree.** The host creates one child per sandbox and writes `cpu.max`, `memory.max`, `memory.swap.max`, `pids.max` and `io.max` before the payload starts. The child joins it first. `kill()` is `cgroup.kill`. rlimits are set (`RLIMIT_CORE` 0 always). The wall-clock deadline is a host timer, and the CPU deadline is a `cpu.stat` watchdog. Without a delegated subtree the resource fields fail closed (D990) | Proposed | §10 |
| D1000 | **Identity is unprivileged.** The sandbox's uid and gid are mapped onto themselves, with `setgroups` denied, so the payload is never root inside. Every capability is dropped, ambient ones included. `no_new_privs` is always set. A distinct host uid comes from the deployment (`runAsUser`, a service user), not from the crate. Subordinate ranges (`newuidmap`) are not used in SB1 (Q886) | Proposed | §6.5 |
| D1001 | **One conformance kit, `loams-sandbox-conformance`, with a probe binary, `loams-sandbox-probe`.** About thirty named escape probes run against every adapter and every syscall profile. Each probe passes, or the adapter's `capabilities()` declares it not applicable and gives a reason. House's and Live's existing escape tests are kept as the baseline and also run as kit probes | Proposed | §12 |
| D1002 | **House moves onto the crate unchanged in behaviour.** `--sandbox=netns\|pods\|none` stays as it is and maps to specs: `netns` becomes `linux`, `worker` and `Forward(house-cache)`; `pods` becomes `linux`, `worker` and `HostPorts([s3 port])`; `none` becomes `Unsandboxed`, with the existing warning. `loams_chdb_sys::capabilities::drop_all` is replaced by the crate's safe capability drop. The fabric workspace takes `loams-sandbox` by path | Proposed | §13.1 |
| D1003 | **Live moves onto the crate and gets the limits §45 promised.** The `interpreter` profile, Landlock with no rights at all, `RLIMIT_NOFILE` 3 and `RLIMIT_AS` are unchanged. **`RLIMIT_CPU`**, which §45 §3.1 lists but the code does not set, is added as a backstop at the deployment's CPU limit plus 1 s (target). A cgroup is used where one is delegated | Proposed | §13.2 |
| D1004 | **agentd enforces `SandboxLevel` with the platform adapter** (`linux`, `seatbelt`, `appcontainer`) around the whole harness process tree. `read-only` and `workspace-write` become specs (§13.3). `danger-full-access` runs unsandboxed and is labelled. When Loams enforces, the harness's own sandbox is set to its full-access mode, so two sandboxes never nest. When the adapter is unavailable, the run follows Q885. The thread header shows who enforces (§53 §8.2). **This answers Q815** | Proposed | §13.3 |
| D1005 | **Functions:**<br>• T0 (workerd) runs in `gvisor` around the linux spec, and in `linux` alone where gVisor is absent, which the node reports;<br>• T1 is the `wasm` adapter inside a `linux`-sealed host process;<br>• T2 is `gvisor`;<br>• T3 stays deferred (D174).<br>**This amends §24 §4.1 and §10**: the T0 fallback "seccomp-bpf plus a user namespace and cgroup" is the `linux` adapter | Proposed | §13.4 |
| D1006 | **Workspaces run in `microvm`**, which has `snapshot`, `restore` and `fork`. The guest runs the linux spec again inside it. §15's `Runtime` trait is replaced by `Sandbox` plus the `microvm` extension. §15's in-sandbox agent (layers, proxy wiring, checkpoints) keeps its role under a new name, `loams-workspace-agent`. **This amends D24**: microsandbox is no longer the default backend (Q881) | Proposed | §13.5 |
| D1007 | **Every sandboxed process reports what was enforced.** A `SandboxReport` lists the adapter, the namespaces, the Landlock ABI and status, the seccomp profile, the cgroup controllers and any best-effort fields that were not enforced. The report goes to the log and to each product's status: House's start line, Live's deployment status, agentd's thread header and the function node's status | Proposed | §7.3 |
| D1008 | **Where the code lives, and `unsafe`.** `crates/loams-sandbox` (root workspace) forbids `unsafe`. It uses `landlock` 0.4.7, `seccompiler` 0.5.0 and `rustix` 1, which both workspaces already pin. macOS and Windows FFI goes into `crates/loams-sandbox-sys` (`unsafe_code = "deny"`, local allows with `SAFETY` comments), the pattern of `loams-chdb-sys` and `loams-agentd-harness`. The macOS adapter prefers `/usr/bin/sandbox-exec`, which needs no FFI | Proposed | §17 |
| D1009 | **Host requirements and degradation:**<br>• `linux` with network rules needs Landlock ABI ≥ 4 and unprivileged user namespaces (Ubuntu's AppArmor restriction must allow them; Kubernetes needs a profile that permits `CLONE_NEWUSER`).<br>• When `capabilities()` falls short, each consumer has a fixed policy: House and Live **refuse** (as today), and agentd follows Q885.<br>• No consumer silently runs a weaker sandbox than its spec asks for | Proposed | §8.1.6 |

### 1.3 What it looks like

```
                          SandboxSpec  (fs · net · syscalls · resources · identity)
                                │  compiled by
   ┌──────────────┬─────────────┼──────────────┬──────────────┬───────────────┬────────────────┐
   │ linux        │ wasm        │ gvisor       │ microvm      │ seatbelt      │ appcontainer   │
   │ userns, mnt  │ wasmtime    │ runsc OCI    │ Firecracker /│ sandbox-exec  │ AppContainer   │
   │ (pivot_root),│ fuel/epoch, │ bundle; the  │ Cloud Hyp.;  │ SBPL profile, │ SID + ACLs,    │
   │ pid, net ns; │ WASI caps;  │ linux spec   │ guest runs   │ proxy env,    │ job object     │
   │ cgroup v2;   │ host process│ runs inside  │ the linux    │ rlimits       │ limits, proxy  │
   │ Landlock;    │ sealed by   │ the Sentry   │ spec; snap-  │               │                │
   │ seccomp; no  │ linux       │              │ shot / fork  │               │                │
   │ caps, nnp    │             │              │              │               │                │
   └──────┬───────┴──────┬──────┴──────┬───────┴──────┬───────┴───────┬───────┴────────┬───────┘
          │              │             │              │               │                │
   House workers   Functions T1   Functions T0/T2  Workspaces     Desktop agentd   Desktop agentd
   Live workers                   Cloud Live (Q628) (§15)         (macOS)          (Windows)
   agentd (Linux)
   Functions T0 (self-hosted fallback)

   self-hosted, single tenant:   linux
   Loams cloud, hostile code:    gvisor( linux )   or   microvm( linux )        (D993)
```

```
 one linux spawn (D996)
   host (agentd, House front, supervisor)
     ├─ mkdir cgroup child; write cpu.max, memory.max, pids.max, io.max
     ├─ open forwarder socketpair; host end → ForwardTarget (relay | house-cache | proxy)
     └─ spawn  <self> __loams-sandbox-exec  (spec on fd 3, forwarder on fd 4, argv after --)
           stage 1: one thread? → join cgroup → unshare(user|mnt|pid|net|ipc|uts) → uid/gid map
           └─ spawn <self> __loams-sandbox-init       ← PID 1 of the new PID namespace
                 stage 2: tmpfs root, ro/rw binds, /proc, /dev, pivot_root, umount old root
                          lo up; forwarder listener on 127.0.0.1:<p>
                          RLIMIT_CORE 0 + spec rlimits; drop all caps
                          Landlock (fs + TCP); no_new_privs; seccomp (profile)
                          forwarder thread; spawn payload (PID 2); reap; relay signals
                 └─ payload  (git, node, cargo, the harness CLI …)
```

## 2. Scope

**In scope:**
- every process that Loams starts to run code it does not trust;
- House's chDB workers;
- Live's isolated QuickJS workers;
- the desktop and remote agent daemon's harness runs (Claude Code, Codex, opencode, Pi, the ACP agents);
- §24's function tiers;
- §15's agent workspaces.

**Not in scope:**
- **The product layers above the OS.** House's L1 (deny list) and L2 (chDB grants), Live's frozen globals and interrupt handler, and agentd's approvals (D911) and role checks (`roles.rs`) all stay where they are. This crate is the layer under them. It is never the reason to remove them.
- **The workspace state plane.** §15's layers, environment images, package caches, registry proxy, checkpoints and credential vending are what a workspace *holds*. This document is about what contains the process.
- **Metering** (D995).
- **Kubernetes pod security.** `RuntimeClass`, `securityContext` and NetworkPolicy belong to [§41](41-multitenant-byoc-control-plane.md) and [§38](38-knative-authentik-gitops.md). Where a pod is the boundary, the spec's `HostPorts` mode is what runs inside it (§13.1).
- **Browser frames and plugin trust tiers** (§37 §5.6, §53 §8.3).
- **Studio terminals.** They are the person's own shell. They are not agent code, so they are not sandboxed (Q893).

## 3. Where things are today (checked on `dev` at `c6551a69`, 2026-10-15)

### 3.1 House L3 (HS1 Task 6, §49 §13.2)

| File | What it does |
|---|---|
| `fabric/crates/loams-house-worker/src/sandbox/mod.rs` (177 lines) | `Sandbox::apply(SandboxConfig)`. It runs in the worker's `main` after the sockets are taken and before libchdb boots, in nine documented steps:<br>1. one thread;<br>2. join `--cgroup`;<br>3. `RLIMIT_CORE` 0;<br>4. a user and network namespace, with `lo` up;<br>5. the forwarder's listener;<br>6. every capability dropped;<br>7. Landlock;<br>8. seccomp;<br>9. the forwarder thread.<br>A worker that cannot seal answers `Error` (`LOAMS_SANDBOX`) and exits 77 (R6.1) |
| `…/sandbox/cgroup.rs` (35) | `join(dir)` writes `0` to `cgroup.procs` and checks `/proc/self/cgroup` |
| `…/sandbox/netns.rs` (156) | `enter()` calls `unshare(CLONE_NEWUSER \| CLONE_NEWNET)` and maps the worker's own uid and gid onto themselves after `setgroups deny`. `raise_loopback()` sends one rtnetlink `RTM_NEWLINK` |
| `…/sandbox/landlock.rs` (232) | ABI 4 is required and the rules are written for ABI 9, best effort above 4. Read-write is the private directory only. Read is allowed for every file in `/proc/self/maps`, `READ_DIRS`, `READ_FILES` and `/proc/self`. TCP `connect` is allowed only to the given ports, and there is no `bind`. Signal and abstract-socket scoping (ABI 6) are added |
| `…/sandbox/seccomp.rs` (235) | Two filters with `TSYNC`. **`EPERM`** for `denied()` (exec, fork and vfork, namespaces, mounts, `ptrace`, `process_vm_*`, the pidfd calls, `bpf`, `perf_event_open`, `userfaultfd`, the key ring, modules, `kexec`, clocks and more). The same filter also refuses:<br>• `clone` without `CLONE_THREAD`;<br>• the kill-family calls to any pid but the worker's own;<br>• `socket` outside `AF_INET`/`AF_INET6`;<br>• `tkill`, unless Landlock scopes signals.<br>**`ENOSYS`** for `clone3` and io_uring |
| `fabric/crates/loams-house-worker/src/forwarder.rs` (92) | The worker's half of the relay. It listens on `127.0.0.1:FORWARDER_PORT` (9180) inside the namespace and asks the front for a stream over fd 4 |
| `fabric/crates/loams-house/src/sandbox.rs` (413) | The front's half:<br>• `serve_forwarder` hands out socket pairs over `SCM_RIGHTS` to `Forward::Relay` or `Forward::Target` (`house-cache`), with at most 64 connections per worker;<br>• `CgroupLimits` covers `memory.max`, `memory.swap.max`, `cpu.max` and `pids.max` (no `io.max`), with `WorkerCgroups::prepare` and `create`;<br>• `default_mode()` is `netns` on Linux and `none` elsewhere |
| `fabric/crates/loams-house-ipc/src/lib.rs` | `SandboxMode::{Netns, Pods, None}` and `FORWARDER_PORT` |
| `fabric/crates/loams-house/src/watchdog.rs` | `ProcessLauncher::with_sandbox`, `unsandboxed`, `with_cgroups` |
| `deploy/house/kind/worker-pods.yaml` | The `pods` fallback's NetworkPolicy |

**The escape tests**, which are the baseline the kit keeps (D1001):

- **`fabric/crates/loams-house-worker/tests/sandbox.rs`:**
  - `sandbox_blocks_with_deny_list_off`
  - `worker_reaches_only_its_forwarder`
  - `worker_cannot_exec`
  - `worker_cannot_write_outside_tmp`
  - `cgroup_limits_applied`
  - `pods_fallback_network_policy` (`#[ignore]`, needs kind)
  - `sandbox_tests_are_linux_only`
- **Unit tests:**
  - `landlock::tests::rules_read_the_code_and_write_only_the_private_dir`
  - `netns::tests::the_request_is_one_new_link_with_up`
  - `netns::tests::an_error_answer_is_its_errno`
  - `seccomp::tests::filters_compile_for_this_machine`
- **In `loams-house`:**
  - `limits_name_their_controllers`
  - `a_directory_that_is_not_a_cgroup_is_refused`
  - `the_default_is_sealed_on_linux`
  - `the_forwarder_answers_a_request_with_one_socket`

**In CI**, `.github/workflows/fabric.yml` ("Fabric: fmt, clippy, tests and deny") runs `sudo sysctl -w kernel.apparmor_restrict_unprivileged_userns=0` before the tests. Ubuntu 24.04's AppArmor refuses unprivileged user namespaces otherwise, and `/proc/self/setgroups` returns `EACCES`.

**Differences from §49 §13.2's text.** §13.2 lists "a distinct UID; rlimits". The code sets only `RLIMIT_CORE` and maps the worker's own uid, as R6.6 records: an unprivileged front can map only its own uid, and the deployment's `runAsUser` supplies the distinct uid. §16 amends the text to match the code.

### 3.2 Live's isolated worker (LV1 Task 5, §45 §3.1)

- **`crates/loams-live-js/src/sandbox_linux.rs` (169 lines), `apply(memory_limit)`.** It runs after `Load` and runtime creation, before the bundle is evaluated:
  - it checks that the process has one thread;
  - it creates a Landlock ruleset that handles every filesystem and network right (ABI 6) and grants none, and scopes signals and abstract sockets (best effort; seccomp still applies on a kernel without Landlock);
  - it sets `PR_SET_NO_NEW_PRIVS` (`rustix`);
  - it sets `RLIMIT_NOFILE` = 3, and `RLIMIT_AS` = the mapped size + the memory limit + 64 MiB;
  - it installs a seccomp **allowlist** of 13 calls, `read`/`write` only on fds 0–2, with `KILL_PROCESS` for everything else.
- **`crates/loams-live-js/src/child.rs`** calls it, and refuses on non-Linux: "isolated workers need Linux".
- **`crates/loams-live-js/src/worker.rs`** spawns `loams live-worker` with `env_clear()` and `kill_on_drop`.
- **Tests:**
  - `crates/loams-live-js/tests/sandbox.rs`: `worker_cannot_open_files`, `worker_cannot_open_sockets`, `worker_cannot_exec`;
  - the unit test `sandbox_linux::tests::the_filter_compiles_and_the_address_space_is_read`.
- **Dependencies:** `landlock`, `seccompiler`, `rustix` (`process`, `thread`) and `libc`, on Linux only.

**Differences from §45 §3.1's text.** §3.1 lists `RLIMIT_CPU` and "a separate uid where the operator configures one". The code sets neither. CPU is bounded by the interrupt handler and the host's wall-clock kill (`worker.rs`, row T3-10). D1003 adds `RLIMIT_CPU` as a backstop, and the separate uid stays a deployment matter (D1000).

### 3.3 The agent daemon and Studio (§50, §53)

- **The levels.** `loams_agentd_proto::SandboxLevel` (`crates/loams-agentd-proto/src/agent.rs`) is `ReadOnly`, `WorkspaceWrite` or `DangerFullAccess`. `RunRequest` carries it beside `cwd` and an optional `worktree`.
- **The ceiling check.** `crates/loams-agentd/src/roles.rs` ranks the levels and refuses a run or a new side chat whose sandbox is wider than its session's: "a run's sandbox is wider than the session's". The check stops a run from escalating. It does not confine anything.
- **What the harnesses actually do:**
  - Codex (`crates/loams-agentd-harness/src/codex/mod.rs`) **forces `DangerFullAccess`** unless the thread is supervised or title-only. `catalog.rs` maps the level onto Codex's `read-only` / `workspace-write` / `danger-full-access`, with `networkAccess: true` for workspace-write.
  - Claude Code (`claude/mod.rs`) runs with `--permission-mode bypassPermissions --dangerously-skip-permissions` unless supervised.
  - ACP agents (`acp/mod.rs`) pick the agent's no-prompts mode (`bypassPermissions`, `agent-full-access`, `bypass`).
  - So, as §53 §8.1 says, **no harness is confined today** except by its own optional sandbox in supervised mode.
- **Process ownership:**
  - Unix: `process/owned.rs` puts each harness in its own process group.
  - Windows: `windows_process.rs` creates a kill-on-close **job object** at process creation, using proc-thread attribute lists. The Windows job plumbing exists. A sandboxing token or AppContainer does not.
- **Spawn sites:**
  - `claude/mod.rs` (`Command::new(exe)` at about line 178);
  - `codex/mod.rs` (`app-server`, about 729);
  - `opencode/mod.rs` (`spawn`, about 607);
  - `pi/mod.rs` (about 140);
  - `acp/mod.rs` (about 1389, 1665, 1705);
  - Studio terminals in `crates/loams-agentd-sessions/src/terminals.rs` (`portable_pty::CommandBuilder`).
- **The service unit.** `crates/loams-agentd/src/service/systemd.rs` renders a user unit without `Delegate=yes`, so the daemon owns no cgroup subtree today.
- **Q815** ("OS-level confinement in the daemon … a separate design") is open. This document is that design.

### 3.4 Functions and workspaces

- **No function runtime crate exists.** There is no `loams-runner`, workerd host, wasmtime host or `runsc` integration in either workspace, and no gVisor manifest under `deploy/`. §24's tiers and RN1's `Runner` trait are unbuilt.
- **No `loams-sandbox` crate exists.** §15 W2's `Runtime` trait with microsandbox, Firecracker, gVisor and process backends (D24) is unbuilt. The name is mentioned in §11's buy-versus-build table and in §16's fleet demo.

### 3.5 Two workspaces, two syscall crates

- **The root workspace** (`Cargo.toml`) pins `landlock = "0.4.7"`, `seccompiler = "0.5.0"`, `rustix = "1"` and `libc`.
- **The `fabric/` workspace** pins `landlock 0.4.7`, `seccompiler 0.5.0`, `nix 0.31.3` and `libc`, but not `rustix`.
- Both use `unsafe_code = "forbid"`. House drops capabilities through `loams_chdb_sys::capabilities::drop_all` (`prctl(PR_CAP_AMBIENT_CLEAR_ALL)` and a raw `capset`), the one `unsafe` crate of the fabric workspace.
- Fabric already takes root crates by path (`loams-store`, `loams-cache` in `loams-house-worker`'s dev-dependencies). So `loams-sandbox` can live in the root workspace and be used by both (D1008).

## 4. What others built, and what Loams takes

### 4.1 The systems

| System | Isolation | Network | Notes | What Loams takes |
|---|---|---|---|---|
| **OpenAI Codex CLI** | Linux: the `codex-linux-sandbox` helper. Bubblewrap is now the default filesystem sandbox, with seccomp for network and syscalls; Landlock is the legacy path. macOS: Seatbelt through `/usr/bin/sandbox-exec`. Windows: a restricted token, local sandbox accounts and DACLs, plus job objects | Off by default; seccomp blocks socket families in the restricted modes | Codex's README: "Bubblewrap is the default filesystem sandbox"; "All filter modes block `io_uring`". The secondary write-ups differ on the Linux mechanism. The primary README is authoritative | The three levels (`read-only`, `workspace-write`, `danger-full-access`) agentd already uses. A helper re-exec instead of in-process forking (D996). io_uring blocked. Seatbelt via `sandbox-exec` on macOS |
| **Anthropic `sandbox-runtime` (srt)**, Apache-2.0, research preview (v0.0.76, 2026-09-10) | `bubblewrap` on Linux, `sandbox-exec` on macOS | "By default, all network access is denied": an HTTP proxy and a SOCKS5 proxy outside the sandbox with `allowedDomains` (wildcards) | What Claude Code's sandboxed bash uses | The allowlisting proxy reached through `HTTP_PROXY`/`ALL_PROXY` (D998); deny by default |
| **gVisor** | A user-space kernel (the Sentry). `runsc` is the OCI runtime. **systrap** is the default platform | The Sentry's netstack, or none | `runsc` creates and limits cgroups itself, or through systemd (`--systemd-cgroup`, cgroup v2) | The `gvisor` adapter compiles a spec to an OCI bundle (D1005). `stats()` reads the sandbox's cgroup |
| **Firecracker** (E2B) | KVM microVM, with a `jailer` | tap devices | Snapshot create and restore are supported. Diff snapshots are in developer preview. A snapshot restores only on identical software and hardware. E2B: "Firecracker microVMs that resume from a snapshot, run untrusted agent code" | The `microvm` adapter's first VMM (Q882). Snapshot fork for workspaces |
| **Cloud Hypervisor** | KVM microVM (virtiofs, hotplug) | tap | Snapshot of a paused VM, restored into an identical VM that starts paused | The second `microvm` VMM |
| **Microsoft Hyperlight** (CNCF Sandbox) | A micro-VM per function call, with no guest OS | none | "Hyperlight is pre-1.0. The API may change between releases". The 2025 post reports function calls returning "in just 0.0009 seconds on average". Hyperlight-Wasm: "experimental … not considered production-grade" | A later `wasm` backend (Q887); never for agents, which need an OS (§15 §8) |
| **wasmtime** | Wasm memory safety; WASI capabilities | Through WASI only | Fuel: execution can "consume fuel" to halt or yield. Epochs: "the deadline check cannot be avoided by WebAssembly code" (and are incompatible with Winch) | The `wasm` adapter (§24 T1), with fuel for exact CPU and epochs for cheap deadlines |
| **Alibaba OpenSandbox**, Apache-2.0 | Docker locally, Kubernetes in production; "Firecracker sandboxes support pause/resume with memory and disk state preserved" | — | Open-sourced in March 2026 according to one secondary source **(verify)** | A reference for the workspace lifecycle API (pause, resume) |
| **Sandlock** (Wang and Zheng, arXiv 2605.26298, 2026-05-25) | Unprivileged Linux primitives: static policy compiled to kernel-enforced rules, plus a narrow supervisor | Policy-driven | No root, cgroups or images, with about 5 ms of startup overhead (reported) | Confirms the `linux` direction: unprivileged, kernel-enforced, a small supervisor |
| **boat** (boat.dev, by ASCII) | Persistent Ubuntu VMs reached over SSH | — | "Fast disk-level forking from a snapshot"; `boat fork`, `boat stop` | The workspace model of D1006: a persistent VM, forked from a snapshot |
| **microsandbox** (libkrun) | microVM per sandbox | Egress allowlists | §15's chosen default (D24). Runs on Linux KVM, Apple HVF and Windows WHP | Q881: a `microvm` backend for desktops later, not the default |

**The landscape surveys** agree on a ladder, not a single answer:
- Modal's (May 2026) is a vendor ranking of hosted platforms.
- amux.io (August 2026) compares Docker, Podman, E2B, Firecracker, gVisor, Modal, Daytona and Fly, with the safety models of Claude Code, Codex and Cursor.
- PandaStack (June 2026) concludes that "there is no single best self-hosted code execution sandbox".
- Reinvently (July 2026, updated September) argues that "the model you chose is the security posture" when comparing nono, Docker Sandboxes, OpenSandbox, Firecracker and SmolVM.
- restyler/awesome-sandbox lists about nineteen platforms and notes that Daytona is no longer self-hostable as of June 2026.

### 4.2 Where they converge, and where Loams differs

- **The laptop tools (Codex, srt)** wrap a well-known setuid-free helper (`bubblewrap`) and add a seccomp filter and a proxy. **Loams does the same things in Rust, in-tree**, because it already does so in House:
  - the namespaces, `pivot_root` and binds are what bubblewrap does;
  - Landlock and seccomp are what House already ships.

  This keeps the one escape-tested code path, adds no runtime dependency on a host binary, and lets the same spec drive gVisor and VMs. The cost is that Loams maintains mount-namespace code that bubblewrap has hardened for years (§18).
- **A network proxy, not a firewall.** Every agent tool reaches the network through a proxy that the host controls and that checks names, not addresses. Loams's House forwarder is already this shape.
- **A ladder of strength.** The hosted sandboxes (E2B, Modal, OpenSandbox) use a VM or gVisor per tenant. The laptop tools use OS primitives. Nobody runs strangers' code on bare namespaces in a shared cloud. D993 is the same rule.
- **Snapshots are the workspace primitive** (E2B, boat, OpenSandbox, microsandbox): pause, resume and fork a whole machine.

## 5. Threat model (D993)

### 5.1 Who runs code, and against whom

| Class | Example | Adversary | Required adapter |
|---|---|---|---|
| **A: the operator's own code** | Live `in_process` (D681), a self-hoster's own functions | None beyond bugs | None required; `linux` recommended |
| **B: single tenant, untrusted input** | House workers on a self-hosted stack (SQL from users); agent harness runs on a desktop or remote agentd (a prompt-injected agent with a shell); Live `isolated` single-tenant | The code or the agent, acting for an outsider (prompt injection, a hostile query, a poisoned dependency) | **`linux`** (`seatbelt` and `appcontainer` on desktops) |
| **C: multi-tenant, hostile** | Loams cloud's functions, multi-tenant Live, hosted House workers, hosted workspaces | A paying tenant who wants another tenant's data or the host | **`gvisor(linux)`** or **`microvm(linux)`**; never bare `linux` |

### 5.2 What is protected

1. **Host files and credentials:** `~/.ssh`, cloud credentials (`~/.aws`, `~/.config/gcloud`, `~/.kube`), the daemon's token and keyring, `/etc/shadow`, other projects.
2. **Other tenants:** their processes, cgroups, scratch, sockets, and the data planes' credentials. House workers already hold none, and `house-cache` signs requests in the front.
3. **The network:** SSRF to metadata (`169.254.169.254`, `fd00:ec2::254`), to the LAN and to the stack's internal ports; exfiltration to arbitrary hosts.
4. **The host's availability:** CPU, memory, PIDs, disk I/O and disk space.
5. **The person's terminal:** `TIOCSTI` injection into the controlling tty.

### 5.3 What the `linux` adapter does not stop

The kernel is shared, so a kernel bug reachable through the allowed syscalls is an escape. seccomp shrinks that surface but cannot remove it, and the `shell` profile must allow a general-purpose program's syscalls. Side channels (Spectre-class, cache timing) are also not addressed. This is why class C stacks the spec inside gVisor (a user-space kernel; the host sees only the Sentry's narrow syscall set) or a microVM (a separate guest kernel behind KVM). workerd's README makes the same point about itself: "WARNING: `workerd` is not a hardened sandbox … `workerd` on its own does not contain suitable defense-in-depth against the possibility of implementation bugs."

### 5.4 Stacking

"The linux spec inside gvisor" means the same `SandboxSpec` is applied twice.

- **The outer layer** is gVisor's OCI bundle: mounts, the network mode, cgroup limits and its own seccomp on `runsc`.
- **The inner layer** is the `linux` trampoline running inside the sandbox, against the Sentry.

The Sentry implements user namespaces, seccomp, cgroups and `pivot_root`, but not necessarily Landlock **(verify)**. So the inner layer reports what the Sentry gave it (D1007), and the kit runs inside gVisor (§12.3). The same holds for the microVM: the guest kernel enforces the inner layer, and the VMM and `jailer` are the outer one.

## 6. The `SandboxSpec` (D990)

### 6.1 The shape

```rust
// crates/loams-sandbox/src/spec.rs (sketch; SB1 Task 1 fixes the names)
pub struct SandboxSpec {
    pub fs: FsSpec,
    pub net: NetSpec,
    pub syscalls: SyscallProfile,
    pub resources: Resources,
    pub identity: Identity,          // always: no capabilities, no_new_privs
    pub env: EnvSpec,                // cleared, then an explicit allowlist
    pub stdio: StdioSpec,            // Pipes | NewPty; never the caller's tty
    pub best_effort: BestEffort,     // fields an adapter may skip, reported (D990)
}

pub struct FsSpec {
    pub read_only: Vec<PathBuf>,     // bound read-only; code may be mapped from here
    pub read_write: Vec<PathBuf>,    // bound read-write (a worktree)
    pub scratch: Scratch,            // Dir(PathBuf) | Tmpfs { bytes } — private, the cwd by default
    pub mask: Vec<PathBuf>,          // inside a bound tree, hidden (an empty file or tmpfs)
    pub code: CodeSource,            // ReadOnlyPaths | MappedAtSeal (House: /proc/self/maps)
    pub cwd: Option<PathBuf>,
}

pub enum NetSpec {
    None,                            // a network namespace with lo only, nothing forwarded
    Forward(ForwardSpec),            // lo only, plus the forwarder (§9)
    HostPorts(Vec<u16>),             // no network namespace; Landlock TCP connect to these ports (House `pods`)
}

pub struct Resources {
    pub cpu_max: Option<CpuMax>,     // quota µs per period µs
    pub memory_max: Option<u64>,     // memory.swap.max = 0 when set
    pub pids_max: Option<u64>,
    pub io_max: Vec<IoMax>,          // (major:minor, rbps, wbps, riops, wiops)
    pub rlimits: Rlimits,            // core (always 0), nofile, as_, cpu, fsize
    pub wall_deadline: Option<Duration>,
    pub cpu_deadline: Option<Duration>,
}

pub struct Identity { pub uid_map: UidMap /* SelfOnly in SB1 */ }
```

### 6.2 The rules

- **What the spec is.** A spec is a value. It is serialisable (postcard on the wire to the trampoline, JSON for logs and tests) and validated before any adapter sees it.
- **Validation:**
  - absolute paths only;
  - no `..`;
  - `read_write` paths must not contain `read_only` ones, or the reverse, unless one is a `mask`;
  - the scratch directory exists or is created with mode 0700;
  - `HostPorts` is refused with `syscalls: Shell`, where the network is not contained.
- **Fail closed (D990).** An adapter whose `capabilities()` lacks a field the spec sets refuses the spawn with `SandboxError { step: "capabilities", … }`, unless the field is in `best_effort`. Skipped best-effort fields are listed in the `SandboxReport` (D1007). Two examples:
  - Landlock above ABI 4 is best-effort, as House does today;
  - `io_max` on a host without the `io` controller is not.
- **Named presets.** Presets build specs; they are not new fields:
  - `house_worker(tmp, ports)`;
  - `live_worker(memory, cpu)`;
  - `agent(level, cwd, worktree, home_policy)`;
  - `function_t0(tenant)`.

  Consumers call a preset, and the conformance kit runs each preset.

### 6.3 Syscall profiles (D997)

| Profile | Default action | Denies (beyond the default action) | Used by |
|---|---|---|---|
| `worker` | Allow | House's `denied()` list → `EPERM`; `clone` without `CLONE_THREAD`; the kill family to any pid but self; `tkill` unless Landlock scopes signals; `socket` outside `AF_INET`/`AF_INET6`. `ENOSYS`: `clone3`, io_uring | House workers (as built) |
| `interpreter` | `KILL_PROCESS` | Allows only Live's 13 calls, with `read`/`write` limited to fds 0–2 | Live workers (as built) |
| `shell` | Allow | The `worker` list **minus** `execve`/`execveat`/`fork`/`vfork`/`clone` (process creation is allowed). The kill family is refused outside the sandbox's PID namespace by the namespace itself, so the pid rule is dropped. **Plus:** `ioctl(TIOCSTI)` and `ioctl(TIOCLINUX)` → `EPERM`; `socket(AF_NETLINK)` after setup; `personality` changes other than the default; `ENOSYS` for `clone3`, which libc falls back from, and io_uring | Agent harness runs, workerd (T0), anything that runs a userland |
| `none` | — | Nothing (for `Unsandboxed`, which is not an adapter) | `--sandbox=none`, `danger-full-access` |

`clone3` returning `ENOSYS` works for `shell` because glibc and musl fall back to `clone`, which the filter can read. Go's runtime uses `clone` directly, and Node and Bun use libc **(verify per runtime in SB1 Task 6)**. `prctl(PR_SET_SECCOMP)` stays allowed: a nested, tighter filter is harmless.

### 6.4 Filesystem semantics

- **Under `linux`** the payload's root is a new tmpfs. Only `read_only`, `read_write` and `scratch` are bound into it, plus a minimal `/dev` (`null`, `zero`, `full`, `random`, `urandom`, `tty` and a fresh `pts`) and a new `/proc`. Anything not bound does not exist.
- **Landlock** repeats the same rules underneath, so a mount mistake is not an escape.
- **House keeps its `MappedAtSeal` mode** (`CodeSource::MappedAtSeal`): it has no mount namespace (R6.6), and it reads what it already mapped from `/proc/self/maps` at seal time.
- **`mask` hides credential files inside trees that must be bound**: `/etc/shadow`, `/etc/gshadow`, `/etc/sudoers`, `/etc/ssh/*_key`, and `~/.ssh`, `~/.aws`, `~/.config/gcloud`, `~/.kube`, `~/.docker/config.json` and `~/.netrc` when a home tree is bound. Masks are an empty read-only file or an empty tmpfs.

### 6.5 Identity (D1000)

- **The uid map.** The payload runs with its own uid and gid mapped onto themselves in a new user namespace, after `setgroups` is denied, which is House's `netns::enter`. Files it writes in `read_write` and scratch stay the host user's.
- **Never root.** Nothing runs as root inside. A process whose host uid is 0 refuses to spawn under `linux` unless the deployment passes a non-root target uid (Q886). Whether every Loams image runs as non-root today is not checked here: only `deploy/house/kind/worker-pods.yaml` sets `runAsNonRoot` for House, so SB1 Task 0 records the rest.
- **Capabilities.** Every capability set is cleared (effective, permitted, inheritable, ambient, and the bounding set where permitted).
- **`no_new_privs`** is always set, so a setuid binary inside gains nothing.

## 7. The `Sandbox` trait (D991)

### 7.1 Shape

```rust
// crates/loams-sandbox/src/lib.rs (sketch)
pub trait Sandbox: Send + Sync + fmt::Debug + 'static {
    fn kind(&self) -> AdapterKind;                       // Linux, Wasm, Gvisor, Microvm, Seatbelt, AppContainer
    fn capabilities(&self) -> &Capabilities;             // probed once at construction
    fn spawn(&self, spec: &SandboxSpec, cmd: SandboxCommand) -> Result<SandboxedChild, SandboxError>;
    fn kill(&self, child: &ChildRef) -> Result<(), SandboxError>;     // the whole tree
    fn stats(&self, child: &ChildRef) -> Result<SandboxStats, SandboxError>;
}

pub struct SandboxCommand { pub program: PathBuf, pub args: Vec<OsString>, pub env: Vec<(OsString, OsString)> }

pub struct SandboxedChild {               // owns the process (or VM, or instance); kills it on drop
    pub stdin: Option<ChildStdin>, pub stdout: Option<ChildStdout>, pub stderr: Option<ChildStderr>,
    pub report: SandboxReport,
    /* ChildRef, wait(), try_wait(), and a tokio variant behind the `tokio` feature */
}

pub struct SandboxStats {                 // plain numbers, ops-grade (D995)
    pub cpu_usage_usec: u64, pub cpu_user_usec: u64, pub cpu_system_usec: u64,   // cpu.stat
    pub nr_throttled: u64, pub throttled_usec: u64,
    pub memory_current: Option<u64>, pub memory_peak: Option<u64>,               // memory.current, memory.peak
    pub pids_current: Option<u64>, pub pids_peak: Option<u64>,
    pub io: Vec<IoStat>,                                                          // io.stat
    pub oom_kills: u64,                                                           // memory.events oom_kill
}

pub struct Capabilities {
    pub fs_read_only: bool, pub fs_read_write: bool, pub fs_scratch: bool, pub fs_mask: bool,
    pub net_none: bool, pub net_forward: bool, pub net_host_ports: bool,
    pub syscall_profiles: Vec<SyscallProfileName>,
    pub cpu_max: bool, pub memory_max: bool, pub pids_max: bool, pub io_max: bool,
    pub wall_deadline: bool, pub cpu_deadline: bool,
    pub landlock_abi: Option<u8>, pub user_namespaces: bool, pub cgroup_delegated: bool,
    pub snapshot: bool,                                   // microvm only
    pub not_applicable: Vec<(ProbeId, &'static str)>,     // kit probes this adapter cannot face, and why
}
```

- **`kill`** kills every process the sandbox holds:
  - `linux` writes `1` to `cgroup.kill`, or SIGKILLs the PID-namespace init when no cgroup is delegated, which takes the namespace down with it;
  - `gvisor` runs `runsc kill --all`;
  - `microvm` stops the VMM;
  - `seatbelt` kills the process group;
  - `appcontainer` uses `TerminateJobObject`.
- **`stats`** is cheap enough to poll at 1 Hz per sandbox (target). Where a field has no source (macOS), it is `None`, or it is filled from `wait4` rusage at exit.

### 7.2 `seal_self` (Linux only)

```rust
pub mod linux {
    /// Seals the calling process. It must have one thread. For processes that load their
    /// code before they are sealed (House, Live). No mount or PID namespace: the process
    /// keeps its view, and Landlock, seccomp, the user and network namespaces, the cgroup,
    /// rlimits and capabilities do the work, in House's order (§8.1.2).
    pub fn seal_self(spec: &SandboxSpec, inherited: Inherited) -> Result<SandboxReport, SandboxError>;
    /// The trampoline's entry; the host binary calls it first thing in `main` (D996).
    pub fn trampoline() -> Option<ExitCode>;
}
```

House and Live keep their process model (the front spawns the worker, and the worker seals itself after loading libchdb or QuickJS). They change only the code that seals. `seal_self` supports `CodeSource::MappedAtSeal` and refuses `FsSpec` fields that need a mount namespace (`read_write` outside scratch, `mask`).

### 7.3 The report (D1007)

`SandboxReport` is a serialisable value that every consumer logs at start. House already logs `landlock: full | partial (…)`. Its fields are:
- `adapter`;
- `namespaces` (user, mnt, pid, net, ipc, uts);
- `landlock: { abi, status }`;
- `seccomp: profile`;
- `cgroup: Option<{ path, controllers }>`;
- `rlimits`;
- `net: { mode, forward_target }`;
- `skipped: Vec<(field, reason)>`;
- for stacked adapters, `outer: Option<Box<SandboxReport>>`.

The report reaches:
- the House front's start line;
- Live's deployment status;
- agentd's thread header ("enforced by Loams: linux, workspace-write");
- the function node's status (§24 §10 "the fallback is reported in the node's status").

## 8. Adapters (D992)

### 8.1 `linux` (default)

#### 8.1.1 Two ways in

- **`spawn`** is for a fresh payload such as a harness CLI, workerd or a shell command. It uses the trampoline (D996).
- **`seal_self`** is for House and Live (§7.2).

#### 8.1.2 The order of steps

This order is House's documented order, extended for `spawn`. The House order is kept exactly for `seal_self`.

| # | Step | Stage | Why here |
|---|---|---|---|
| 1 | Refuse unless the process has one thread (`/proc/self/status`) | 1 | `unshare(CLONE_NEWUSER)` and Landlock bind the calling thread; a second, unsandboxed thread would be an escape (House, Live) |
| 2 | Join the cgroup child that the host made (`cgroup.procs` ← `0`) and check `/proc/self/cgroup` | 1 | Everything after it is counted (House `cgroup::join`) |
| 3 | `RLIMIT_CORE` 0 | 1 | A core dump holds the payload's data (House) |
| 4 | `unshare(CLONE_NEWUSER \| CLONE_NEWNS \| CLONE_NEWPID \| CLONE_NEWNET \| CLONE_NEWIPC \| CLONE_NEWUTS)`, `setgroups deny`, uid and gid map onto self | 1 | House does user and net only. The mount namespace enables `pivot_root`; the PID namespace hides the host's processes and `/proc` (R6.6's gap) |
| 5 | Spawn stage 2 (`<self> __loams-sandbox-init`); it is PID 1 of the new namespace | 1 → 2 | The PID namespace applies to children only. Stage 1 waits, relays the exit status, and forwards SIGTERM and SIGINT |
| 6 | Mount: `/` made `MS_PRIVATE` recursively; a tmpfs as the new root; bind each `read_only` path and remount it `MS_RDONLY \| MS_NOSUID \| MS_NODEV` (keeping locked flags); bind `read_write` and scratch `nosuid,nodev`; masks; a minimal `/dev`, a new `devpts`, a new `proc` (`hidepid=invisible`); `pivot_root`; unmount the old root | 2 | What bubblewrap does, in Rust. Only listed paths exist |
| 7 | `lo` up (rtnetlink, House `netns::raise_loopback`). For `Forward`, bind the listener(s) on `127.0.0.1:<p>` | 2 | Before seccomp, which refuses `AF_NETLINK` after setup |
| 8 | Spec rlimits (`nofile`, `as`, `cpu`, `fsize`), hard and soft equal | 2 | The payload cannot raise them again |
| 9 | Drop every capability (bounding, then effective, permitted, inheritable and ambient) | 2 | They belonged to the new user namespace only (House step 6) |
| 10 | Landlock: the filesystem rules of §6.4 (read for `read_only`; all for `read_write` and scratch; `/dev` nodes as listed), TCP rules (`HostPorts`: connect to the listed ports only; inside a network namespace, `bind` and `connect` are left to the namespace), and signal and abstract-socket scoping. ABI 4 required, rules written for the newest ABI the crate knows, best effort above 4 | 2 | House step 7 |
| 11 | `no_new_privs`, then seccomp for the profile, synchronised to every thread (`TSYNC`) | 2 | House step 8 |
| 12 | Start the forwarder thread (House step 9). Spawn the payload with the spec's env, cwd and stdio. Reap orphans as PID 1 and exit with the payload's status | 2 | The payload inherits steps 4 to 11 |

Steps 6 to 11 run after stage 2 has started its forwarder listener and before any payload code exists. A failure at any step exits **77** with `SandboxError { step, detail }` on the status pipe, as House's worker does. The payload never starts unsealed.

#### 8.1.3 The trampoline (D996)

- **Why not `pre_exec`.** `std::os::unix::process::CommandExt::pre_exec` is `unsafe`, and its closure runs in a forked child of a multi-threaded parent, where only async-signal-safe calls are allowed. Building mounts, Landlock rulesets and seccomp programs there is unsound.
- **The trampoline instead.** It is a fresh `execve` of the host's own binary, with:
  - argv: `__loams-sandbox-exec`;
  - the serialised spec on fd 3;
  - the forwarder socket on fd 4;
  - a status pipe on fd 5;
  - the payload's argv after `--`.

  It starts single-threaded, and everything is ordinary safe Rust.
- **Which binaries carry it.** Every binary that spawns sandboxed children calls `loams_sandbox::linux::trampoline()` first in `main`: `loams`, `loams-agentd`, the function supervisor, and `loams-house` for symmetry. The House worker itself uses `seal_self`. A unit test in each binary checks that the entry point is wired (`trampoline_entry_is_wired`).

#### 8.1.4 Cgroups

- **Delegation.** The host needs a delegated cgroup v2 subtree (one it may write, with the controllers enabled in `cgroup.subtree_control`, holding no processes itself). House's `WorkerCgroups::prepare` already checks exactly this. It moves into the crate as `cgroup::Delegated` and gains `io.max`.
- **Where the subtree comes from:**
  - **agentd** adds `Delegate=yes` (with `cpu memory pids io`) to its systemd user unit (§13.3). On a system without user delegation it runs without cgroups, and `capabilities().cgroup_delegated` is false.
  - **The House front** keeps `--cgroup-root`.
  - **Containers** use the cgroup namespace root the runtime gives them.
- **Cleanup.** The host removes the child after `wait`. A leftover child from a crash is removed at the next `Delegated::prepare` (only children named by this crate's prefix).

#### 8.1.5 Kernels and hosts

| Need | For | Where it is missing |
|---|---|---|
| Unprivileged user namespaces | All of `linux` | Ubuntu with `kernel.apparmor_restrict_unprivileged_userns=1` (on by default in 24.04 **(verify)**; the repository's CI turns it off); Debian kernels with `kernel.unprivileged_userns_clone=0`; containers whose seccomp profile refuses `CLONE_NEWUSER` (Docker's default allows it only with `CAP_SYS_ADMIN` **(verify)**; Kubernetes `RuntimeDefault` not measured, R1.11) |
| Landlock ABI ≥ 4 (TCP rules) | `HostPorts`; defence in depth everywhere | Kernels before ABI 4 (Linux 6.7 **(verify)**). R1.11 measured ABI 10 on the build machine |
| Landlock ABI ≥ 6 (signal scoping) | `worker` lets `tkill` through only with it | Before ABI 6 (Linux 6.12 **(verify)**); the `worker` profile refuses `tkill` there |
| A writable `/proc` mount in a new PID namespace | Step 6 | Containers that mask `/proc` (some Docker setups). The adapter then uses `MappedAtSeal`-style rules without the PID namespace and reports it, as a best-effort field the agent preset allows (Q885) |
| A delegated cgroup subtree | Resources | Hosts without systemd user delegation |

#### 8.1.6 Degradation (D1009)

Each consumer has a fixed policy for a host that falls short:
- **House** refuses to start sealed workers unless `--sandbox=none` (logged; R1.10, R6.1).
- **Live** refuses `isolated` mode.
- **The function supervisor** refuses multi-tenant tiers on bare `linux` (D993) and reports a degraded T0.
- **agentd** follows Q885.

No consumer runs a weaker sandbox silently.

### 8.2 `wasm`

- **What it runs.** The payload is a Wasm component. `spawn` instantiates it in a wasmtime engine inside the **host process**, which is itself sealed by `linux` with a `worker`-like profile.
- **What the spec becomes:**
  - `FsSpec` becomes WASI preopens: `read_only` dirs read-only, `read_write` and scratch read-write; nothing else exists.
  - `NetSpec::None` means no `wasi:sockets` and no `wasi:http`. `Forward` routes `wasi:http` outgoing requests through the same allowlisting proxy (§9).
  - `cpu_deadline` uses **fuel** (exact; §24 D175's T1 meter), and `wall_deadline` uses **epoch interruption**.
  - `memory_max` uses the instance's `StoreLimits`.
  - `pids_max` is not applicable.
- **Stats** come from fuel consumed, converted to µs at a calibrated rate **(estimate)**, plus wall time.
- **Pooling.** wasmtime's pooling allocator is used for many instances.
- **Later.** Hyperlight-Wasm is a later backend behind the same adapter (Q887). It is pre-1.0, and Hyperlight-Wasm calls itself experimental.

### 8.3 `gvisor`

- **The bundle.** `spawn` writes an OCI bundle under the host's runtime directory and runs `runsc --rootless` (or as root where the node runs gVisor as a system runtime) with `--network=none`, or with a sandbox-internal loopback for `Forward` **(verify the flags in SB1's gvisor task)**. The bundle's contents:
  - its `config.json` holds the spec's mounts, rlimits, cgroup limits (`linux.resources`: CPU quota and period, memory limit, pids limit, block I/O), an empty capability set, `noNewPrivileges` and the `shell` seccomp profile;
  - its process is the `linux` trampoline running the payload, which stacks the spec inside (§5.4).
- **The network.** `Forward` passes the forwarder socket into the sandbox as an inherited file descriptor (`--host-uds` and fd donation **(verify)**). The proxy is the same host-side proxy as for `linux`.
- **Stats.** `runsc` creates the sandbox's cgroup itself, or uses systemd's driver (`--systemd-cgroup`); `stats()` reads that cgroup.
- **Platform.** systrap is the default and needs no KVM.
- **Where gVisor is missing.** On Kubernetes the function pods use the `gvisor` `RuntimeClass` (MT2, §38). The adapter is for the node supervisor's own processes. On a node without `runsc`, `capabilities()` says so, and D1005's fallback applies.

### 8.4 `microvm`

- **VMMs.** Firecracker first (Q882), under its `jailer`, with Cloud Hypervisor second. Both need KVM.
- **What the spec becomes:**
  - `FsSpec` becomes the guest's block devices and virtiofs shares: a read-only rootfs image (an environment image, §15 §5.2), a read-write workspace disk, and scratch on the guest's tmpfs.
  - `NetSpec` becomes a tap device into a host-side namespace whose only route is the proxy (`Forward`), or no device (`None`).
  - `Resources` become the vCPU count and rate limiters (Firecracker's token buckets), the VM's memory size, and the VMM's cgroup.
- **Inside the guest**, a small init (`loams-workspace-agent`, D1006) runs the payload through the `linux` adapter again (§5.4).
- **The `Microvm` extension trait:**
  - `snapshot(child) -> SnapshotRef`: the memory file, the VM state and the disk diff;
  - `restore(spec, SnapshotRef) -> SandboxedChild`;
  - `fork(SnapshotRef, n) -> Vec<SandboxedChild>`: restore *n* copies with copy-on-write memory, which is Firecracker's documented restore.
- **Snapshot constraints.** A snapshot restores only on identical software and hardware, so a snapshot records its host class and the VMM version, and restore refuses a mismatch. Diff snapshots are in developer preview and are not used in SB1. Snapshot files go to the bucket's CAS with chunk dedup (§15 §8, "VM memory snapshots").
- **The boat model.** A workspace is a persistent VM that is stopped to a snapshot and forked from it. That gives an agent a machine that survives between sessions and forks in seconds (target).

### 8.5 `seatbelt` (macOS)

- **The profile.** `spawn` writes an SBPL profile and runs `/usr/bin/sandbox-exec -f <profile> -- <payload>`, as Codex does. No FFI is needed. Apple marks `sandbox-exec` deprecated, but Codex and srt rely on it **(verify the current status)**; see Q880.
- **What the profile allows:**
  - **Files:** `(deny default)`, `file-read*` for `read_only` subpaths and the system's frameworks, and `file-read*`/`file-write*` for `read_write` and scratch. Masks are `deny` rules after the allows.
  - **Network:** `network-outbound` only to the proxy's localhost port for `Forward`, and none for `None`.
  - **Processes:** `process-exec` and `process-fork` for `shell`.
- **Resources.** `setrlimit` for `nofile`, `as`, `cpu` and `core`. The wall deadline is a host timer. No cgroups, so `cpu_max`, `memory_max`, `pids_max` and `io_max` are not applicable, and the agent preset marks them best-effort on macOS.
- **Stats** come from `proc_pid_rusage` while the payload runs, or `wait4` at exit.
- **`kill`** signals the process group (`owned.rs` already makes one).

### 8.6 `appcontainer` (Windows)

- **The container.** `spawn` creates (or reuses) an AppContainer profile for the run. It grants that SID access to the `read_only`, `read_write` and scratch paths, and launches the payload with `SECURITY_CAPABILITIES` in the proc-thread attribute list that `windows_process.rs` already builds for the job object.
- **Resources.** The job object gets `JOB_OBJECT_LIMIT_PROCESS_MEMORY`/`JOB_MEMORY`, `ACTIVE_PROCESS`, the CPU-rate control and kill-on-close.
- **The network.** With no `internetClient` capability the container has no network. Reaching a loopback proxy from an AppContainer needs a loopback exemption, or a broker that the container can reach **(verify)**.
- **ACLs.** Granting the container's SID ACEs on the person's files changes their ACLs. The grants are removed when the run ends, but a crash leaves them behind.
- **The alternative.** Codex chose a **restricted token** with dedicated local sandbox accounts and DACLs instead, plus job objects. Neither of Codex's descriptions mentions AppContainer.

The owner named AppContainer, so it stays the adapter's name. Q884 asks whether the mechanism under it should be AppContainer or a restricted token. SB1 Task 0 measures both on a Windows runner.

## 9. The network (D998)

### 9.1 Modes

| Mode | Inside | Outside |
|---|---|---|
| `None` | A network namespace with `lo` only, up | Nothing |
| `Forward` | `lo` plus the forwarder listener(s) on `127.0.0.1:<p>`. For agents, `HTTP_PROXY`, `HTTPS_PROXY`, `ALL_PROXY` (and lowercase) point at them, and `NO_PROXY=localhost,127.0.0.1` | A `ForwardTarget` on the host |
| `HostPorts(ports)` | The host's network namespace; Landlock TCP `connect` to `ports` only, no `bind` | The pod's NetworkPolicy (House `pods`) |

### 9.2 The forwarder, generalised

House's forwarder works like this:
- the sandbox's thread accepts on `127.0.0.1:<p>` inside the namespace;
- for each connection it sends one request byte over the inherited Unix socket (fd 4);
- the host answers with one end of a fresh socket pair (`SCM_RIGHTS`) and serves the other end.

This moves into the crate unchanged as `net::forwarder` (the in-sandbox half) and `net::serve_forwarder` (the host half). The host-side target is a trait:

```rust
pub trait ForwardTarget: Send + Sync + fmt::Debug + 'static {
    /// One forwarded connection from sandbox `who`; `listener` is the in-sandbox port it came on.
    fn serve(&self, who: &SandboxId, listener: u16, stream: UnixStream);
}
```

House's `Forward::Relay` and `Forward::Target` (`house-cache`) become two implementations. The per-sandbox connection cap (`MAX_FORWARDED_CONNECTIONS` = 64) moves with them.

### 9.3 The allowlisting proxy

`net::proxy` is a new `ForwardTarget` that speaks HTTP `CONNECT`, plain HTTP proxying and SOCKS5 (with domain names; `socks5h` semantics) on the forwarded stream. It works in five steps:

1. **Parse** the requested `host:port`.
2. **Match** it against the allowlist. Entries are `host`, `*.domain`, `host:port`, or a CIDR for explicitly allowed addresses.
3. **Resolve on the host**, then refuse if any resolved address is loopback, link-local (including `169.254.169.254` and `fd00:ec2::254`), private (RFC 1918, ULA), CGNAT or multicast, unless the entry names that address or CIDR. This also defeats DNS rebinding, because the check is on the address actually connected.
4. **Connect** and splice bytes. There is no TLS interception. The name checked is the `CONNECT` target, so it can differ from the TLS SNI the client then sends **(estimate: accepted, as srt does)**.
5. **Emit an audit event** (`sandbox`, `host`, `port`, `allowed`, `bytes`) to the log, and for agentd to the thread's event stream.

Inside a `Forward` sandbox there is no resolver and no route. A program that ignores the proxy variables fails to connect instead of leaking, and that is the intended failure.

### 9.4 Reverse forwards (`expose`)

- **What it is for.** Studio's preview (§53) needs a dev server the agent starts to be reachable from the host. `ForwardSpec.expose: Vec<u16>` makes the host listen on an ephemeral loopback port and forward each connection into the sandbox's `127.0.0.1:<port>`, through the same fd channel in reverse.
- **Binding.** Nothing inside can bind a host port.

### 9.5 The agentd shim

agentd injects its MCP shim, which dials agentd's loopback port with a per-run token (D788). That port is added to the run's allowlist as an explicit `127.0.0.1:<port>` entry. It is reached through the forwarder like any other destination, so the in-sandbox shim needs no direct host route.

## 10. Resources and deadlines (D999)

| Field | `linux` | `gvisor` | `microvm` | `wasm` | `seatbelt` | `appcontainer` |
|---|---|---|---|---|---|---|
| `cpu_max` | `cpu.max` | OCI CPU quota | vCPU rate limiter + VMM cgroup | fuel rate | — | job CPU rate |
| `memory_max` | `memory.max`, `memory.swap.max` 0 | OCI memory limit | VM memory | `StoreLimits` | — (`RLIMIT_AS` only) | job memory |
| `pids_max` | `pids.max` | OCI pids limit | guest cgroup | n/a | — | job active processes |
| `io_max` | `io.max` (needs the `io` controller and the backing device's major:minor) | OCI block I/O | drive rate limiters | n/a | — | — |
| `rlimits` | `setrlimit` | OCI `rlimits` | guest | n/a | `setrlimit` | — |
| `wall_deadline` | host timer → `kill()` | same | same | epoch | host timer | host timer |
| `cpu_deadline` | `cpu.stat` watchdog → `kill()`; `RLIMIT_CPU` as backstop | cgroup watchdog | VMM cgroup watchdog | fuel | `RLIMIT_CPU` | job time limit |

- **Scratch size.** Disk space is bounded by making scratch a `Tmpfs { bytes }`, counted in `memory.max`, or by the host's quota on a scratch directory. `io.max` bounds rate, not size.
- **The CPU watchdog** polls `cpu.stat usage_usec` at 10 Hz (target) and kills at the deadline. This is §24 §10's "a supervisor watchdog on the cgroup delta for T0 and T2", made one piece of code.
- **What `stats()` is.** It is the open, plain source of the numbers that §27's `InvocationObserver` reports (D995). It is not a meter.

## 11. The open-core line (D995)

- **What this repository has.** The spec, every limit and its enforcement, `stats()`, the report, the cgroup layout and labels. These are mechanism, useful to any self-hoster (open-core §3, the "Quotas" and "Observability" rows).
- **What `loams-platform` has.** Billing-grade usage: signed records, delivery and acknowledgement, final reads, reconciliation. The platform may read `stats()` and the cgroup layout as hints, as it reads any open extension point. Nothing in `loams-sandbox` depends on it, names a meter record, or writes a usage stream.

## 12. The conformance kit (D1001)

### 12.1 Shape

- **`crates/loams-sandbox-conformance`** is a library plus test harness. For each adapter available on the machine and each preset, it spawns `loams-sandbox-probe <probe>` under the preset's spec and checks the outcome.
- **The probe binary** is static where possible and makes the system call directly. It prints one JSON line: `{"probe":…,"outcome":"denied|allowed|killed","errno":…}`.
- **Expected outcomes** are a table per preset. A probe the adapter cannot face (`io_max` on macOS) must appear in `capabilities().not_applicable` with a reason, or the kit fails.

### 12.2 Probes

The names are the test names, `probe_<name>`.

| Probe | Does | Expected under `worker` / `shell` (agent `workspace-write`) / `interpreter` |
|---|---|---|
| `read_etc_shadow` | `open("/etc/shadow")` | denied / denied (masked) / killed |
| `read_home_ssh_key` | read a planted `~/.ssh/id_ed25519` outside the binds | denied / denied / killed |
| `read_other_proc_environ` | read `/proc/<host pid>/environ` of a planted process | denied / denied (not visible) / killed |
| `write_outside_scratch` | create a file in a host directory not bound | denied / denied / killed |
| `write_read_only_bind` | write under a `read_only` path | denied / denied / killed |
| `symlink_escape` | from `read_write`, follow a planted symlink to a host file | denied / denied / killed |
| `hardlink_escape` | `link()` a host file into scratch | denied / denied / killed |
| `execve` | exec `/bin/true` | denied / **allowed** / killed |
| `fork` | `fork()`, `clone` without `CLONE_THREAD` | denied / **allowed** / killed |
| `fork_bomb` | fork until failure | stopped at `pids.max` / stopped at `pids.max`; the host stays responsive / killed |
| `memory_balloon` | allocate past `memory_max` | OOM-killed, counted in `oom_kills` |
| `cpu_spin` | spin past `cpu_deadline` | killed by the watchdog, `ExitReport::CpuDeadline` |
| `wall_sleep` | sleep past `wall_deadline` | killed, `ExitReport::WallDeadline` |
| `disk_fill` | write until failure in a tmpfs scratch | stopped at its size |
| `connect_host_port` | TCP connect to a planted listener on the host | denied / denied / killed |
| `connect_metadata` | connect `169.254.169.254:80` directly and through the proxy | denied / denied both ways / killed |
| `connect_lan` | connect to an RFC 1918 address through the proxy | — / denied / — |
| `connect_allowed_via_proxy` | `CONNECT` an allowlisted planted host | — / **allowed** / — |
| `connect_denied_via_proxy` | `CONNECT` a host not on the list | — / denied, audited / — |
| `dns_direct` | resolve a name without the proxy | no resolver / no resolver / killed |
| `unix_socket_host` | connect a pathname socket on the host (a planted `/run/…sock`) | denied / denied / killed |
| `abstract_socket_host` | connect an abstract socket a host process listens on | denied (ABI 6) / denied (net namespace) / killed |
| `ptrace_parent` | `ptrace(PTRACE_ATTACH)` the host process | denied / denied / killed |
| `process_vm_readv` | read another process's memory | denied / denied / killed |
| `kill_outside` | `kill(-1, SIGKILL)`, `kill(<host pid>)` | denied / no effect outside / killed |
| `unshare_and_mount` | `unshare(CLONE_NEWUSER)`, `mount` | denied / denied / killed |
| `kernel_surfaces` | `bpf`, `perf_event_open`, `keyctl`, `init_module`, `userfaultfd` | denied / denied / killed |
| `io_uring` | `io_uring_setup` | `ENOSYS` / `ENOSYS` / killed |
| `tiocsti` | `ioctl(0, TIOCSTI, …)` on its stdin | n/a (pipes) / denied / killed |
| `setuid_binary` | run a planted setuid binary, check `geteuid` | n/a / no privilege gained / killed |
| `capabilities_and_nnp` | read `/proc/self/status` `CapEff`, `NoNewPrivs`, `Seccomp` | 0, 1, 2 |
| `other_tenant_cgroup` | write its pid to a sibling sandbox's `cgroup.procs`, read its `memory.current` | denied / denied / killed |
| `other_tenant_scratch` | read a sibling sandbox's scratch | denied / denied / killed |
| `escape_report_matches` | the `SandboxReport` names exactly what the probes found | report = outcome |

House's `sandbox_blocks_with_deny_list_off` and Live's three tests stay where they are. They test the products (chDB statements, QuickJS probes) and run unchanged after the move. The kit tests the crate.

### 12.3 Where the kit runs

| Adapter | Runner | Tier |
|---|---|---|
| `linux` | `ubuntu-24.04`, after `sysctl kernel.apparmor_restrict_unprivileged_userns=0` (as `fabric.yml` does) | Pull requests touching `crates/loams-sandbox*`, the House sandbox or Live's sandbox; part of `CI required` once green for two weeks (Q889) |
| `linux` inside a rootless podman container | same | nightly (R1.11 measured that it works) |
| `gvisor` | `ubuntu-24.04` with a pinned `runsc` | nightly, then pull requests touching the adapter |
| `microvm` | a KVM-capable runner (the heavy runner, or nested virtualisation) | nightly |
| `seatbelt` | `macos-latest` | pull requests touching the adapter |
| `appcontainer` | `windows-2025` (the repository's `windows-check` runner) | pull requests touching the adapter |
| `wasm` | `ubuntu-24.04` | with `linux` |

## 13. Consumers, and how each one moves

### 13.1 House (build step 1, D1002)

- **The code moves.** `loams-house-worker/src/sandbox/` and `forwarder.rs`, and the forwarder and cgroup halves of `loams-house/src/sandbox.rs`, move into `loams-sandbox::linux`, `::net` and `::cgroup`.
- **What stays in House:** the modes, the flags, `--cgroup`, `FORWARDER_PORT`, the warning text and exit 77.
- **The modes become specs:**
  - `netns` = `house_worker(tmp, Forward(house-cache))`;
  - `pods` = `house_worker(tmp, HostPorts([s3]))`;
  - `none` = `Unsandboxed`.
- **The worker's `main`** calls `linux::seal_self`.
- **Capabilities.** `loams_chdb_sys::capabilities::drop_all` is replaced by the crate's drop, using rustix's capability calls **(verify their coverage of the ambient and bounding sets in SB1 Task 0)**. If rustix lacks the ambient clear, it stays a one-function `unsafe` in `loams-sandbox-sys`.
- **The test of the move.** Every House test named in §3.1 passes unchanged on the moved code. The worker's Landlock rule list and seccomp program are byte-identical before and after: `house_worker_preset_matches_legacy_rules` compares the old `Rules::new` and `filters()` output, kept as golden files.

### 13.2 Live (build step 2, D1003)

- **The code moves.** `sandbox_linux.rs`'s `apply` becomes `linux::seal_self(&live_worker(memory, cpu), …)` with the `interpreter` profile.
- **Unchanged:** Landlock with no rights, `RLIMIT_NOFILE` 3 and `RLIMIT_AS`.
- **Added:**
  - `RLIMIT_CPU` at the deployment's CPU limit (D682's 1 s by default) + 1 s (target), whose hard limit kills;
  - the worker's own cgroup child where the host has a delegated subtree (`[live] cgroup_root`).
- **§45 §3.1's text** is amended to match (§16). The three Live tests pass unchanged.
- **Cloud.** Multi-tenant Live in Loams cloud runs its workers inside `gvisor` (D993, Q628).

### 13.3 agentd and Studio threads (build step 4, D1004)

**Where.** A new seam in `loams-agentd-harness`, `process::sandboxed(command, &RunSandbox) -> Command`, wraps each harness spawn site listed in §3.3. On Linux it rewrites the command to `loams-agentd __loams-sandbox-exec … -- <harness> <args>`. On macOS it becomes `sandbox-exec … -- <harness>`. On Windows the launch goes through `appcontainer`. The harness drivers do not change otherwise.

**The levels become specs.** The `agent` preset; paths are illustrative:

| | `read-only` | `workspace-write` | `danger-full-access` |
|---|---|---|---|
| `read_only` | the system (`/usr`, `/bin`, `/lib*`, `/etc` with masks), the project or worktree, toolchains (`~/.cargo/bin`, `~/.rustup`, `~/.nvm`, `~/.bun`, `~/.local/bin`; configurable) | the system, toolchains, the main checkout when the run is in a worktree | — |
| `read_write` | the harness's own state directory (`~/.claude`, `~/.codex`, `~/.config/opencode`, …), which it needs for sessions and auth | the worktree (or the `cwd`), the harness's state directory, package caches (`~/.cache`, `~/.npm`, `~/.cargo/registry`; configurable) | — |
| `scratch` | a private `/tmp` (tmpfs) | a private `/tmp` (tmpfs) | — |
| `mask` | §6.4's credential files; the rest of `$HOME` does not exist | same | — |
| `net` | `Forward`: the model provider's API hosts and agentd's shim port | `Forward`: those, plus the package registries (or Loams's registry proxy, §15 §6) and the project's allowlist (Q883) | the host's network |
| `syscalls` | `shell` | `shell` | none |
| `resources` | a cgroup with `pids.max` 4096, `memory.max` = the configured per-thread cap (target 8 GiB) | same | none |
| enforced by | `linux` / `seatbelt` / `appcontainer` | same | **nobody: labelled "not sandboxed"** |

- **The harness's own sandbox.** When Loams enforces, the harness runs with its own sandbox in full-access mode: Codex `danger-full-access`, Claude `bypassPermissions` for file tools. Codex's bubblewrap inside a Loams user namespace would nest namespaces that `shell` refuses, and two sandboxes with different rules confuse people. Approvals (D911) are unchanged: supervised mode still asks before each tool call. The Loams sandbox bounds what an approved or auto-approved call can reach.
- **The harness's credentials are inside the sandbox.** The sandbox wraps the whole harness process tree, so the harness's own credentials (its API key or OAuth token in its state directory) are readable by the commands it runs. That is no worse than today, and the sandbox still hides every *other* credential. Per-tool-call sandboxing would need the harness's cooperation, so it is a later step (§18).
- **When the adapter is unavailable** (no user namespaces, an old macOS), Q885 decides. Its default is to run with the harness's own sandbox at the thread's level, with the header saying "not enforced by Loams: <reason>".
- **The remote agentd** (§53, D921) uses the same `linux` adapter. §53 §8.2's advice to run it in a container or VM still stands for class C.
- **Delegation.** agentd's systemd unit gains `Delegate=cpu memory pids io` (DD1's `render_unit`). On macOS and Windows there is nothing to delegate.

### 13.4 Functions (build step 5, D1005)

| Tier | §24 today | With §56 |
|---|---|---|
| T0 (workerd, one process per tenant) | "gVisor `runsc` around the process, or seccomp-bpf plus a user namespace and cgroup where gVisor is not available" | `gvisor(function_t0(tenant))`, with the linux spec stacked inside; where `runsc` is absent, `linux(function_t0)` with the `shell` profile and `Forward` to the tenant's egress allowlist (D141's rule), reported in the node's status. Multi-tenant clusters refuse the fallback (D993) |
| T1 (wasmtime) | Wasm plus the host's process and cgroup | `wasm` inside a `linux`-sealed host |
| T2 (`http-port` in `runsc`) | gVisor | `gvisor`; on Knative, the `gvisor` `RuntimeClass` (MT2) |
| T3 (Firecracker snapshots) | Deferred (D174) | Still deferred; when it comes, it is `microvm` |

The function supervisor (F1, unbuilt) uses `Sandbox::stats()` for §24 §10's CPU watchdog and §27's observer.

### 13.5 Workspaces (build step 6, D1006)

- **§15 §8's backends.** The `firecracker` and `gvisor` backends are the `microvm` and `gvisor` adapters. The `process` backend is `linux`, no longer "trusted local development only", because it is now the escape-tested adapter for class B.
- **microsandbox.** §15's default backend is not on the owner's ladder. Q881 asks whether it returns as a desktop `microvm` backend (it runs on Apple HVF and Windows WHP, where Firecracker does not).
- **The in-sandbox agent.** §15's "small agent (sidecar or in-VM binary)" that mounts layers, points package managers at the proxy and implements `checkpoint`, `fork`, `suspend` and `resume` is renamed **`loams-workspace-agent`**, since the crate name now means the isolation layer. Its `fork` and `suspend` call the `Microvm` extension (§8.4).

## 14. Performance targets

| What | Target | Note |
|---|---|---|
| `linux` spawn overhead (trampoline, namespaces, mounts, Landlock, seccomp) | ≤ 15 ms p95 **(target)** | R1.11 measured House's netns plus users file at +8–11 ms p95 over an unsealed worker; Sandlock reports about 5 ms |
| `seal_self` | no regression against today's House and Live boots | Measured before and after in SB1 Tasks 2 and 3 |
| Proxy added latency per connection | ≤ 2 ms p95 on loopback **(target)** | One fd pass and a host connect |
| `stats()` | ≤ 100 µs **(target)** | A few small cgroup files |
| `microvm` fork of a warm snapshot | ≤ 1 s for 1 GiB guest memory **(target)** | Firecracker restore with copy-on-write memory |

## 15. The build order (D994) and the plan

| Step | What | Plan |
|---|---|---|
| 1 | Extract House's L3 into `loams-sandbox::linux` | SB1a (Tasks 1–2) |
| 2 | Move Live onto it | SB1a (Task 3) |
| 3 | The conformance kit | SB1b (Tasks 4–5) |
| — | `spawn`: the trampoline, mounts, PID namespace, proxy | SB1c (Tasks 6–8) |
| 4 | agentd sessions enforce `SandboxLevel` (with `seatbelt` and `appcontainer`) | SB1d (Tasks 9–13) |
| 5 | `gvisor` for functions T0 and T2 (and `wasm` for T1) | SB1e (Tasks 14–15); the supervisor itself is F1's |
| 6 | `microvm` with snapshot forking | SB1f (Tasks 16–17), with §15 W2 |

**SB1 starts after v0.1.0-preview.3** (the owner, 2026-10-15). Nothing in preview.3 changes because of this document.

## 16. Reconciliation with earlier designs

| Earlier | What it says | Here | Resolution |
|---|---|---|---|
| §24 §4.1 T0, §10 (D171) | T0 isolation: "gVisor `runsc` around the process, or seccomp-bpf plus a user namespace and cgroup where gVisor is not available"; "the fallback is reported in the node's status" | D1005, D993 | **Amended**: the fallback is the `linux` adapter (namespaces, cgroup, Landlock, seccomp `shell`), reported through `SandboxReport`; multi-tenant clusters refuse it. T1, T2 and T3 unchanged in substance |
| §24 §10 | "An egress allowlist per tenant is enforced by the supervisor's network namespace (the same rule as D141's outbound allowlist)" | D998 | Kept; the mechanism is `Forward` plus the proxy |
| §24 D175 | Fuel or epochs for T1, cgroup `cpu.stat` for gVisor | D995, §8.2, §10 | Kept; `stats()` is the open source of those numbers |
| §49 §13.2 L3 (D774), HS1 R6.1 | The order and rules of L3 | D1002 | Kept exactly; the code moves. **Text amended**: "a distinct UID; rlimits" reads "the worker's own uid mapped in a user namespace (a distinct uid comes from the deployment's `runAsUser`, R6.6); `RLIMIT_CORE` 0", matching the code |
| HS1 R6.6 | No PID namespace, no mount namespace for House | §7.2 | Kept for `seal_self`; `spawn` adds both for new consumers |
| §45 §3.1 (D681) | `isolated`: seccomp-bpf, Landlock, `RLIMIT_AS`, `RLIMIT_CPU`, `PR_SET_NO_NEW_PRIVS`, "a separate uid where the operator configures one" | D1003, D1000 | **Amended**: `RLIMIT_CPU` is added to the code (it was missing); the separate uid is the deployment's (as for House); multi-tenant cloud Live runs inside gVisor |
| Q628 | Is seccomp plus Landlock enough for multi-tenant Loams cloud Live? | D993 | **Answered by the owner's threat model**: no; stack inside gVisor or a microVM |
| §15 §8 (D24) | `loams-sandbox` is a crate with a `Runtime` trait and four backends (microsandbox default, Firecracker, gVisor, process for trusted development only); also "a small agent … that runtimes call with a spec" | D990, D1006 | **Amended**: the crate name now means the isolation crate of this document; the `Runtime` trait becomes `Sandbox` plus the `Microvm` extension; the in-sandbox agent is `loams-workspace-agent`; microsandbox is not the default (Q881); `process` becomes `linux`, escape-tested and fit for class B |
| §15 preamble | "Loams … never executes agent code in its own processes" | §13.3 | Holds for the cloud state plane. The desktop daemon (§50) does run harnesses as the person's processes; §56 confines them |
| §11 buy-vs-build, §16 fleet demo | `loams-sandbox` backends: microsandbox default | D1006 | Follow §15's amendment; the rows are updated when SB1f lands |
| §50 §7 / §53 §8.2 | Worktrees do not contain processes; "OS-level confinement inside the daemon … is Q815" | D1004 | **Q815 answered**: Loams enforces `SandboxLevel` with the platform adapter |
| §53 §8.1 (D911) | Codex forced to `danger-full-access` unless supervised; other drivers auto-approve | D1004 | Kept for approvals; with Loams enforcing, "full access" is inside the Loams sandbox |
| Open-core boundary (§41) | Limits and enforcement open; metering private | D995 | Kept |

## 17. Where the code lives (D1008)

```
crates/loams-sandbox/                 root workspace; unsafe_code = "forbid"
  src/lib.rs        Sandbox, SandboxedChild, Capabilities, SandboxStats, SandboxError, AdapterKind
  src/spec.rs       SandboxSpec and its parts; validate(); presets in src/presets.rs
  src/report.rs     SandboxReport
  src/linux/{mod.rs, trampoline.rs, seal.rs, mounts.rs, ns.rs, landlock.rs, seccomp.rs, caps.rs, rlimits.rs}
  src/cgroup.rs     Delegated, Child, limits, stats, kill
  src/net/{mod.rs, forwarder.rs, serve.rs, proxy.rs, allowlist.rs}
  src/wasm.rs       (feature `wasm`)
  src/gvisor.rs     (feature `gvisor`; writes OCI bundles, drives runsc)
  src/microvm/{mod.rs, firecracker.rs, snapshot.rs}   (feature `microvm`)
  src/seatbelt.rs   (macOS)
  src/appcontainer.rs (Windows; FFI through loams-sandbox-sys)
crates/loams-sandbox-sys/             macOS and Windows FFI only; unsafe_code = "deny", SAFETY comments
crates/loams-sandbox-conformance/     the kit; bin loams-sandbox-probe
```

- **Dependencies:** `landlock` 0.4.7, `seccompiler` 0.5.0 and `rustix` 1 (`process`, `thread`, `mount`, `net`, `fs`), all already pinned in the root workspace. The fabric workspace adds `rustix` to its lockfile through the path dependency. `wasmtime`, the gVisor and Firecracker client code, and `windows-sys` are behind features, and each new pin follows the 14-day rule.
- **Netlink.** House's rtnetlink code uses `nix` sockets. The crate uses `rustix::net` with `AF_NETLINK` **(verify that rustix's netlink address type supports the bind; else keep `nix` behind the `linux` module)**.
- **The fabric workspace** depends on `loams-sandbox = { path = "../../../crates/loams-sandbox" }`, the precedent of `loams-store` and `loams-cache` there. A shared-target hash collision between worktrees (HS1 R-ACL.1's note) is handled by the usual practice.

## 18. Risks

| Risk | Likelihood | Mitigation |
|---|---|---|
| Loams's own mount-namespace code has a bug that bubblewrap fixed years ago | Medium | Landlock repeats the filesystem rules underneath (§6.4); the kit's `symlink_escape`, `hardlink_escape` and `write_read_only_bind`; an external review before class B exposure (SB1 Task 18); compare with bubblewrap's mount steps line by line in review |
| Ubuntu's AppArmor userns restriction, Kubernetes `RuntimeDefault` or a container's seccomp blocks user namespaces, so `linux` is unavailable where people run Loams | High on Ubuntu 24.04 | Detect at start and report (D1009); document the sysctl or an AppArmor profile for `loams` and `loams-agentd` (SB1 Task 19); House keeps `pods`; agentd follows Q885 |
| The `shell` profile breaks a real toolchain (Node, Bun, Go, Python, cargo) | Medium | Task 6 runs each harness's own test suite and `npm install`, `cargo build`, `go build`, `uv sync` under the agent preset; the profile denies only the escape list |
| Proxy-unaware tools fail under `workspace-write` | Medium | A clear error in the thread ("network goes through the Loams proxy"); the allowlist UI; `danger-full-access` stays available, labelled |
| The harness's own credentials are inside the sandbox | Certain (by design) | Recorded (§13.3); every other credential is masked; per-tool-call sandboxing needs harness cooperation and is later work |
| macOS removes `sandbox-exec` | Low near term | Q880; the `sandbox_init` C API through `loams-sandbox-sys` is the fallback |
| AppContainer cannot reach a loopback proxy | Medium | Q884; the restricted-token approach Codex uses |
| gVisor does not implement Landlock, so the stacked inner layer is weaker than on a host | Medium | The report says so (D1007); the outer layer is the boundary for class C; the kit runs inside gVisor |
| Firecracker snapshot restore needs identical hardware | Certain | Snapshots carry their host class; restore refuses a mismatch; cold boot from the workspace disk is the fallback |
| Extraction changes House's behaviour | Low | Golden rules and filters (`house_worker_preset_matches_legacy_rules`); every House sandbox test unchanged |

## 19. Open questions (Q880–Q894)

| # | Question | Default |
|---|---|---|
| Q880 | `seatbelt` uses `/usr/bin/sandbox-exec`, which Apple marks deprecated. Accept it as Codex and srt do? | Yes; fall back to `sandbox_init` through `loams-sandbox-sys` if it is removed |
| Q881 | microsandbox (§15's default): keep it as a desktop `microvm` backend (Apple HVF, Windows WHP), or drop it? | Drop from SB1; re-evaluate when desktop workspaces are planned |
| Q882 | Firecracker or Cloud Hypervisor first for `microvm`? | Firecracker (E2B's precedent, copy-on-write restore); Cloud Hypervisor second |
| Q883 | `workspace-write`'s network: an allowlist (model APIs, package registries, the project's list) or open, as Codex's `networkAccess: true` is today? | Allowlist, editable per project, with a one-click "allow this host" from the audit event |
| Q884 | Windows: AppContainer (the owner's name), or a restricted token with sandbox accounts and DACLs (Codex's approach), under the `appcontainer` adapter? | Measure both in SB1 Task 0; AppContainer if a loopback proxy is reachable, else a restricted token |
| Q885 | When the platform adapter is unavailable, does agentd refuse `read-only` and `workspace-write` runs, or run them under the harness's own sandbox, labelled "not enforced by Loams"? | Run them labelled; refuse only when the person turns on "require Loams sandbox" |
| Q886 | Distinct host uids per sandbox (`newuidmap` and subordinate ranges) when a privileged helper is available? | No in SB1; the deployment's service user is the distinct uid |
| Q887 | Hyperlight-Wasm as a `wasm` backend? | After Hyperlight 1.0 |
| Q888 | Which outer adapter per product in Loams cloud? | Functions T0 and T2, Live and hosted House workers: `gvisor`; workspaces: `microvm` |
| Q889 | The `linux` kit in `CI required`? | Yes, after two weeks green on pull requests that touch sandbox paths; `gvisor` and `microvm` nightly |
| Q890 | Name of §15's in-sandbox agent, now that `loams-sandbox` is the isolation crate | `loams-workspace-agent` |
| Q891 | Keep House's `pods` mode as `HostPorts`, or retire it once user namespaces are measured on the target clusters? | Keep it as `HostPorts` |
| Q892 | Per-thread memory cap for agent runs | 8 GiB, configurable in agentd's `config.toml` |
| Q893 | Sandbox Studio terminals? | No; they are the person's own shell |
| Q894 | Should `stats()` feed §27's `InvocationObserver` directly, or only through the function supervisor? | Only through the supervisor (the crate stays independent of §27) |

## 20. Sources

Read on 2026-10-15 unless noted.

- Landscape surveys:
  - [Modal: best sandbox for OpenHands](https://modal.com/resources/best-sandbox-openhands) (May 2026, vendor);
  - [amux.io: AI agent sandboxing](https://amux.io/guides/ai-agent-sandboxing/) (August 2026);
  - [PandaStack: best self-hosted code sandboxes 2026](https://www.pandastack.ai/blog/best-self-hosted-code-sandboxes-2026/) (2026-06-28, vendor);
  - [Reinvently: microVM sandbox options](https://reinvently.co.uk/blog/microvm-sandbox-options-firecracker-opensandbox-smolvm-nono/) (2026-07-01, updated 2026-09-19);
  - [restyler/awesome-sandbox](https://github.com/restyler/awesome-sandbox).
- Codex:
  - [codex-rs/linux-sandbox README](https://github.com/openai/codex/blob/main/codex-rs/linux-sandbox/README.md) (primary);
  - [Codex CLI sandbox internals](https://codex.danielvaughan.com/2026/05/03/codex-cli-sandbox-internals-seatbelt-bubblewrap-landlock-windows-dacl/) (updated 2026-10-14);
  - [Codex sandboxing (Mintlify)](https://www.mintlify.com/openai/codex/concepts/sandboxing) (older; lists bubblewrap as experimental).
- Anthropic sandbox-runtime:
  - [anthropic-experimental/sandbox-runtime](https://github.com/anthropic-experimental/sandbox-runtime) (Apache-2.0);
  - [Bill Mill's notes](https://notes.billmill.org/AI/sandboxes/sandbox-runtime.html);
  - [Ry Walker: local agent sandboxes](https://rywalker.com/research/local-agent-sandboxes);
  - [How Claude Code and Codex sandbox untrusted code](https://medium.com/@Koukyosyumei/how-claude-code-and-codex-sandbox-untrusted-code-ba39b493046a) **(verify: returned 403)**.
- gVisor:
  - [platforms](https://gvisor.dev/docs/user_guide/platforms/) (systrap is the default);
  - [systemd and cgroups](https://gvisor.dev/docs/user_guide/systemd/).
- Firecracker and Cloud Hypervisor:
  - [Firecracker snapshot support](https://github.com/firecracker-microvm/firecracker/blob/main/docs/snapshotting/snapshot-support.md);
  - [e2b-dev/infra](https://github.com/e2b-dev/infra);
  - [Cloud Hypervisor snapshot and restore](https://github.com/cloud-hypervisor/cloud-hypervisor/blob/main/docs/snapshot_restore.md).
- Hyperlight:
  - [Hyperlight: a 0.0009-second micro-VM execution time](https://opensource.microsoft.com/blog/2025/02/11/hyperlight-creating-a-0-0009-second-micro-vm-execution-time/) (2025-02-11);
  - [Hyperlight README](https://cdn.jsdelivr.net/gh/hyperlight-dev/hyperlight@main/README.md) ("pre-1.0", "A Cloud Native Computing Foundation sandbox project");
  - hyperlight-wasm's README ("experimental").
- [alibaba/OpenSandbox](https://github.com/alibaba/OpenSandbox) (Apache 2.0; the March 2026 date is from Reinvently only).
- [Sandlock: Confining AI Agent Code with Unprivileged Linux Primitives](https://arxiv.org/pdf/2605.26298), Cong Wang and Yusheng Zheng, 2026-05-25.
- boat: [boat on EveryDev](https://www.everydev.ai/tools/boat) and boat.dev.
- [wasmtime `Config`](https://docs.wasmtime.dev/api/wasmtime/struct.Config.html) (fuel, epoch interruption).
- [Landlock userspace API](https://docs.kernel.org/userspace-api/landlock.html) (TCP rules from ABI 4; signal and abstract-socket scoping from ABI 6; the kernel releases are **(verify)**).
- [Ubuntu 23.10 restricted unprivileged user namespaces](https://ubuntu.com/blog/ubuntu-23-10-restricted-unprivileged-user-namespaces); the 24.04 default is **(verify)** against [Understanding AppArmor user namespace restriction](https://discourse.ubuntu.com/t/understanding-apparmor-user-namespace-restriction/58007).
- [cloudflare/workerd README](https://github.com/cloudflare/workerd) ("not a hardened sandbox").
- In-repository:
  - §15, §24, §45, §49 and HS1 (R1.10, R1.11, R6.1, R6.6, R-ACL.1), §50, §53;
  - the code paths in §3.


> **See also (2026-10-17):** [§61 Loams Jailer](61-loams-jailer.md) builds on this sandbox: it adds `pause`, `resume`, `set_net_policy`, `set_limits` and a `SandboxEvent` stream (D1096), plus sensors and actuators per adapter (§61 §4.6).
