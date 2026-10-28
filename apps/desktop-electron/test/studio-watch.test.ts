// Main's watch of the daemon for the tray and OS notifications (SD1 Task 28). The
// fake fetch answers Connect server streams with real protobuf frames.
import {
	create,
	type DescMessage,
	type MessageInitShape,
	toBinary,
} from "@bufbuild/protobuf";
import {
	NotificationKind,
	SessionStatus,
	WatchApprovalsResponseSchema,
	WatchNotificationsResponseSchema,
	WatchSessionsResponseSchema,
	WatchThreadsResponseSchema,
} from "@loams/proto/agentd";
import { describe, expect, it } from "vite-plus/test";
import { AgentdToken } from "../src/main/agentd/client";
import { watchStudio } from "../src/main/agentd/studio-watch";
import type {
	StudioNotice,
	StudioSnapshot,
} from "../src/main/shell/studio-tray-model";

function frame(flags: number, bytes: Uint8Array): Uint8Array {
	const out = new Uint8Array(5 + bytes.length);
	out[0] = flags;
	new DataView(out.buffer).setUint32(1, bytes.length);
	out.set(bytes, 5);
	return out;
}

function stream<D extends DescMessage>(
	schema: D,
	items: MessageInitShape<D>[],
	end: object = {},
): Response {
	const parts = [
		...items.map((i) => frame(0, toBinary(schema, create(schema, i)))),
		frame(2, new TextEncoder().encode(JSON.stringify(end))),
	];
	const body = new Uint8Array(parts.reduce((n, p) => n + p.length, 0));
	let at = 0;
	for (const p of parts) {
		body.set(p, at);
		at += p.length;
	}
	return new Response(body, {
		status: 200,
		headers: { "content-type": "application/connect+proto" },
	});
}

const unimplemented = (schema: DescMessage) =>
	stream(schema, [], {
		error: { code: "unimplemented", message: "G17 lands with SD1 Task 13" },
	});

async function until(cond: () => boolean): Promise<void> {
	for (let i = 0; i < 200 && !cond(); i++)
		await new Promise((r) => setTimeout(r, 5));
	expect(cond()).toBe(true);
}

