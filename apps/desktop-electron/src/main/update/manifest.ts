import * as ed from "@noble/ed25519";

const HEX32 = /^[0-9a-fA-F]{64}$/;
const B64 = /^[A-Za-z0-9+/]+={0,2}$/;

/**
 * Ed25519 check of a `latest*.yml` against its detached base64 signature.
 * Never throws: anything malformed (empty signature, bad key) is "not verified".
 */
export async function verifyManifest(
	yml: Uint8Array,
	sigB64: string,
	pubkeyHex: string,
): Promise<boolean> {
	try {
		const sigText = sigB64.trim();
		if (!HEX32.test(pubkeyHex) || !B64.test(sigText)) return false;
		const sig = Uint8Array.from(Buffer.from(sigText, "base64"));
		if (sig.length !== 64) return false;
		const pub = Uint8Array.from(Buffer.from(pubkeyHex, "hex"));
		return await ed.verifyAsync(sig, yml, pub, { zip215: false });
	} catch {
		return false;
	}
}
