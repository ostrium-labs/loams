// Builds the bench app per target: dist/aws.zip (Lambda), dist/azure.zip (Flex
// Consumption). Cloud Run's image is built by Containerfile.cloudrun (Bun).
// Zips are made with fixed timestamps so the same source gives the same hash.
import { build } from "esbuild";
import { execFileSync } from "node:child_process";
import { copyFileSync, mkdirSync, rmSync, writeFileSync, utimesSync, readdirSync } from "node:fs";
import { join } from "node:path";

const targets = process.argv.slice(2).length ? process.argv.slice(2) : ["aws", "azure"];
const common = { bundle: true, platform: "node", format: "esm", target: "node22", minify: false, legalComments: "none", logLevel: "warning" };
// The ESM bundle may pull in CommonJS that calls require(); give it one.
const banner = { js: "import { createRequire as __cr } from 'node:module'; const require = __cr(import.meta.url);" };

function zipDir(dir, out) {
  const epoch = new Date("2026-01-01T00:00:00Z");
  for (const f of readdirSync(dir)) utimesSync(join(dir, f), epoch, epoch);
  rmSync(out, { force: true });
  execFileSync("zip", ["-X", "-q", "-r", out, "."], { cwd: dir });
}

mkdirSync("dist", { recursive: true });
for (const t of targets) {
  const dir = join("dist", t);
  rmSync(dir, { recursive: true, force: true });
  mkdirSync(dir, { recursive: true });
  if (t === "aws") {
    // The Lambda Node runtime provides AWS SDK v3.
    await build({ ...common, entryPoints: ["src/shims/lambda.mjs"], outfile: join(dir, "index.mjs"), external: ["@aws-sdk/*"], banner });
  } else if (t === "azure") {
    // The Functions Node worker provides @azure/functions-core at run time.
    await build({ ...common, entryPoints: ["src/shims/azure.mjs"], outfile: join(dir, "index.mjs"), external: ["@azure/functions-core"], banner });
    copyFileSync("azure/host.json", join(dir, "host.json"));
    writeFileSync(join(dir, "package.json"), JSON.stringify({ name: "loams-cloud-bench-app", type: "module", main: "index.mjs" }, null, 2) + "\n");
  } else {
    throw new Error(`unknown target ${t} (aws, azure; Cloud Run uses Containerfile.cloudrun)`);
  }
  zipDir(dir, join(process.cwd(), "dist", `${t}.zip`));
  console.log(`dist/${t}.zip`);
}
