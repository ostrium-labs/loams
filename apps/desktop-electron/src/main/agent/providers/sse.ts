// A hand-written Server-Sent Events reader (no dependency): `event:` and `data:`
// fields, multi-line data, comments, CRLF or LF line ends.

export interface SseMessage {
	event: string;
	data: string;
}

export async function* readSse(
	body: ReadableStream<Uint8Array>,
): AsyncGenerator<SseMessage> {
	const reader = body.getReader();
	const decoder = new TextDecoder();
	let buf = "";
	let event = "";
	let data: string[] = [];
	let finished = false;
	const flush = (): SseMessage | undefined => {
		const msg =
			data.length > 0
				? { event: event || "message", data: data.join("\n") }
				: undefined;
		event = "";
		data = [];
		return msg;
	};
	try {
		for (;;) {
			const { done, value } = await reader.read();
			buf += done ? decoder.decode() : decoder.decode(value, { stream: true });
			let nl = buf.search(/\r\n|\r|\n/);
			while (nl >= 0) {
				// A "\r" at the end of a chunk may be the first half of "\r\n".
				if (!done && buf[nl] === "\r" && nl === buf.length - 1) break;
				const line = buf.slice(0, nl);
				const sep = buf[nl] === "\r" && buf[nl + 1] === "\n" ? 2 : 1;
				buf = buf.slice(nl + sep);
				if (line === "") {
					const msg = flush();
					if (msg) yield msg;
				} else if (!line.startsWith(":")) {
					const colon = line.indexOf(":");
					const field = colon < 0 ? line : line.slice(0, colon);
					let value = colon < 0 ? "" : line.slice(colon + 1);
					if (value.startsWith(" ")) value = value.slice(1);
					if (field === "event") event = value;
					else if (field === "data") data.push(value);
				}
				nl = buf.search(/\r\n|\r|\n/);
			}
			if (done) {
				// A last data line without a line end still counts.
				if (buf.length > 0 && !buf.startsWith(":")) {
					const colon = buf.indexOf(":");
					if (colon >= 0 && buf.slice(0, colon) === "data")
						data.push(buf.slice(colon + 1).replace(/^ /, ""));
				}
				finished = true;
				const msg = flush();
				if (msg) yield msg;
				return;
			}
		}
	} finally {
		// Stopped early (cancel, error): close the connection rather than leave it open.
		if (!finished) await reader.cancel().catch(() => undefined);
		else reader.releaseLock();
	}
}
