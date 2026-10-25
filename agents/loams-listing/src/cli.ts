// loams-listing: lint <listing.json>… | from-package <dir> | index build <dir> --out <dir>
import { readFileSync, writeFileSync } from 'node:fs';
import { resolve } from 'node:path';
import { fromPackageDir, PackageError } from './from-package.js';
import { buildIndex, canonical, IndexError, writeIndex } from './index-build.js';
import { formatIssue, hasErrors, lint, RULES } from './lint.js';

export interface Io {
  stdout: (line: string) => void;
  stderr: (line: string) => void;
  cwd: string;
}

const USAGE = `usage:
  loams-listing lint [--first-party-only] <listing.json>…
  loams-listing from-package <dir> (--tarball <file> | --integrity <sha512-…>) [--commit <sha>] [--out <file>]
  loams-listing index build <listings-dir> --out <dir> [--first-party-only] [--source-commit <sha>]
  loams-listing rules`;

interface Parsed {
  positional: string[];
  flags: Map<string, string | true>;
}

const VALUE_FLAGS = new Set(['--tarball', '--integrity', '--commit', '--out', '--source-commit']);

function parseArgs(args: string[]): Parsed {
  const positional: string[] = [];
  const flags = new Map<string, string | true>();
  for (let i = 0; i < args.length; i++) {
    const arg = args[i]!;
    if (VALUE_FLAGS.has(arg)) {
      const value = args[++i];
      if (value === undefined) throw new UsageError(`${arg} needs a value`);
      flags.set(arg, value);
    } else if (arg.startsWith('--')) {
      flags.set(arg, true);
    } else {
      positional.push(arg);
    }
  }
  return { positional, flags };
}

class UsageError extends Error {}

function str(flags: Parsed['flags'], name: string): string | undefined {
  const value = flags.get(name);
  return typeof value === 'string' ? value : undefined;
}

export async function main(args: string[], io: Io): Promise<number> {
  try {
    const [command, ...rest] = args;
    const { positional, flags } = parseArgs(rest);
    const at = (path: string) => resolve(io.cwd, path);
    switch (command) {
      case 'lint': {
        if (positional.length === 0) throw new UsageError('lint needs at least one listing.json');
        let failed = false;
        for (const file of positional) {
          let doc: unknown;
          try {
            doc = JSON.parse(readFileSync(at(file), 'utf8'));
          } catch (error) {
            io.stderr(`${file}: error [schema] not readable JSON: ${(error as Error).message}`);
            failed = true;
            continue;
          }
          const issues = lint(doc, { firstPartyOnly: flags.has('--first-party-only') });
          for (const issue of issues) io.stderr(formatIssue(file, issue));
          if (hasErrors(issues)) failed = true;
          else io.stdout(`${file}: ok`);
        }
        return failed ? 1 : 0;
      }
      case 'from-package': {
        const [dir] = positional;
        if (!dir) throw new UsageError('from-package needs a package directory');
        const tarball = str(flags, '--tarball');
        const listing = fromPackageDir(at(dir), {
          commit: str(flags, '--commit'),
          integrity: str(flags, '--integrity'),
          tarball: tarball ? at(tarball) : undefined,
        });
        const issues = lint(listing);
        for (const issue of issues) io.stderr(formatIssue('listing.json', issue));
        const text = `${JSON.stringify(listing, null, 2)}\n`;
        const out = str(flags, '--out');
        if (out) writeFileSync(at(out), text);
        else io.stdout(text.trimEnd());
        return hasErrors(issues) ? 1 : 0;
      }
      case 'index': {
        const [sub, dir] = positional;
        if (sub !== 'build' || !dir) throw new UsageError('index build needs a listings directory');
        const out = str(flags, '--out');
        if (!out) throw new UsageError('index build needs --out <dir>');
        const result = buildIndex(at(dir), {
          firstPartyOnly: flags.has('--first-party-only'),
          sourceCommit: str(flags, '--source-commit'),
        });
        for (const warning of result.warnings) io.stderr(warning);
        for (const file of writeIndex(result, at(out))) io.stdout(`wrote ${file}`);
        return 0;
      }
      case 'rules': {
        for (const [rule, text] of Object.entries(RULES)) io.stdout(`${rule}\t${text}`);
        return 0;
      }
      default:
        throw new UsageError(command ? `unknown command ${command}` : 'no command');
    }
  } catch (error) {
    if (error instanceof UsageError) {
      io.stderr(`${error.message}\n${USAGE}`);
      return 2;
    }
    if (error instanceof IndexError) {
      for (const problem of error.problems) io.stderr(problem);
      return 1;
    }
    if (error instanceof PackageError) {
      io.stderr(error.message);
      return 1;
    }
    throw error;
  }
}

export { canonical };
