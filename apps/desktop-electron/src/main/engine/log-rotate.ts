import {
	appendFileSync,
	existsSync,
	mkdirSync,
	renameSync,
	rmSync,
	statSync,
} from "node:fs";
import { dirname } from "node:path";

export class RotatingLog {
	private size = 0;

	constructor(
		private readonly file: string,
		private readonly maxBytes = 10 * 1024 * 1024,
		private readonly keep = 5,
	) {
		try {
			mkdirSync(dirname(file), { recursive: true });
			this.size = statSync(file).size;
		} catch {
			this.size = 0;
		}
	}

	write(chunk: string | Uint8Array): void {
		const len =
			typeof chunk === "string" ? Buffer.byteLength(chunk) : chunk.length;
		if (this.size > 0 && this.size + len > this.maxBytes) this.rotate();
		try {
			appendFileSync(this.file, chunk);
			this.size += len;
		} catch {
			/* logging must never crash the app */
		}
	}

	private rotate(): void {
		try {
			rmSync(`${this.file}.${this.keep}`, { force: true });
			for (let i = this.keep - 1; i >= 1; i--) {
				if (existsSync(`${this.file}.${i}`))
					renameSync(`${this.file}.${i}`, `${this.file}.${i + 1}`);
			}
			renameSync(this.file, `${this.file}.1`);
		} catch {
			/* best effort */
		}
		this.size = 0;
	}
}
