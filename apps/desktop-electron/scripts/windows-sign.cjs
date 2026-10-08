// electron-builder custom Windows signer, wired only when WINDOWS_SIGN_KEYSTORE is set
// (see electron-builder.config.cjs). Calls Jsign: https://ebourg.github.io/jsign/
// Env: WINDOWS_SIGN_KEYSTORE, WINDOWS_SIGN_STOREPASS, WINDOWS_SIGN_ALIAS (optional),
// WINDOWS_SIGN_STORETYPE (optional, default PKCS12), WINDOWS_SIGN_TSA (optional).
// SignPath submission of release artifacts is a separate release step, not this hook.
const { execFileSync } = require("node:child_process");

exports.default = async function sign(configuration) {
	const e = process.env;
	const args = [
		"--keystore", e.WINDOWS_SIGN_KEYSTORE,
		"--storetype", e.WINDOWS_SIGN_STORETYPE || "PKCS12",
		"--storepass", e.WINDOWS_SIGN_STOREPASS || "",
		"--tsaurl", e.WINDOWS_SIGN_TSA || "http://timestamp.digicert.com",
	];
	if (e.WINDOWS_SIGN_ALIAS) args.push("--alias", e.WINDOWS_SIGN_ALIAS);
	args.push(configuration.path);
	execFileSync("jsign", args, { stdio: "inherit" });
};
