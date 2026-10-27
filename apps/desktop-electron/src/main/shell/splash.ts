// The splash gate (D1084): the main window shows when the console reports ready, or after
// SPLASH_TIMEOUT_MS so a slow or failed start still shows the console's own boot screen.

export const SPLASH_TIMEOUT_MS = 30_000;

export class SplashGate {
	#done = false;
	readonly #cancel: () => void;

	constructor(
		private readonly o: {
			timeoutMs: number;
			onShow: () => void;
			schedule?: (fn: () => void, ms: number) => () => void;
		},
	) {
		const schedule =
			o.schedule ??
			((fn: () => void, ms: number) => {
				const t = setTimeout(fn, ms);
				return () => clearTimeout(t);
			});
		this.#cancel = schedule(() => this.#show(), o.timeoutMs);
	}

	ready(): void {
		this.#show();
	}

	dispose(): void {
		this.#cancel();
	}

	#show(): void {
		if (this.#done) return;
		this.#done = true;
		this.#cancel();
		this.o.onShow();
	}
}
