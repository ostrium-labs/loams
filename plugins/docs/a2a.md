# The A2A endpoint

A2A is a **protocol translation layer over the agent bus**, not a second
dispatch path. `packages/core/src/a2a.ts` builds the external wire shapes;
`packages/core/src/bus.ts` does delivery. Every `message:send` becomes an
`AgentMessage`, is handed to the bus, and the reply is translated back.

That layering is the point. A second dispatch path would give the platform two
answers to "is this agent loaded?" and two places to get A2A error shapes wrong.

Tests: `packages/core/tests/a2a.spec.ts`.

## Routes

| Method | Path                           | Notes                                                         |
| ------ | ------------------------------ | ------------------------------------------------------------- |
| `GET`  | `/.well-known/agent-card.json` | Aggregate card. **Loaded plugins only.**                      |
| `GET`  | `/.well-known/agent.json`      | Pre-v1.0 spelling. Same handler, same body. Never advertised. |
| `GET`  | `/.well-known/agent-card/:id`  | One plugin's card. Refused when the plugin is off.            |
| `POST` | `/a2a/v1/message:send`         | The only implemented method.                                  |

All four are ordinary entries in the mutable router table (`mountA2A` →
`router.addAll(..., "core:a2a")`), which is why they participate in the same
add/remove lifecycle as plugin routes.

## The aggregate card lists loaded agents, not declared ones

```ts
const loaded = deps.registry
  .list()
  .filter((plugin) => plugin.agent && plugin.enabled && plugin.state === "loaded");
```

Advertising a plugin that merely _declares_ an agent would make the card promise
a route that answers 404 — the exact REST/A2A disagreement this layer exists to
avoid. So a disabled plugin disappears from the aggregate card, and its
per-plugin card returns `AgentNotLoaded`.

Skill ids on the **aggregate** card are namespaced as `<pluginId>.<skillId>`,
because two adapters are perfectly entitled to both declare `listChannels` and
an aggregate card that silently collapsed them would route a client's call to
whichever loaded last. **Per-plugin** cards use the bare skill id.

## The request shape

The skill travels in a `DataPart`, not in prose: it is structured routing
information, and parsing routing out of a `TextPart` is precisely what A2A
avoids.

```jsonc
POST /a2a/v1/message:send
{
  "message": {
    "kind": "message",
    "id": "msg-1",
    "role": "user",
    "parts": [
      { "kind": "text", "text": "list the dashboards" },
      { "kind": "data", "data": { "agent": "dashboard", "skill": "listDashboards", "params": {} } }
    ]
  }
}
```

`agent` is also accepted as `agentId`, and `metadata.agent` / `metadata.skill` /
`metadata.params` are honoured. A shorthand body — `{ agent, skill, params }`
with no `message` wrapper — is accepted so the endpoint is usable from `curl`
without building an envelope.

`parseSendRequest` rejects, with HTTP 400 and code `-32602`:

- a body that is not a JSON object,
- a `parts` array that is empty or not an array,
- any part whose `kind` is neither `"text"` nor `"data"`,
- a `data` part whose `data` is not an object,
- a missing or non-string `agent`, or a missing or non-string `skill`,
- a `params` that is not an object.

## The reply shape

A successful call returns a `Task`:

```jsonc
{
  "kind": "task",
  "id": "task-msg-1",
  "status": { "state": "completed", "timestamp": "...", "message": {/* the reply Message */} },
  "history": [/* the inbound message, then the reply */],
  "artifacts": [
    {
      "artifactId": "…",
      "name": "listDashboards-result",
      "description": "Result of listDashboards",
      "parts": [{ "kind": "data", "data": {/* the handler's return value */} }],
    },
  ],
}
```

Structured results ride in a `DataPart` artifact. A skill that emitted only text
gets `{ "text": "…" }` as the artifact data, so the artifact is never empty.

`A2ATaskBuilder` exists so `history` and `artifacts` are accumulated together.
A multi-step skill (fetch a dashboard, describe a widget, compile it) produces
several artifacts; hand-building the object literal is how the two arrays drift
apart.

If the handler throws, the bus error becomes a **failed Task** (HTTP 200,
`status.state: "failed"`, with the message in `status.message`) rather than an
HTTP error. The call itself succeeded — the work did not. Protocol-level
refusals (unknown agent, unknown skill, not loaded, unauthenticated,
permission denied) _are_ HTTP errors.

## Error envelope

```jsonc
{ "error": { "code": -32001, "message": "…", "data": {/* optional */} } }
```

alongside a meaningful HTTP status. `A2AErrorCode`:

