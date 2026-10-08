// Pull handshake for deep-link navigation. Pure state machine, no electron import.
// Until the renderer has taken pending navigation once per page load, links are
// held (newest wins); afterwards they are pushed live.

export type SubmitResult = "push" | "held";

export class NavQueue {
	private pending: string | null = null;
	private ready = false;

	submit(path: string): SubmitResult {
		if (this.ready) return "push";
		this.pending = path;
		return "held";
	}

	/** Renderer pull: returns and clears the held path and marks the page ready. */
	take(): string | null {
		const p = this.pending;
		this.pending = null;
		this.ready = true;
		return p;
	}

	/** Main-window page load started: the new page has not subscribed yet. */
	reset(): void {
		this.ready = false;
	}
}
