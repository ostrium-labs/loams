/** The renderer is told the app version through a `webPreferences.additionalArguments` flag. */
export const VERSION_ARG = "--loams-desktop-version=";

/** `--loams-desktop-version=1.2.3` from a process argv, or "0.0.0" when absent or empty. */
export function versionFromArgv(argv: readonly string[]): string {
	const hit = argv.find((a) => a.startsWith(VERSION_ARG));
	const v = hit?.slice(VERSION_ARG.length).trim();
	return v ? v : "0.0.0";
}
