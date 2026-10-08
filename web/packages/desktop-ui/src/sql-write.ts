/** Words that change data or schema; a statement containing one needs a confirm. */
const WRITE_WORDS =
  /\b(insert|update|delete|merge|create|drop|alter|truncate|into|for\s+update|for\s+share)\b/i;

function stripNoise(sql: string): string {
  return sql
    .replace(/'(?:[^']|'')*'/g, "''")
    .replace(/"(?:[^"]|"")*"/g, '""')
    .replace(/--[^\n]*/g, ' ')
    .replace(/\/\*[\s\S]*?\*\//g, ' ');
}

/**
 * True unless every statement is a plain SELECT, SHOW, EXPLAIN or WITH-SELECT.
 * Conservative: a false positive only costs one extra click.
 */
export function isWrite(sql: string): boolean {
  const statements = stripNoise(sql)
    .split(';')
    .map((s) => s.trim())
    .filter(Boolean);
  if (statements.length === 0) return false;
  return statements.some((s) => {
    const head = (/^\(*\s*([a-z]+)/i.exec(s)?.[1] ?? '').toLowerCase();
    if (
      !['select', 'show', 'explain', 'with', 'values', 'table', 'describe', 'desc'].includes(head)
    )
      return true;
    if (head === 'show' || head === 'describe' || head === 'desc') return false;
    return WRITE_WORDS.test(s);
  });
}
