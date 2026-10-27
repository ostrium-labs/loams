// Adapted from dataelement/dsh-desktop (MIT), src/main/context-menu-template.ts.
// Loams changes (CF1 Task 14, D1084): English labels only (no locale), and Look Up for a
// selection on macOS, which takes the platform as the second argument.
import type { MenuItemConstructorOptions } from "electron";

export interface ContextMenuState {
	isEditable: boolean;
	selectionText: string;
	linkURL: string;
	hasImageContents: boolean;
	editFlags: {
		canUndo: boolean;
		canRedo: boolean;
		canCut: boolean;
		canCopy: boolean;
		canPaste: boolean;
		canSelectAll: boolean;
	};
}

export interface ContextMenuActions {
	openLink: (url: string) => void;
	copyLink: (url: string) => void;
	copyImage: () => void;
	lookUp: () => void;
}

const labels = {
	openLink: "Open Link in Browser",
	copyLink: "Copy Link Address",
	copyImage: "Copy Image",
	lookUp: (text: string) => `Look Up “${text}”`,
	undo: "Undo",
	redo: "Redo",
	cut: "Cut",
	copy: "Copy",
	paste: "Paste",
	selectAll: "Select All",
};

/** Look Up names at most this many characters of the selection. */
const LOOK_UP_MAX = 40;

export function isExternalWebUrl(rawUrl: string): boolean {
	try {
		const url = new URL(rawUrl);
		return (
			(url.protocol === "https:" || url.protocol === "http:") &&
			url.hostname !== "127.0.0.1" &&
			url.hostname !== "localhost"
		);
	} catch {
		return false;
	}
}

function appendSection(
	template: MenuItemConstructorOptions[],
	section: MenuItemConstructorOptions[],
): void {
	if (section.length === 0) return;
	if (template.length > 0) template.push({ type: "separator" });
	template.push(...section);
}

export function buildContextMenuTemplate(
	state: ContextMenuState,
	platform: NodeJS.Platform,
	actions: ContextMenuActions,
): MenuItemConstructorOptions[] {
	const template: MenuItemConstructorOptions[] = [];
	const selection = state.selectionText.trim();
	const hasSelection = selection.length > 0;

	if (platform === "darwin" && hasSelection) {
		const shown =
			selection.length > LOOK_UP_MAX ? `${selection.slice(0, LOOK_UP_MAX)}…` : selection;
		appendSection(template, [{ label: labels.lookUp(shown), click: actions.lookUp }]);
	}

	if (state.linkURL) {
		const linkItems: MenuItemConstructorOptions[] = [];
		if (isExternalWebUrl(state.linkURL)) {
			linkItems.push({
				label: labels.openLink,
				click: () => actions.openLink(state.linkURL),
			});
		}
		linkItems.push({
			label: labels.copyLink,
			click: () => actions.copyLink(state.linkURL),
		});
		appendSection(template, linkItems);
	}

	if (state.hasImageContents) {
		appendSection(template, [{ label: labels.copyImage, click: actions.copyImage }]);
	}

	if (state.isEditable) {
		appendSection(template, [
			{ label: labels.undo, role: "undo", enabled: state.editFlags.canUndo },
			{ label: labels.redo, role: "redo", enabled: state.editFlags.canRedo },
			{ type: "separator" },
			{ label: labels.cut, role: "cut", enabled: state.editFlags.canCut },
			{
				label: labels.copy,
				role: "copy",
				enabled: state.editFlags.canCopy || hasSelection,
			},
			{ label: labels.paste, role: "paste", enabled: state.editFlags.canPaste },
			{ type: "separator" },
			{ label: labels.selectAll, role: "selectAll", enabled: state.editFlags.canSelectAll },
		]);
	} else {
		const contentItems: MenuItemConstructorOptions[] = [];
		if (hasSelection) contentItems.push({ label: labels.copy, role: "copy" });
		contentItems.push({ label: labels.selectAll, role: "selectAll" });
		appendSection(template, contentItems);
	}

	return template;
}
