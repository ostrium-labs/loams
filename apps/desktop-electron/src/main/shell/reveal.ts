// Pure: bring a possibly hidden or minimized window to the front.
export interface Revealable {
	isMinimized(): boolean;
	restore(): void;
	show(): void;
	focus(): void;
}

export function reveal(win: Revealable): void {
	if (win.isMinimized()) win.restore();
	win.show();
	win.focus();
}
