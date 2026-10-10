// electron-builder custom Windows signer, wired only when WINDOWS_SIGN_KEYSTORE is set
// (see electron-builder.config.cjs). Calls Jsign (>= 4): https://ebourg.github.io/jsign/
// Env: WINDOWS_SIGN_KEYSTORE, one of WINDOWS_SIGN_STOREPASS or WINDOWS_SIGN_STOREPASS_FILE (required),
// WINDOWS_SIGN_ALIAS (optional), WINDOWS_SIGN_STORETYPE (optional, default PKCS12),
// WINDOWS_SIGN_TSA (optional).
// The password never appears on the command line: Jsign reads `env:NAME` / `file:PATH` itself.
// SignPath submission of release artifacts is a separate release step (desktop-sign.yml), not this hook.
const { execFileSync } = require("node:child_process");

/** Returns the Jsign --storepass reference for the environment, or throws a clear error. */
function storepassRef(e) {
	if (e.WINDOWS_SIGN_STOREPASS_FILE) return `file:${e.WINDOWS_SIGN_STOREPASS_FILE}`;
	if (e.WINDOWS_SIGN_STOREPASS) return "env:WINDOWS_SIGN_STOREPASS";
	throw new Error(
		"Windows signing: WINDOWS_SIGN_KEYSTORE is set but neither WINDOWS_SIGN_STOREPASS nor WINDOWS_SIGN_STOREPASS_FILE is; refusing to sign with an empty keystore password",
	);
}

/** Builds the Jsign argv (exported for tests). */
function jsignArgs(e, file) {
	const args = [
		"--keystore", e.WINDOWS_SIGN_KEYSTORE,
		"--storetype", e.WINDOWS_SIGN_STORETYPE || "PKCS12",
		"--storepass", storepassRef(e),
		"--tsaurl", e.WINDOWS_SIGN_TSA || "http://timestamp.digicert.com",
	];
	if (e.WINDOWS_SIGN_ALIAS) args.push("--alias", e.WINDOWS_SIGN_ALIAS);
	args.push(file);
	return args;
}

exports.default = async function sign(configuration) {
	execFileSync("jsign", jsignArgs(process.env, configuration.path), { stdio: "inherit", env: process.env });
};
exports.jsignArgs = jsignArgs;
