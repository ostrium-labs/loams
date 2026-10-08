// Pure model for the embedded factory views (D678): bounds validation and an
// LRU of live views. No electron import, so it is unit-testable.
import type {
	EmbedRect,
	FactoryAppId,
	IpcResult,
} from "../../shared/contracts";

export const MAX_LIVE_VIEWS = 4;

export interface Size {
	width: number;
	height: number;
}

/**
 * Round and clamp `rect` to the window's content area. Non-finite, negative or
 * non-numeric values are rejected (`undefined`). A rect that falls entirely
 * outside the content (or has no area) clamps to `null`: hide the view.
 */
export function clampBounds(
	rect: unknown,
	content: Size,
): EmbedRect | null | undefined {
	if (typeof rect !== "object" || rect === null) return undefined;
	const r = rect as Record<string, unknown>;
	const { x, y, width, height } = r;
	for (const n of [x, y, width, height])
		if (typeof n !== "number" || !Number.isFinite(n) || n < 0) return undefined;
	const nx = x as number;
	const ny = y as number;
	const left = Math.min(Math.round(nx), content.width);
	const top = Math.min(Math.round(ny), content.height);
	const right = Math.min(Math.round(nx + (width as number)), content.width);
	const bottom = Math.min(Math.round(ny + (height as number)), content.height);
	if (right <= left || bottom <= top) return null;
	return { x: left, y: top, width: right - left, height: bottom - top };
}

/** Most-recently-used order of live app views. */
export class Lru {
	#order: FactoryAppId[] = [];
	constructor(readonly max = MAX_LIVE_VIEWS) {}
	/** Mark `app` most recent; returns the apps evicted to stay within `max`. */
	touch(app: FactoryAppId): FactoryAppId[] {
		this.remove(app);
		this.#order.push(app);
		const out: FactoryAppId[] = [];
		while (this.#order.length > this.max) {
			const old = this.#order.shift();
			if (old) out.push(old);
		}
		return out;
	}
	remove(app: FactoryAppId): void {
		this.#order = this.#order.filter((a) => a !== app);
	}
	has(app: FactoryAppId): boolean {
		return this.#order.includes(app);
	}
	/** Least recent first. */
	list(): FactoryAppId[] {
		return [...this.#order];
	}
}

/** The shortcut that sends focus back to the console, from inside an app view. */
export function isFocusConsoleKey(input: {
	type: string;
	key: string;
	control: boolean;
	meta: boolean;
	alt: boolean;
	shift: boolean;
}): boolean {
	return (
		input.type === "keyDown" &&
		(input.control || input.meta) &&
		!input.alt &&
		!input.shift &&
		input.key.toLowerCase() === "l"
	);
}

export interface EmbedAdapter<V> {
	contentSize(): Size;
	/** A new hidden view loading the app, or undefined when it is not configured. */
	create(app: FactoryAppId): V | undefined;
	place(view: V, bounds: EmbedRect): void;
	conceal(view: V): void;
	destroy(view: V): void;
}

/** Owns the live views: one visible at most, LRU-bounded, destroyed on demand. */
export class EmbedController<V> {
	readonly #views = new Map<FactoryAppId, V>();
	readonly #lru = new Lru();
	#visible: FactoryAppId | undefined;
	constructor(private readonly a: EmbedAdapter<V>) {}

	show(app: FactoryAppId, rect: unknown): IpcResult<void> {
		const bounds = clampBounds(rect, this.a.contentSize());
		if (bounds === undefined)
			return { ok: false, code: "bad_request", message: "Invalid bounds" };
		if (this.#visible && this.#visible !== app) this.hide();
		let view = this.#views.get(app);
		if (!view) {
			view = this.a.create(app);
			if (!view)
				return {
					ok: false,
					code: "unconfigured",
					message: "This app is not configured yet",
				};
			this.#views.set(app, view);
		}
		for (const evicted of this.#lru.touch(app)) this.destroy(evicted);
		if (bounds === null) {
			this.a.conceal(view);
			this.#visible = undefined;
		} else {
			this.a.place(view, bounds);
			this.#visible = app;
		}
		return { ok: true, value: undefined };
	}

	hide(): void {
		const v = this.#visible ? this.#views.get(this.#visible) : undefined;
		if (v) this.a.conceal(v);
		this.#visible = undefined;
	}

	/** Remove, reconfigure, pop out, eviction and quit all end here. */
	destroy(app: FactoryAppId): void {
		const v = this.#views.get(app);
		this.#views.delete(app);
		this.#lru.remove(app);
		if (this.#visible === app) this.#visible = undefined;
		if (v) this.a.destroy(v);
	}

	popOut(
		app: FactoryAppId,
		openWindow: (app: FactoryAppId) => IpcResult<void>,
	): IpcResult<void> {
		const r = openWindow(app);
		if (r.ok) this.destroy(app);
		return r;
	}

	destroyAll(): void {
		for (const app of [...this.#views.keys()]) this.destroy(app);
	}

	get(app: FactoryAppId): V | undefined {
		return this.#views.get(app);
	}
	live(): FactoryAppId[] {
		return this.#lru.list();
	}
}
