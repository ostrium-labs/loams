import { basename } from "node:path";
import type { IpcResult } from "../../shared/contracts";

export const MAX_YAML_BYTES = 256 * 1024;

export interface SaveDeps {
	/** Shows the save dialog; the chosen path, or undefined when cancelled. */
	pick(defaultName: string): Promise<string | undefined>;
	write(path: string, text: string): Promise<void>;
}

/** Writes an exported instance YAML where the user picks. The renderer never chooses a path. */
export async function saveYaml(
	deps: SaveDeps,
	name: unknown,
	text: unknown,
): Promise<IpcResult<{ saved: boolean }>> {
	if (typeof name !== "string" || typeof text !== "string")
		return { ok: false, code: "bad_request", message: "Invalid request" };
	if (Buffer.byteLength(text) > MAX_YAML_BYTES)
		return {
			ok: false,
			code: "too_large",
			message: "The YAML is larger than 256 KiB.",
		};
	const safe =
		basename(name)
			.replace(/[^\w.-]+/g, "_")
			.replace(/\.ya?ml$/i, "") || "connector";
	try {
		const path = await deps.pick(`${safe}.yaml`);
		if (!path) return { ok: true, value: { saved: false } };
		await deps.write(path, text);
		return { ok: true, value: { saved: true } };
	} catch (e) {
		return {
			ok: false,
			code: "write_failed",
			message: e instanceof Error ? e.message : String(e),
		};
	}
}
