// Loams Studio end to end on the desktop edition (SD1 Task 29; §53 §10): the
// built app under Playwright's Electron driver, against a real loams-agentd
// running the Mock harness in `<userData>/agentd`, where Electron main finds
// `agentd.json` and `agentd.token` and proxies `loams-app://console/
// loams.agentd.v1.*` to its loopback Connect listener (SD1 Task 5). Main attaches
// to that daemon at start (0.1.0 release fix B9) and starts its own only when none
// answers, so the first scenario checks it attached to this one.
//
// Run `pnpm build` and build the console first, and `cargo build -p loams-agentd`
// (or set LOAMS_AGENTD_BIN), and set LOAMS_E2E_STUDIO=1. LOAMS_E2E_ELECTRON_ARGS: extra electron args, such
// as "--no-sandbox". Linux needs a display: run it under `xvfb-run -a`.
//
// The scenarios share the web edition's steps (web/apps/console/test/e2e/studio.ts).

import { chmodSync, mkdirSync, mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, resolve } from "node:path";
import {
	type ElectronApplication,
	_electron as electron,
	expect,
	type Page,
	test,
} from "@playwright/test";
import { type Agentd, startAgentd } from "../../../../web/apps/console/test/e2e/fixtures/agentd";
import {
	commentOnDiff,
	diffOf,
	expectMockEdit,
	go,
	hashOf,
	modes,
	openInspectorTab,
	projectHref,
	startThread,
	timeline,
} from "../../../../web/apps/console/test/e2e/studio";
import { mainWindow } from "./windows";

const appRoot = resolve(__dirname, "../..");
const fakeEngine = join(appRoot, "test/fixtures/fake-engine.mjs");

/** An executable that runs the fake engine (as the smoke test does); `--help` answers at once. */
function fakeEngineWrapper(dir: string): string {
	const f = join(dir, "loams");
	writeFileSync(
		f,
		`#!/bin/sh\ncase "$*" in *--help*) exit 0;; esac\nexec "${process.execPath}" "${fakeEngine}" "$@"\n`,
	);
	chmodSync(f, 0o755);
	return f;
}

/** Main's view of the agent daemon (`loamsDesktop.agentd.status()`). */
function agentdStatus(page: Page): Promise<Record<string, unknown>> {
	return page.evaluate(() =>
		(
			globalThis as unknown as {
				loamsDesktop: { agentd: { status(): Promise<Record<string, unknown>> } };
			}
		).loamsDesktop.agentd.status(),
	);
}

/** Settings › Studio's approval mode for new threads (`loams.studio.settings`). */
async function setApprovalMode(page: Page, mode: "supervised" | "full-access") {
	await page.evaluate((m) => {
		const key = "loams.studio.settings";
		const prev = JSON.parse(localStorage.getItem(key) ?? "{}") as Record<string, unknown>;
		localStorage.setItem(key, JSON.stringify({ ...prev, approvalMode: m }));
	}, mode);
}

