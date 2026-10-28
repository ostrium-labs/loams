// "Open in editor" (SD1 Task 28, Q810; §53 §5.7): VS Code, Cursor and Zed through their
// URL schemes, or a program the person picked in the OS file dialog, run with the
// path as its one argument. Main builds the URL or the argv itself, from a path it
// checked; nothing is ever handed to a shell. Pure apart from `launchEditor`'s
// injected openers, so the tests drive it without Electron.
import type { StudioEditor, StudioEditorId } from "../../shared/contracts";

/** `studio.editor` in settings.json: `{ editor, command? }`. */
export const EDITOR_KEY = "studio.editor";

export interface EditorChoice {
	editor: StudioEditorId;
	/** The picked program's absolute path (`editor: "command"` only). */
	command?: string;
}

export type EditorLaunch =
	| { kind: "url"; url: string }
	| { kind: "argv"; file: string; args: string[] };

const MAX_PATH = 4096;
const SCHEME: Record<Exclude<StudioEditorId, "command">, string> = {
	vscode: "vscode",
	cursor: "cursor",
	zed: "zed",
};

/** An absolute local path (POSIX, or a Windows drive path) with no control characters. */
export function isLocalPath(p: unknown): p is string {
	return (
		typeof p === "string" &&
		p.length > 0 &&
		p.length <= MAX_PATH &&
		// oxlint-disable-next-line no-control-regex -- control characters are what is refused
		!/[\u0000-\u001f\u007f]/.test(p) &&
		(p.startsWith("/") || /^[A-Za-z]:[\\/]/.test(p))
	);
}

/** `/a b/c` → `/a%20b/c`; `C:\a` → `C:/a`: each segment encoded, the drive kept. */
function urlPath(p: string): string {
	const parts = p.replace(/\\/g, "/").split("/");
	const encoded = parts.map((s, i) =>
		i === 0 && /^[A-Za-z]:$/.test(s) ? s : encodeURIComponent(s),
	);
	const joined = encoded.join("/");
	return joined.startsWith("/") ? joined : `/${joined}`;
}

function position(at?: { line?: number; column?: number }): string {
	const ok = (n: number | undefined): n is number =>
		n !== undefined && Number.isInteger(n) && n > 0 && n < 1e9;
	if (!ok(at?.line)) return "";
	return ok(at?.column) ? `:${at.line}:${at.column}` : `:${at.line}`;
}

/** How to open `path` in the chosen editor; null when the path or the choice is unusable. */
export function editorLaunch(
	choice: EditorChoice,
	path: string,
	at?: { line?: number; column?: number },
	platform: string = process.platform,
): EditorLaunch | null {
	if (!isLocalPath(path)) return null;
	if (choice.editor === "command") {
		if (!isLocalPath(choice.command)) return null;
		// A macOS app bundle is a folder: `open -a` starts it with the path.
		if (platform === "darwin" && /\.app\/?$/.test(choice.command))
			return {
				kind: "argv",
				file: "/usr/bin/open",
				args: ["-a", choice.command, path],
			};
		return { kind: "argv", file: choice.command, args: [path] };
	}
	const scheme = SCHEME[choice.editor];
	if (!scheme) return null;
	return {
		kind: "url",
		url: `${scheme}://file${urlPath(path)}${position(at)}`,
	};
}

export interface LaunchDeps {
	/** `child_process.spawn`; called with `shell: false` only. */
	spawn: (
		file: string,
		args: string[],
		opts: { shell: false; detached: true; stdio: "ignore"; windowsHide: true },
	) => { unref(): void; on(event: "error", cb: (e: Error) => void): unknown };
	/** Electron's `shell.openExternal`, for the editor's URL scheme. */
	openExternal: (url: string) => Promise<void>;
}

export async function launchEditor(
	l: EditorLaunch,
	deps: LaunchDeps,
): Promise<void> {
	if (l.kind === "url") {
		await deps.openExternal(l.url);
		return;
	}
	const child = deps.spawn(l.file, l.args, {
		shell: false,
		detached: true,
		stdio: "ignore",
		windowsHide: true,
	});
	child.on("error", (e) => console.warn("editor did not start:", e.message));
	child.unref();
}

/** The stored choice, VS Code when there is none or it is malformed. */
export function readEditor(read: () => unknown): EditorChoice {
	const v = read();
	if (!v || typeof v !== "object") return { editor: "vscode" };
	const { editor, command } = v as { editor?: unknown; command?: unknown };
	if (editor === "command")
		return isLocalPath(command) ? { editor, command } : { editor: "vscode" };
	return editor === "zed" || editor === "cursor" || editor === "vscode"
		? { editor }
		: { editor: "vscode" };
}

/** What the renderer may see of the choice: the program's file name, not its path. */
export function editorView(c: EditorChoice): StudioEditor {
	if (c.editor !== "command" || !c.command) return { editor: c.editor };
	const name = c.command.split(/[\\/]/).pop() ?? "";
	return { editor: "command", commandName: name };
}
