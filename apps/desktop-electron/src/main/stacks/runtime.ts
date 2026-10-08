export interface ComposeRuntime {
	bin: string;
	/** Arguments that put `<bin>` into compose mode, before any `-p`/`-f` flags. */
	args: string[];
}

/** `docker compose`, then `podman compose`, then the standalone `docker-compose`. */
export function detectRuntime(
	which: (bin: string) => string | null,
): ComposeRuntime | null {
	if (which("docker")) return { bin: "docker", args: ["compose"] };
	if (which("podman")) return { bin: "podman", args: ["compose"] };
	if (which("docker-compose")) return { bin: "docker-compose", args: [] };
	return null;
}
