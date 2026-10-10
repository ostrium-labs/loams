import { createServer } from "node:http";

const argv = process.argv.slice(2);
const i = argv.indexOf("--listen");
const [host, port] = (i >= 0 ? argv[i + 1] : "127.0.0.1:0").split(":");
const mode = process.env.FAKE_ENGINE_MODE ?? "ok";

if (mode === "crash") process.exit(3);

const srv = createServer((req, res) => {
	req.resume();
	if (
		req.method === "POST" &&
		req.url?.endsWith("InstanceService/GetInstance")
	) {
		res.writeHead(200, { "content-type": "application/json" });
		res.end("{}");
	} else {
		res.writeHead(404).end();
	}
});
const start = () => srv.listen(Number(port), host);
if (mode === "slow") setTimeout(start, 1500);
else start();
process.on("SIGTERM", () => process.exit(0));
