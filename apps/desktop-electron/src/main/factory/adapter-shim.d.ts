// Typecheck-only stand-in for the raw-TypeScript workspace adapters (mapped in
// tsconfig.node.json `paths`). They are written for a looser tsconfig; the
// bundler resolves the real packages. Config shapes live in apps.ts.
type Ctor = new (
	// biome-ignore lint/suspicious/noExplicitAny: shim
	ctx: any,
	// biome-ignore lint/suspicious/noExplicitAny: shim
	config: any,
	// biome-ignore lint/suspicious/noExplicitAny: shim
) => Record<string, (...args: any[]) => Promise<any>>;
export declare const ForgejoAdapterService: Ctor;
export declare const ZulipAdapterService: Ctor;
export declare const ItsAPlanAdapterService: Ctor;
export declare const GlitchtipAdapterService: Ctor;
export declare const OpenPanelAdapterService: Ctor;
export declare const MatomoAdapterService: Ctor;
export declare const LangfuseAdapterService: Ctor;
