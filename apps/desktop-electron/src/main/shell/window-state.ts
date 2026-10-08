// Adapted from dataelement/dsh-desktop (MIT), src/main/state/window-state.ts.
import { mkdirSync, readFileSync, renameSync, writeFileSync } from "node:fs";
import { dirname } from "node:path";

export interface WindowState {
	width: number;
	height: number;
	x?: number;
	y?: number;
	isMaximized: boolean;
}
export interface Rect {
	x: number;
	y: number;
	width: number;
	height: number;
}

export const DEFAULT_STATE: WindowState = {
	width: 1280,
	height: 800,
	isMaximized: false,
};
export const MIN_WIDTH = 900;
export const MIN_HEIGHT = 600;

/** Keeps size sane and drops the position unless the window is visibly on a display. */
export function clampToDisplays(s: WindowState, displays: Rect[]): WindowState {
	const width = Math.max(MIN_WIDTH, s.width);
	const height = Math.max(MIN_HEIGHT, s.height);
	const out: WindowState = { width, height, isMaximized: s.isMaximized };
	if (s.x === undefined || s.y === undefined) return out;
	const { x, y } = s;
	const visible = displays.some(
		(d) =>
			x >= d.x - 50 &&
			y >= d.y - 50 &&
			x + 100 <= d.x + d.width &&
			y + 100 <= d.y + d.height,
	);
	if (visible) {
		out.x = x;
		out.y = y;
	}
	return out;
}

const num = (v: unknown): v is number =>
	typeof v === "number" && Number.isFinite(v);

export function loadWindowState(file: string, displays: Rect[]): WindowState {
	try {
		const p = JSON.parse(readFileSync(file, "utf8")) as Record<string, unknown>;
		return clampToDisplays(
			{
				width: num(p.width) ? p.width : DEFAULT_STATE.width,
				height: num(p.height) ? p.height : DEFAULT_STATE.height,
				x: num(p.x) ? p.x : undefined,
				y: num(p.y) ? p.y : undefined,
				isMaximized: p.isMaximized === true,
			},
			displays,
		);
	} catch {
		return { ...DEFAULT_STATE };
	}
}

export function saveWindowState(file: string, state: WindowState): void {
	try {
		mkdirSync(dirname(file), { recursive: true });
		const tmp = `${file}.tmp`;
		writeFileSync(tmp, JSON.stringify(state), "utf8");
		renameSync(tmp, file);
	} catch (err) {
		console.warn("[window-state] failed to save", err);
	}
}
