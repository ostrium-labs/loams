import type { EngineState, ServerEntry } from "../../shared/contracts";

/** Where a proxied loams-app://console request goes. Pure: no electron. */
export type Route =
	| { kind: "forward"; target: string; pathname?: string }
	| {
			kind: "reject";
			status: number;
			body: { code: string; message: string };
	  };

const DURABLE = "/durable/";
const LIVE = "/loams.live.v1.";

const reject = (status: number, code: string, message: string): Route => ({
	kind: "reject",
	status,
	body: { code, message },
});

/**
 * Per-prefix target routing. `/durable/` and `/loams.live.v1.` are served by
 * the local engine's own listeners (not the main API port); everything else
 * goes to the active server. A remote server has no durable surface until a
 * control plane serves it, so `/durable/` is a 404 there.
 */
export function routeRequest(
	pathname: string,
	server: ServerEntry,
	engine: EngineState,
): Route {
	const local = server.kind === "local";
	if (local && server.url === "")
		return reject(503, "engine_not_ready", "the local engine is not ready yet");

	if (pathname.startsWith(DURABLE)) {
		if (!local)
			return reject(
				404,
				"not_available_remote",
				"Durable execution is not available on this server.",
			);
		if (engine.phase !== "ready")
			return reject(
				503,
				"engine_not_ready",
				"the local engine is not ready yet",
			);
		return {
			kind: "forward",
			target: engine.durableUrl,
			pathname: pathname.slice(DURABLE.length - 1),
		};
	}

	if (local && pathname.startsWith(LIVE)) {
		if (engine.phase !== "ready" || !engine.liveUrl)
			return reject(
				503,
				"live_not_running",
				"Loams Live is not running in the local engine.",
			);
		return { kind: "forward", target: engine.liveUrl };
	}

	return { kind: "forward", target: server.url };
}