| code     | meaning                                                                                                          | HTTP |
| -------- | ---------------------------------------------------------------------------------------------------------------- | ---- |
| `-32602` | `InvalidRequest` — the body is not a well-formed A2A message                                                     | 400  |
| `-32601` | `NotFound` — unknown agent, or a skill the agent does not declare                                                | 404  |
| `-32001` | `AgentNotLoaded` — the agent exists, its plugin is off                                                           | 404  |
| `-32002` | `SkillFailed` — **declared but never thrown.** A failing handler becomes a failed `Task`, not an error envelope. | —    |
| `-32003` | `Unauthenticated` — no usable credential                                                                         | 401  |
| `-32004` | `PermissionDenied` — authenticated, not permitted                                                                | 403  |
| `-32603` | `Internal` — a non-`A2AError` escaped the handler                                                                | 500  |

A `NotFound` for an unknown agent carries the list of currently loaded agents in
`error.data.agents`; a `NotFound` for an undeclared skill carries `validSkills`.

## Order of checks

`sendMessage` runs these in order, and the order is deliberate:

1. **Parse.** 400 on a malformed body.
2. **Agent exists?** 404 `NotFound`.
3. **Plugin loaded?** 404 `AgentNotLoaded`. A disabled plugin fails over A2A
   exactly as it fails over REST and from another agent.
4. **Authenticated?** 401. With auth configured, an anonymous caller is refused
   here, _before_ the scope check, so an unauthenticated caller cannot use the
   error text to enumerate which agents hold which scopes.
5. **Permitted to invoke this skill?** 403. A service token is bound to one
   `agentId` _and_ to a subset of that agent's declared skills; both bounds are
   enforced, so a token minted for agent A cannot drive agent B, and cannot reach
   a skill A never declared — including one a later plugin version adds.
6. **`requiredScopes` against the caller.** 403 with the missing names. This is
   the request-time half of enforcement: see [security.md](security.md).
7. **Skill declared on the manifest?** 404 `NotFound` with `validSkills`.

Steps 5–7 mean the same call can be answered differently for two different
callers. That is the point; see [security.md](security.md).

Steps 2–3 deliberately run **before** authentication. The consequence is that an
anonymous caller learns whether a given agent id is registered and loaded —
`404 NotFound` versus `401 Unauthenticated` — but nothing about what scopes it
requires. That trade is accepted so a disabled plugin fails identically for
every caller, including one with no credentials, which is what keeps REST and
A2A from disagreeing about what exists.

## Documented deviations from strict A2A v1.0

These are recorded, not accidental. Each names what would change and why it was
not done.

### 1. The method path is `/a2a/v1/message:send`, not `message/send` in a JSON-RPC envelope

A2A v1.0 names the method `message/send` and carries it in a JSON-RPC 2.0
envelope. This server uses the repository's ConnectRPC-style
`/<prefix>/<version>/<method>:<verb>` path convention instead, and the response
body is the bare `Task`.

The reason is scope: `message:send` is the **only** method implemented. This
file translates one call onto the bus. Adding a second dispatch path for a
partially implemented JSON-RPC envelope would hand the platform two answers to
"which agents exist". Migrating `message:send` to a full JSON-RPC envelope is a
separate change and would break every existing caller.

### 2. The card path was _not_ diverged

The card path **was** corrected to the v1.0 spelling
(`/.well-known/agent-card.json`), because that one is a discovery URL a client
fetches by convention and there is nothing to gain from diverging from it. The
pre-1.0 spelling is still served from the same handler as an alias — one route,
one handler, one body, so the alias cannot drift from the canonical path.

### 3. The cards are unsigned

The `loams-dev` design calls for a JWS (RFC 7515) over an RFC 8785 canonicalised
payload. That is not something to improvise: RFC 8785 has exact rules for member
ordering and ECMAScript number serialisation, and a signature over a payload
that is merely `JSON.stringify`-canonical is not the scheme the design
specifies. The card shape deliberately leaves room for it — a JWS travels as
`application/jose`, alongside this document rather than inside it.

**The cards served here are unsigned today.** Treat them as an availability
mechanism, not an integrity mechanism.

### 4. Streaming and push notifications are declared `false`

The bus is in-process and synchronous-per-hop. There is no SSE stream to attach
to and no webhook queue, and claiming either would be a lie a client would rely
on. `stateTransitionHistory: true` _is_ claimed, and is true: `history` is
populated on every reply.

### 5. `defaultOutputModes` includes `text/plain`

A skill that returns prose answers with a text part, so the mode is real.

## Verify

```sh
curl -s localhost:3001/.well-known/agent-card.json | jq '.skills[].id'

curl -s -X POST localhost:3001/a2a/v1/message:send \
  -H 'content-type: application/json' \
  -d '{"agent":"dashboard","skill":"listDashboards"}' | jq '.status.state'
```
