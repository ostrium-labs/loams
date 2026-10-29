// The TOML subset the policy and config files use: comments, `key = "string"`,
// `key = 123`, `key = true`, `key = ["a", "b"]` (across lines, trailing comma),
// `[table]` and `[[array-of-tables]]`. Anything else is an error, so a file
// that drifts past the subset fails its test instead of being half-read.

export type TomlValue = string | number | boolean | string[];
export type TomlTable = { [key: string]: TomlValue | TomlTable | TomlTable[] };

function stripComment(line: string): string {
  let inStr = false;
  for (let i = 0; i < line.length; i++) {
    const c = line[i];
    if (c === '"' && line[i - 1] !== "\\") inStr = !inStr;
    if (c === "#" && !inStr) return line.slice(0, i);
  }
  return line;
}

function parseString(s: string, where: string): string {
  const m = s.match(/^"((?:[^"\\]|\\.)*)"$/);
  if (!m) throw new Error(`${where}: expected a "string", got ${s}`);
  return m[1].replace(/\\(["\\nt])/g, (_, c) => (c === "n" ? "\n" : c === "t" ? "\t" : c));
}

function parseValue(s: string, where: string): TomlValue {
  if (s.startsWith("[")) {
    const inner = s.slice(1, -1).trim();
    if (!s.endsWith("]")) throw new Error(`${where}: unterminated array`);
    if (inner === "") return [];
    const items: string[] = [];
    for (const m of inner.matchAll(/"((?:[^"\\]|\\.)*)"\s*(,|$)/g)) items.push(parseString(`"${m[1]}"`, where));
    const rebuilt = inner.replace(/"((?:[^"\\]|\\.)*)"\s*(,|$)/g, "").trim();
    if (rebuilt !== "") throw new Error(`${where}: only arrays of strings are supported`);
    return items;
  }
  if (s === "true" || s === "false") return s === "true";
  if (/^-?\d+$/.test(s)) return Number(s);
  return parseString(s, where);
}

export function parseToml(text: string, name = "toml"): TomlTable {
  const root: TomlTable = {};
  let cur: TomlTable = root;
  const lines = text.split("\n");
  for (let i = 0; i < lines.length; i++) {
    const where = `${name}:${i + 1}`;
    let line = stripComment(lines[i]).trim();
    if (!line) continue;
    let m = line.match(/^\[\[([A-Za-z0-9_-]+)\]\]$/);
    if (m) {
      const arr = (root[m[1]] ??= []) as TomlTable[];
      if (!Array.isArray(arr)) throw new Error(`${where}: ${m[1]} is not an array of tables`);
      cur = {};
      arr.push(cur);
      continue;
    }
    m = line.match(/^\[([A-Za-z0-9_-]+)\]$/);
    if (m) {
      if (m[1] in root) throw new Error(`${where}: duplicate table ${m[1]}`);
      cur = root[m[1]] = {};
      continue;
    }
    m = line.match(/^([A-Za-z0-9_-]+)\s*=\s*(.*)$/);
    if (!m) throw new Error(`${where}: not a key = value line`);
    let value = m[2].trim();
    if (value.startsWith("[") && !value.endsWith("]")) {
      while (++i < lines.length) {
        value += " " + stripComment(lines[i]).trim();
        if (value.trimEnd().endsWith("]")) break;
      }
    }
    if (m[1] in cur) throw new Error(`${where}: duplicate key ${m[1]}`);
    cur[m[1]] = parseValue(value.trim(), where);
  }
  return root;
}