describe("studio watch", () => {
	it("folds threads, sessions and approvals, and passes on new notifications", async () => {
		const seen: { url: string; auth: string | null; origin: string | null }[] =
			[];
		const fetchImpl = async (
			input: string | URL | Request,
			init?: RequestInit,
		) => {
			const url = String(input instanceof Request ? input.url : input);
			const headers = new Headers(init?.headers);
			seen.push({
				url,
				auth: headers.get("authorization"),
				origin: headers.get("origin"),
			});
			if (url.endsWith("/WatchThreads"))
				return stream(WatchThreadsResponseSchema, [
					{
						chats: [
							{ id: "c1", title: "Fix login", createdAtMs: 1n },
							{ id: "c2", title: "Old", archived: true, createdAtMs: 1n },
						],
					},
				]);
			if (url.endsWith("/WatchSessions"))
				return stream(WatchSessionsResponseSchema, [
					{
						sessions: [
							{ chatId: "c1", status: SessionStatus.WORKING, updatedAtMs: 1n },
							{ chatId: "c2", status: SessionStatus.IDLE, updatedAtMs: 1n },
						],
					},
				]);
			if (url.endsWith("/WatchApprovals"))
				return stream(WatchApprovalsResponseSchema, [
					{
						pending: [
							{ id: "a1", chatId: "c1", tool: "Bash", createdAtMs: 1n },
						],
					},
				]);
			if (url.endsWith("/WatchNotifications"))
				return stream(WatchNotificationsResponseSchema, [
					// From the daemon's buffer, before this watch: not news.
					{
						notification: {
							id: "n0",
							kind: NotificationKind.TURN_DONE,
							chatId: "c1",
							title: "x",
							createdAtMs: 10n,
						},
					},
					{
						notification: {
							id: "n1",
							kind: NotificationKind.APPROVAL,
							chatId: "c1",
							title: "Fix login",
							createdAtMs: 5_000n,
						},
					},
					{
						notification: {
							id: "n1",
							kind: NotificationKind.APPROVAL,
							chatId: "c1",
							title: "dup",
							createdAtMs: 5_000n,
						},
					},
				]);
			return new Response("", { status: 404 });
		};
		let snap: StudioSnapshot | undefined;
		const notices: StudioNotice[] = [];
		const w = watchStudio({
			target: () => ({
				url: "http://127.0.0.1:4567",
				token: new AgentdToken("sekret"),
			}),
			fetch: fetchImpl as typeof fetch,
			now: () => 1_000,
			onSnapshot: (s) => {
				snap = s;
			},
			onNotice: (n) => notices.push(n),
			retryMs: 60_000,
		});
		try {
			await until(
				() => snap?.approvals.length === 1 && snap.working.size === 1,
			);
			expect(snap?.connected).toBe(true);
			expect([...(snap?.threads.keys() ?? [])]).toEqual(["c1"]);
			expect([...(snap?.working ?? [])]).toEqual(["c1"]);
			await until(() => notices.length === 1);
			expect(notices[0]).toMatchObject({
				id: "n1",
				kind: "approval",
				chatId: "c1",
				title: "Fix login",
			});
			// Main adds the token; no Origin, and only the four watches are called.
			expect(
				seen.every((s) => s.auth === "Bearer sekret" && s.origin === null),
			).toBe(true);
			expect(new Set(seen.map((s) => new URL(s.url).pathname))).toEqual(
				new Set([
					"/loams.agentd.v1.ThreadService/WatchThreads",
					"/loams.agentd.v1.ThreadService/WatchSessions",
					"/loams.agentd.v1.ApprovalService/WatchApprovals",
					"/loams.agentd.v1.NotificationService/WatchNotifications",
				]),
			);
		} finally {
			w.stop();
		}
	});

	it("treats an unimplemented watch as absent and does not retry it", async () => {
		const calls: string[] = [];
		const fetchImpl = async (input: string | URL | Request) => {
			const url = String(input instanceof Request ? input.url : input);
			calls.push(new URL(url).pathname.split("/").pop() ?? "");
			if (url.endsWith("/WatchThreads"))
				return stream(WatchThreadsResponseSchema, [
					{ chats: [{ id: "c1", title: "A", createdAtMs: 1n }] },
				]);
			if (url.endsWith("/WatchSessions"))
				return stream(WatchSessionsResponseSchema, [{ sessions: [] }]);
			if (url.endsWith("/WatchApprovals"))
				return unimplemented(WatchApprovalsResponseSchema);
			return unimplemented(WatchNotificationsResponseSchema);
		};
		let snap: StudioSnapshot | undefined;
		const w = watchStudio({
			target: () => ({
				url: "http://127.0.0.1:4567",
				token: new AgentdToken("t"),
			}),
			fetch: fetchImpl as typeof fetch,
			now: () => 0,
			onSnapshot: (s) => {
				snap = s;
			},
			onNotice: () => undefined,
			retryMs: 1,
		});
		try {
			await until(() => snap?.connected === true);
			await new Promise((r) => setTimeout(r, 50));
			expect(calls.filter((c) => c === "WatchApprovals")).toHaveLength(1);
			expect(calls.filter((c) => c === "WatchNotifications")).toHaveLength(1);
			expect(snap?.approvals).toEqual([]);
		} finally {
			w.stop();
		}
	});

	it("reports no daemon as an empty, disconnected snapshot", async () => {
		let snap: StudioSnapshot | undefined;
		let asked = 0;
		const w = watchStudio({
			target: () => {
				asked++;
				return null;
			},
			fetch: (async () => {
				throw new Error("not called");
			}) as typeof fetch,
			now: () => 0,
			onSnapshot: (s) => {
				snap = s;
			},
			onNotice: () => undefined,
			retryMs: 1,
		});
		try {
			await until(() => asked > 2);
			expect(snap?.connected ?? false).toBe(false);
		} finally {
			w.stop();
		}
	});
});