test.describe("studio on the desktop edition", () => {
	test.describe.configure({ mode: "serial" });
	// Opt in: it needs a debug loams-agentd, which the desktop workflow's smoke does not
	// build. .github/workflows/studio-e2e.yml sets LOAMS_E2E_STUDIO=1.
	test.skip(
		!process.env.LOAMS_E2E_STUDIO,
		"set LOAMS_E2E_STUDIO=1 (and build loams-agentd) to run Studio's e2e",
	);
	test.skip(process.platform !== "linux", "the Studio e2e runs on Linux (CI: xvfb)");

	let work = "";
	let agentd: Agentd;
	let app: ElectronApplication | undefined;
	let page: Page;
	let threadId = "";

	test.beforeAll(async () => {
		work = mkdtempSync(join(tmpdir(), "loams-desktop-studio-e2e-"));
		const userData = join(work, "user-data");
		mkdirSync(userData);
		// Background agents answered "off" (child mode): nothing asks for consent.
		writeFileSync(
			join(userData, "settings.json"),
			`${JSON.stringify({ "daemon.background": "off" }, null, 2)}\n`,
		);
		// The daemon in `<userData>/agentd`, the Mock asking before each edit in a
		// supervised thread (Task 13); the main flow's threads are full access.
		agentd = await startAgentd({
			mode: "local",
			root: work,
			dir: join(userData, "agentd"),
			mockEdit: ["notes.txt"],
			mockApprove: true,
		});
		const env: Record<string, string> = {};
		for (const [k, v] of Object.entries(process.env))
			if (v !== undefined && k !== "ELECTRON_RUN_AS_NODE") env[k] = v;
		// The real daemon client, never the development fixture mock.
		delete env.LOAMS_DESKTOP_AGENTD_MOCK;
		env.HOME = agentd.home;
		env.LOAMS_BIN = fakeEngineWrapper(work);
		env.LOAMS_DESKTOP_USER_DATA = userData;
		const extra = (process.env.LOAMS_E2E_ELECTRON_ARGS ?? "").split(/\s+/).filter(Boolean);
		app = await electron.launch({ args: [...extra, appRoot], cwd: appRoot, env });
		page = await mainWindow(app);
		// The folder picker answers with the fixture repository.
		await app.evaluate(({ dialog }, repo) => {
			const answer = async () => ({ canceled: false, filePaths: [repo] });
			Object.assign(dialog, { showOpenDialog: answer });
		}, agentd.repo);
		await expect.poll(() => hashOf(page), { timeout: 60_000 }).toBe("/studio");
	});

	test.afterAll(async () => {
		if (app) await app.close().catch(() => undefined);
		if (agentd) {
			if (test.info().status !== "passed") console.log(agentd.log().slice(-4000));
			await agentd.stop();
		}
		if (work) rmSync(work, { recursive: true, force: true });
	});

	test("add_project_from_fixture_repo", async () => {
		// Main attached at start to the daemon in `<userData>/agentd` (this test's),
		// rather than starting one of its own: no Start to press.
		await expect
			.poll(() => agentdStatus(page), { timeout: 30_000 })
			.toMatchObject({ state: "running", pid: agentd.pid, dataDir: agentd.dir });
		await expect(page.getByRole("button", { name: "Start" })).toHaveCount(0);
		await expect(page.getByText("No projects yet").first()).toBeVisible({ timeout: 30_000 });
		await setApprovalMode(page, "full-access");
		await page.getByRole("button", { name: "Add project" }).click();
		const projects = page.getByRole("navigation", { name: "Projects" });
		await expect(projects.getByRole("link", { name: /^fixture/ })).toBeVisible();
		const repos = await agentd.call<{ repos?: { path?: string }[] }>(
			"/loams.agentd.v1.ProjectService/ListRepos",
		);
		expect(repos.repos?.map((r) => r.path)).toContain(agentd.repo);
	});

	test("factory_group_visible_in_studio_only", async () => {
		// The sidebar's Factory group; Console's overview has its own Software Factory card.
		const factoryNav = page
			.getByRole("navigation", { name: "Sidebar" })
			.locator('a[href="#/studio/factory"]');
		await expect(factoryNav.first()).toBeVisible();
		await modes(page).console.click();
		await expect.poll(() => hashOf(page)).toBe("/");
		await expect(factoryNav).toHaveCount(0);
		await modes(page).studio.click();
		await expect.poll(() => hashOf(page)).toBe("/studio");
		await expect(factoryNav.first()).toBeVisible();
	});

	test("thread_runs_and_shows_diff", async () => {
		threadId = await startThread(page, agentd.repo, "Please edit the notes");
		// The diff is the shell inspector's Diff tab (design 59).
		await openInspectorTab(page, "Diff");
		await expectMockEdit(page, 1);
		await expect(page.getByRole("main")).toContainText(/Mock\s*full-access/);
		expect(agentd.git(["status", "--porcelain"])).toBe("");
	});

	test("comment_and_send_review_to_agent", async () => {
		await commentOnDiff(page, "notes.txt", "Please add another line");
		await page.getByRole("button", { name: "Send 1 comment to the agent" }).click();
		await expectMockEdit(page, 2);
	});

	test("restore_turn", async () => {
		const restore = timeline(page).getByRole("button", { name: "Restore to before this turn" });
		await expect(restore).toHaveCount(2);
		await restore.last().click();
		await page
			.getByRole("alertdialog", { name: "Restore to before this turn?" })
			.getByRole("button", { name: "Restore" })
			.click();
		await expect(diffOf(page, "notes.txt")).not.toContainText("mock edit 2");
	});

	test("commit_from_toolbar", async () => {
		await page
			.getByRole("toolbar", { name: "Branch and git" })
			.getByRole("button", { name: "Git action options" })
			.click();
		await page.getByRole("menuitem", { name: /^Commit$/ }).click();
		const dialog = page.getByRole("dialog", { name: "Commit changes" });
		await dialog.getByLabel("Commit message").fill("studio e2e: desktop commit");
		await dialog.getByRole("button", { name: "Commit", exact: true }).click();
		await expect(dialog).toBeHidden();
		await expect
			.poll(() => agentd.git(["log", "--all", "--format=%s"]))
			.toContain("studio e2e: desktop commit");
	});

	test("terminal_round_trip", async () => {
		// The terminal is the shell drawer's tab (design 59).
		await page.getByRole("button", { name: "Show drawer" }).click();
		const terminal = page.locator(".xterm").first();
		await expect(terminal).toBeVisible({ timeout: 30_000 });
		await terminal.click();
		await page.keyboard.type("echo studio-$((40 + 2))");
		await page.keyboard.press("Enter");
		await expect(page.locator(".xterm-rows").first()).toContainText("studio-42", {
			timeout: 30_000,
		});
	});

	test("run_group_of_three_compare_and_merge", async () => {
		await go(page, projectHref(agentd.repo));
		await page.getByRole("textbox", { name: "New thread prompt" }).click();
		await page.keyboard.type("Three ways to edit the notes");
		await page.getByRole("button", { name: "Run in parallel…" }).click();
		const dialog = page.getByRole("dialog", { name: "Run in parallel" });
		while ((await dialog.getByLabel(/^Variant \d+ harness$/).count()) < 3)
			await dialog.getByRole("button", { name: "Add a variant" }).click();
		for (const n of [1, 2, 3])
			await dialog.getByLabel(`Variant ${n} harness`).selectOption({ label: "Mock" });
		await dialog.getByLabel("Approvals").selectOption("full-access");
		await dialog.getByRole("button", { name: "Start 3 runs" }).click();
		await expect.poll(() => hashOf(page), { timeout: 30_000 }).toMatch(/^\/studio\/g\//);
		await expect(page.getByText("Ready to resolve")).toBeVisible({ timeout: 90_000 });
		await expect(page.getByRole("table", { name: "Files changed by each variant" })).toContainText(
			"notes.txt",
		);
		await page.getByRole("button", { name: "Resolve…" }).click();
		const resolve = page.getByRole("dialog", { name: "Resolve the run group" });
		await resolve.getByRole("radio", { name: /Merge into the base/ }).check();
		await resolve.getByRole("button", { name: "Merge into main" }).click();
		await expect(page.getByText(/^Resolved: /).first()).toBeVisible({ timeout: 60_000 });
		expect(agentd.git(["show", "main:notes.txt"])).toContain("mock edit 1");
	});

	test("supervised_approval_blocks_then_runs", async () => {
		await setApprovalMode(page, "supervised");
		await startThread(page, agentd.repo, "Edit the notes, with my approval");
		const allow = page.getByRole("button", { name: "Allow once" });
		await expect(allow).toBeVisible({ timeout: 60_000 });
		await expect(page.getByRole("main")).toContainText(/Mock\s*supervised/);
		await allow.click();
		await expectMockEdit(page, 1);
		await expect(page.getByRole("button", { name: "Allow once" })).toHaveCount(0);
	});

	/** A supervised thread started through the API, so the daemon waits on its first edit. */
	async function supervisedThread(prompt: string): Promise<string> {
		const chatId = `e2e-${Date.now()}`;
		await agentd.call("/loams.agentd.v1.ApprovalService/SetThreadApprovalMode", {
			chatId,
			mode: "APPROVAL_MODE_SUPERVISED",
		});
		await agentd.call("/loams.agentd.v1.ThreadService/Send", {
			chatId,
			messageId: `${chatId}-m1`,
			request: { prompt, cwd: agentd.repo, sandbox: "SANDBOX_LEVEL_WORKSPACE_WRITE" },
		});
		return chatId;
	}

	/** Allows a waiting thread's edit from its Studio page, then goes back to Console. */
	async function allowFromStudio(chatId: string) {
		await modes(page).studio.click();
		await go(page, `/studio/t/${chatId}`);
		await page.getByRole("button", { name: "Allow once" }).click();
		await expect(timeline(page).getByRole("button", { name: "Edited notes.txt" })).toBeVisible({
			timeout: 60_000,
		});
		await modes(page).console.click();
		await expect.poll(() => hashOf(page)).toBe("/");
	}

	test("badge_live_in_console_mode", async () => {
		// In Console, an approval the daemon starts waiting on shows on the thread's
		// row in the shell sidebar: Studio's streams stay live outside Studio (§53 §4.2).
		await modes(page).console.click();
		await expect.poll(() => hashOf(page)).toBe("/");
		const chatId = await supervisedThread("Edit while Console is open");
		const waiting = page
			.getByRole("navigation", { name: "Projects" })
			.getByRole("link", { name: /^Edit while Console is open/ })
			.getByRole("img", { name: "Pending Approval" });
		await expect(waiting).toBeVisible({ timeout: 60_000 });
		// Resolved in Studio, it leaves the badge.
		await allowFromStudio(chatId);
		await expect(waiting).toHaveCount(0);
		expect(threadId).not.toBe("");
	});

	// The header's approvals chip comes from @loams/plugin-approvals, which `requires.api`
	// loams.approvals.v1. The Loams engine does not serve that package, so on a real
	// engine the plugin does not load: no chip, no #/approvals page, no tray item for it.
	// The daemon's approvals show where Studio shows them (the thread row badge).
	test("approvals_chip_absent_without_server_approvals", async () => {
		await modes(page).console.click();
		await expect.poll(() => hashOf(page)).toBe("/");
		const chip = page.locator('a.lc-chip[href="#/approvals"]');
		const chatId = await supervisedThread("Edit while the chip is absent");
		const waiting = page
			.getByRole("navigation", { name: "Projects" })
			.getByRole("link", { name: /^Edit while the chip is absent/ })
			.getByRole("img", { name: "Pending Approval" });
		await expect(waiting).toBeVisible({ timeout: 60_000 });
		await expect(chip).toHaveCount(0);
		await allowFromStudio(chatId);
		await expect(chip).toHaveCount(0);
	});
});
