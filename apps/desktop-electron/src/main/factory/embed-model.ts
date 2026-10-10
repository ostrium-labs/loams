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
 * Scale `rect` by the console's zoom factor (CSS px -> window DIPs), round it
 * and clamp it to the window's content area. Negative x/y clamp to 0 and shrink
 * the rect; a rect with no area left (or a negative size) clamps to `null`:
 * hide the view. Non-finite or non-numeric values are rejected (`undefined`).
 */
export function clampBounds(
	rect: unknown,
	content: Size,
	zoom = 1,
): EmbedRect | null | undefined {
	if (typeof rect !== "object" || rect === null) return undefined;
	const r = rect as Record<string, unknown>;
	const { x, y, width, height } = r;
	for (const n of [x, y, width, height])
		if (typeof n !== "number" || !Number.isFinite(n)) return undefined;
	if (!Number.isFinite(zoom) || zoom <= 0) return undefined;
	const nx = (x as number) * zoom;
	const ny = (y as number) * zoom;
	const nw = (width as number) * zoom;
	const nh = (height as number) * zoom;
	if (nw <= 0 || nh <= 0) return null;
	const left = Math.max(0, Math.min(Math.round(nx), content.width));
	const top = Math.max(0, Math.min(Math.round(ny), content.height));
	const right = Math.max(0, Math.min(Math.round(nx + nw), content.width));
	const bottom = Math.max(0, Math.min(Math.round(ny + nh), content.height));
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
	/** The console's zoom factor (1 when unzoomed). */
	zoom(): number;
	isFocused(view: V): boolean;
	focusConsole(): void;
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
		const bounds = clampBounds(rect, this.a.contentSize(), this.a.zoom());
		if (bounds === undefined) {
			// Never leave a stale view showing after a bad report.
			this.hide();
			return { ok: false, code: "bad_request", message: "Invalid bounds" };
		}
		if (this.#visible && this.#visible !== app) this.hide();
		if (bounds === null) {
			this.hide();
			return { ok: true, value: undefined };
		}
		let view = this.#views.get(app);
		if (!view) {
			// Make room first, so there are never more than `max` live views.
			while (this.#lru.list().length >= this.#lru.max) {
				const oldest = this.#lru.list()[0];
				if (!oldest) break;
				this.destroy(oldest);
			}
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
		this.a.place(view, bounds);
		this.#visible = app;
		return { ok: true, value: undefined };
	}

	hide(): void {
		const v = this.#visible ? this.#views.get(this.#visible) : undefined;
		if (v) this.#release(v);
		if (v) this.a.conceal(v);
		this.#visible = undefined;
	}

	/** Remove, reconfigure, pop out, eviction and quit all end here. */
	destroy(app: FactoryAppId): void {
		const v = this.#views.get(app);
		this.#views.delete(app);
		this.#lru.remove(app);
		if (this.#visible === app) this.#visible = undefined;
		if (v) {
			this.#release(v);
			this.a.destroy(v);
		}
	}

	/** A hidden or destroyed view must not keep the keyboard: return it to the console. */
	#release(v: V): void {
		if (this.a.isFocused(v)) this.a.focusConsole();
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
