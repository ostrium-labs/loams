import type { LoamsDesktopApi } from "./contracts";

export type { LoamsDesktopApi };

declare global {
	interface Window {
		loamsDesktop?: LoamsDesktopApi;
	}
}
