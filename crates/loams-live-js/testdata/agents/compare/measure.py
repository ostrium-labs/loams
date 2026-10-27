#!/usr/bin/env python3
"""The agent-profile spike's measurements (MS1 Task 1), from outside, over
ACP on stdio, for the QuickJS profile and the same bundle on node and bun:

    measure.py --qjs <loams-agent-worker> [--node node] [--bun bun] [--runs 50]

QuickJS runs `loams-agent-worker acp <bundle> --fake-hosts` (agentd's side
as a process, plus the sealed worker it spawns); node and bun run
`compare/host.mjs <bundle>`. Prints JSON.
"""
import argparse, json, os, statistics, subprocess, time

HERE = os.path.dirname(os.path.abspath(__file__))
AGENTS = os.path.dirname(HERE)


def tree(pid):
    pids = [pid]
    try:
        for tid in os.listdir(f"/proc/{pid}/task"):
            with open(f"/proc/{pid}/task/{tid}/children") as f:
                for child in f.read().split():
                    pids += tree(int(child))
    except OSError:
        pass
    return pids


def status_kb(pid, field):
    try:
        with open(f"/proc/{pid}/status") as f:
            for line in f:
                if line.startswith(field):
                    return int(line.split()[1])
    except OSError:
        return 0
    return 0


def rss_mib(pid):
    per = {p: status_kb(p, "VmRSS:") / 1024 for p in tree(pid)}
    return round(sum(per.values()), 1), [round(v, 1) for v in per.values()]


class Agent:
    def __init__(self, cmd):
        self.p = subprocess.Popen(cmd, stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL, bufsize=0)
        self.next = 1

    def request(self, method, params):
        i = self.next
        self.next += 1
        self.p.stdin.write((json.dumps({"jsonrpc": "2.0", "id": i, "method": method, "params": params}) + "\n").encode())
        said = []
        while True:
            line = self.p.stdout.readline()
            if not line:
                raise RuntimeError(f"{method}: the agent exited")
            m = json.loads(line)
            if m.get("id") == i and "method" not in m:
                return m, "".join(said)
            u = m.get("params", {}).get("update", {})
            if u.get("sessionUpdate") == "agent_message_chunk":
                said.append(u["content"]["text"])

    def close(self):
        self.p.kill()
        self.p.wait()


def pct(xs, q):
    xs = sorted(xs)
    return round(xs[min(len(xs) - 1, int(round(q * (len(xs) - 1))))], 2)


def measure(cmd, runs):
    out = {}
    cold = []
    for _ in range(runs):
        t = time.perf_counter()
        a = Agent(cmd)
        a.request("initialize", {"protocolVersion": 1})
        cold.append((time.perf_counter() - t) * 1000)
        a.close()
    out["cold_start_ms"] = {"p50": pct(cold, 0.5), "p95": pct(cold, 0.95)}
    a = Agent(cmd)
    a.request("initialize", {"protocolVersion": 1})
    session = a.request("session/new", {"cwd": "/tmp", "mcpServers": []})[0]["result"]["sessionId"]
    time.sleep(0.5)
    out["idle_rss_mib"] = rss_mib(a.p.pid)
    turns = []
    for _ in range(20):
        t = time.perf_counter()
        r, _ = a.request("session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": "weather in Paris?"}]})
        assert r["result"]["stopReason"] == "end_turn", r
        turns.append((time.perf_counter() - t) * 1000)
    out["turn_ms"] = {"p50": pct(turns, 0.5), "p95": pct(turns, 0.95)}
    out["rss_after_20_turns_mib"] = rss_mib(a.p.pid)
    out["peak_rss_mib"] = round(sum(status_kb(p, "VmHWM:") for p in tree(a.p.pid)) / 1024, 1)
    _, said = a.request("session/prompt", {"sessionId": session, "prompt": [{"type": "text", "text": "bench"}]})
    out["host_call_ms"] = json.loads(said)
    a.close()
    return out


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--qjs", required=True)
    ap.add_argument("--node", default="node")
    ap.add_argument("--bun")
    ap.add_argument("--runs", type=int, default=50)
    args = ap.parse_args()
    bundle = os.path.join(AGENTS, "tool-agent", "agent.js")
    host = os.path.join(HERE, "host.mjs")
    runtimes = {"quickjs": [args.qjs, "acp", bundle, "--fake-hosts"], "node": [args.node, host, bundle]}
    if args.bun:
        runtimes["bun"] = [args.bun, host, bundle]
    print(json.dumps({name: measure(cmd, args.runs) for name, cmd in runtimes.items()}, indent=2))


if __name__ == "__main__":
    main()
