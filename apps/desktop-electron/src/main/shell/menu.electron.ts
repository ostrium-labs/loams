import { Menu, type MenuItemConstructorOptions } from "electron";
import { type MenuNode, menuModel } from "./menu-model";

export function installAppMenu(o: {
	dev: boolean;
	onAction: (id: string) => void;
}): void {
	const map = (n: MenuNode): MenuItemConstructorOptions => {
		const out: Record<string, unknown> = {};
		if (n.type) out.type = n.type;
		if (n.label) out.label = n.label;
		if (n.role) out.role = n.role;
		if (n.accelerator) out.accelerator = n.accelerator;
		if (n.registerAccelerator !== undefined)
			out.registerAccelerator = n.registerAccelerator;
		if (n.submenu) out.submenu = n.submenu.map(map);
		const id = n.id;
		if (id) {
			// The id also lets Menu.getMenuItemById find the item (the e2e clicks it).
			out.id = id;
			out.click = () => o.onAction(id);
		}
		return out as MenuItemConstructorOptions;
	};
	const tpl = menuModel({ platform: process.platform, dev: o.dev });
	Menu.setApplicationMenu(Menu.buildFromTemplate(tpl.map(map)));
	console.log(`app menu installed: ${tpl.map((m) => m.label).join(", ")}`);
}
