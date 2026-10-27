import { afterEach, describe, expect, it, vi } from "vite-plus/test";

vi.mock("electron", () => ({ contextBridge: { executeInMainWorld: vi.fn() } }));

const { notificationShimMain } = await import(
	"../src/preload/notification-shim"
);

type G = { loamsDesktop?: unknown; Notification?: unknown };
const g = globalThis as unknown as G;
const before = g.Notification;

afterEach(() => {
	delete g.loamsDesktop;
	g.Notification = before;
});

describe("the page's Notification on the desktop (D1085)", () => {
	it("posts through the shell bridge with the route, and needs no permission prompt", async () => {
		const notify = vi.fn(async () => {});
		g.loamsDesktop = { shell: { notify } };
		notificationShimMain();
		const N = g.Notification as {
			new (t: string, o?: unknown): EventTarget & { onshow: unknown };
			permission: string;
			requestPermission(): Promise<string>;
		};
		expect(N.permission).toBe("granted");
		expect(await N.requestPermission()).toBe("granted");
		const shown = vi.fn();
		const n = new N("Approval waiting", {
			body: "Deploy",
			data: { route: "/studio/t/c1" },
		});
		n.addEventListener("show", shown);
		expect(notify).toHaveBeenCalledWith(
			"Approval waiting",
			"Deploy",
			"/studio/t/c1",
		);
		await vi.waitFor(() => expect(shown).toHaveBeenCalledOnce());
	});

	it("leaves Notification alone where there is no bridge", () => {
		notificationShimMain();
		expect(g.Notification).toBe(before);
	});

	it("is self-contained, as executeInMainWorld sends only its source", () => {
		// The source must not call helpers defined outside the function.
		const src = notificationShimMain.toString();
		expect(src).not.toMatch(/__publicField|__defNormalProp|_defineProperty/);
	});
});
