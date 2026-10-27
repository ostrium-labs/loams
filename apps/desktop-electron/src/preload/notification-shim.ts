// Adapted from dataelement/dsh-desktop (MIT), src/preload/desktop-notification.ts.
// Page code that calls `new Notification(title, { body, data: { route } })` gets an OS
// notification through the existing shell bridge (`loamsDesktop.shell.notify`, D1085); a
// click focuses the window and opens `route` (main's onNotifyClick), so the page's own
// `onclick` is not called. Upstream injects a script and relays custom events both ways;
// here the shim runs in the main world with contextBridge and calls the bridge the preload
// already exposes. Main checks the sender, the lengths and the route (shell-ipc.electron.ts).
import { contextBridge } from "electron";

/** The part of the preload's bridge the shim calls. */
interface NotifyBridge {
	shell: {
		notify(title: string, body?: string, route?: string): Promise<void>;
	};
}

/**
 * Replaces `globalThis.Notification` with one that posts through `loamsDesktop.shell.notify`.
 * It runs in the page's main world, so it is self-contained: no imports and no closures.
 */
export function notificationShimMain(): void {
	type Permission = "default" | "denied" | "granted";
	const g = globalThis as unknown as {
		loamsDesktop?: NotifyBridge;
		Notification?: unknown;
	};
	const bridge = g.loamsDesktop;
	if (!bridge) return;
	// No class fields or static fields: the bundler may turn them into helper calls outside
	// this function, and executeInMainWorld sends the function's source alone.
	class DesktopNotification extends EventTarget {
		declare readonly title: string;
		declare readonly body: string;
		declare readonly tag: string;
		declare readonly data: unknown;
		declare onclick: ((e: Event) => void) | null;
		declare onclose: ((e: Event) => void) | null;
		declare onerror: ((e: Event) => void) | null;
		declare onshow: ((e: Event) => void) | null;
		static get permission(): Permission {
			return "granted";
		}
		static get maxActions(): number {
			return 0;
		}
		static requestPermission(
			callback?: (p: Permission) => void,
		): Promise<Permission> {
			const p = Promise.resolve<Permission>("granted");
			if (typeof callback === "function") void p.then(callback);
			return p;
		}
		constructor(
			title: string,
			options?: { body?: unknown; tag?: unknown; data?: unknown },
		) {
			super();
			const opts: { body?: unknown; tag?: unknown; data?: unknown } =
				options && typeof options === "object" ? options : {};
			const self = this as {
				-readonly [K in keyof DesktopNotification]: DesktopNotification[K];
			};
			self.title = String(title ?? "");
			self.body = typeof opts.body === "string" ? opts.body : "";
			self.tag = typeof opts.tag === "string" ? opts.tag : "";
			self.data = opts.data;
			self.onclick = null;
			self.onclose = null;
			self.onerror = null;
			self.onshow = null;
			const data = opts.data as { route?: unknown } | null | undefined;
			const route =
				data && typeof data === "object" && typeof data.route === "string"
					? data.route
					: undefined;
			bridge?.shell.notify(self.title, self.body, route).then(
				() => this.fire("show"),
				() => this.fire("error"),
			);
		}
		close(): void {
			this.fire("close");
		}
		fire(type: "show" | "close" | "error"): void {
			const ev = new Event(type);
			const handler = {
				show: this.onshow,
				close: this.onclose,
				error: this.onerror,
			}[type];
			if (typeof handler === "function") handler.call(this, ev);
			this.dispatchEvent(ev);
		}
	}
	g.Notification = DesktopNotification;
}

/** Installs the shim in the page's main world; called after the bridge is exposed. */
export function installNotificationShim(): void {
	contextBridge.executeInMainWorld({ func: notificationShimMain });
}
