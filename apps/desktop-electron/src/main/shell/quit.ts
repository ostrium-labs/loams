// The quit order, without Electron so it is unit-tested: state is flushed first (agent turns
// stopped and their chat files written), then either the quit-time install of a verified update
// (which stops the engine itself, D661) or a plain engine stop. Every step is time-bounded.

export interface QuitDeps {
	/** Stops agent turns and writes their chat files. */
	flush: () => Promise<unknown>;
	stopEngine: () => Promise<unknown>;
	hasVerifiedDownload: () => boolean;
	/** Resolves true when quitAndInstall was invoked. */
	installOnQuit: () => Promise<boolean>;
	timeoutMs?: number;
}

export const QUIT_STEP_TIMEOUT_MS = 6000;

const bounded = (p: Promise<unknown>, ms: number): Promise<unknown> =>
	Promise.race([
		p.catch(() => undefined),
		new Promise((r) => setTimeout(r, ms).unref?.()),
	]);

export async function quitSequence(d: QuitDeps): Promise<void> {
	const ms = d.timeoutMs ?? QUIT_STEP_TIMEOUT_MS;
	await bounded(Promise.resolve().then(d.flush), ms);
	if (d.hasVerifiedDownload()) {
		const started = await d.installOnQuit().catch(() => false);
		if (started) return;
	}
	await bounded(Promise.resolve().then(d.stopEngine), ms);
}
