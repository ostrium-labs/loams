// The `chat` IPC namespace (D675). Every handler checks the sender (D659).
import { join } from "node:path";
import { app, ipcMain, type Session } from "electron";
import { CH, type ChatEvent, type FactoryAppId } from "../../shared/contracts";
import type { ConnectorCatalog } from "../connectors/catalog";
import type { FactoryHost } from "../factory/host";
import { OPS } from "../factory/ops";
import type { Vault } from "../factory/vault";
import { assertTrustedSender } from "../security/policy";
import { getMainWindow } from "../shell/main-window";
import { builtinTools } from "./builtin-tools";
import { liveToolDefs } from "./live-tools";
import { ProviderConfigs } from "./providers/presets";
import { ChatService } from "./service";
import { ChatStore } from "./store";
import { registerTools, TOOLS } from "./tools";

export interface AgentDeps {
	session: Session;
	vault: Vault;
	factory: FactoryHost;
	connectors: ConnectorCatalog;
}

export function registerChatIpc(deps: AgentDeps): ChatService {
	const userData = app.getPath("userData");
	registerTools(
		builtinTools({
			// Through the console's own protocol handler: same routing, cookies and
			// local shim as the pages (active server; /durable/ to the engine).
			api: (path, init) =>
				deps.session.fetch(`loams-app://console${path}`, {
					...init,
					credentials: "include",
				}),
			connectors: () => deps.connectors.catalog(),
			factoryQuery: (q) => deps.factory.query(q),
			factoryOps: Object.fromEntries(
				Object.entries(OPS)
					.filter(([, ops]) => Object.keys(ops).some((o) => o !== "health"))
					.map(([id, ops]) => [
						id,
						Object.keys(ops).filter((o) => o !== "health"),
					]),
			) as Partial<Record<FactoryAppId, string[]>>,
			now: Date.now,
		}),
	);
	registerTools(
		liveToolDefs({
			fetch: (url, init) =>
				deps.session.fetch(String(url), { ...init, credentials: "include" }),
			liveUrl: "loams-app://console",
		}),
	);
	const service = new ChatService({
		store: new ChatStore(join(userData, "chats")),
		configs: new ProviderConfigs(
			join(userData, "agent", "providers.json"),
			deps.vault,
			(url, init) => fetch(url, init),
		),
		tools: TOOLS,
		emit: (e: ChatEvent) => {
			const w = getMainWindow();
			if (w && !w.isDestroyed()) w.webContents.send(CH.chatEvent, e);
		},
		now: Date.now,
		extraSecrets: () => deps.factory.allSecrets(),
	});

	ipcMain.handle(CH.chatProviders, (e) => {
		assertTrustedSender(e);
		return service.providers();
	});
	ipcMain.handle(CH.chatConfigureProvider, (e, id: unknown, cfg: unknown) => {
		assertTrustedSender(e);
		return service.configureProvider(id, cfg);
	});
	ipcMain.handle(CH.chatTestProvider, (e, id: unknown) => {
		assertTrustedSender(e);
		return service.testProvider(id);
	});
	ipcMain.handle(CH.chatList, (e) => {
		assertTrustedSender(e);
		return service.list();
	});
	ipcMain.handle(CH.chatGet, (e, id: unknown) => {
		assertTrustedSender(e);
		return service.get(id);
	});
	ipcMain.handle(CH.chatCreate, (e, opts: unknown) => {
		assertTrustedSender(e);
		return service.create(opts);
	});
	ipcMain.handle(
		CH.chatSend,
		(e, id: unknown, text: unknown, opts: unknown) => {
			assertTrustedSender(e);
			return service.send(id, text, opts);
		},
	);
	ipcMain.handle(CH.chatCancel, async (e, id: unknown) => {
		assertTrustedSender(e);
		await service.cancel(id);
	});
	ipcMain.handle(
		CH.chatApprove,
		(e, id: unknown, callId: unknown, decision: unknown) => {
			assertTrustedSender(e);
			return service.approve(id, callId, decision);
		},
	);
	ipcMain.handle(CH.chatRemove, (e, id: unknown) => {
		assertTrustedSender(e);
		return service.remove(id);
	});
	return service;
}
