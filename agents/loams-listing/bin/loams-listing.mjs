#!/usr/bin/env node
// The built CLI (`pnpm build` writes dist/). See src/cli.ts.
import { main } from '../dist/cli.js';

process.exitCode = await main(process.argv.slice(2), {
  stdout: (line) => process.stdout.write(`${line}\n`),
  stderr: (line) => process.stderr.write(`${line}\n`),
  cwd: process.cwd(),
});
