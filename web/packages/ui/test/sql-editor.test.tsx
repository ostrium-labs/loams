import { act, cleanup, fireEvent, render, screen } from '@testing-library/react';
import { afterEach, describe, expect, it, vi } from 'vite-plus/test';
import {
  callAt,
  cleanSqlError,
  codeEditorText,
  formatSql,
  pushHistory,
  readHistory,
  SqlEditor,
  SqlWorkbench,
  setCodeEditorText,
  signatureText,
  sqlErrorPosition,
  sqlExamples,
  statementAt,
  tablesInScope,
  toCsv,
  toJsonRows,
} from '../src/index';

afterEach(cleanup);

// jsdom has no layout: the code editor measures text ranges for its selection and tooltips.
Range.prototype.getClientRects ??= () => [] as unknown as DOMRectList;
Range.prototype.getBoundingClientRect ??= () => new DOMRect();

describe('formatSql', () => {
  it('puts clauses on their own lines and upper-cases keywords', () => {
    expect(
      formatSql("select a, b from t where a = 1 and b like 'x%' order by a desc limit 5"),
    ).toBe(
      [
        'SELECT',
        '  a,',
        '  b',
        'FROM t',
        'WHERE a = 1',
        "  AND b LIKE 'x%'",
        'ORDER BY',
        '  a DESC',
        'LIMIT 5',
      ].join('\n'),
    );
  });

  it('keeps strings, quoted names, function calls and BETWEEN … AND intact', () => {
    const out = formatSql(
      'select "Group By", count(*) from t where x between 1 and 2 and y = \'from where\'',
    );
    expect(out).toContain('"Group By"');
    expect(out).toContain('count(*)');
    expect(out).toContain('x BETWEEN 1 AND 2');
    expect(out).toContain("'from where'");
  });

  it('indents a sub-query and keeps table functions on one line', () => {
    const out = formatSql(
      "select * from text_search('kb', 'refund', 'body', 10) where _id in (select _id from kb)",
    );
    expect(out).toContain("FROM text_search('kb', 'refund', 'body', 10)");
    expect(out).toMatch(/IN \(\n {2}SELECT/);
  });
});

describe('sqlErrorPosition', () => {
  it('reads DataFusion, ClickHouse, PostgreSQL and MySQL positions', () => {
    const df =
      'invalid argument: sql: SQL error: ParserError("Expected: an SQL statement, found: SELEC at Line: 1, Column: 1")';
    expect(sqlErrorPosition(df, 'SELEC * FROM t')).toEqual({ line: 1, column: 1, length: 5 });
    expect(cleanSqlError(df)).toBe(
      'Expected: an SQL statement, found: SELEC at Line: 1, Column: 1',
    );
    expect(
      sqlErrorPosition('Syntax error: failed at position 10 (FRM)', 'SELECT 1\nFRM t'),
    ).toEqual({ line: 2, column: 1, length: 3 });
    expect(sqlErrorPosition('syntax error at character 8', 'SELECT xx FROM t')?.column).toBe(8);
    expect(
      sqlErrorPosition(
        'You have an error in your SQL syntax; near "FRM t" at line 2',
        'SELECT 1\nFRM t',
      ),
    ).toEqual({ line: 2, column: 1, length: 3 });
    expect(
      sqlErrorPosition('Schema error: No field named nope.', 'SELECT nope FROM t')?.column,
    ).toBe(8);
    expect(sqlErrorPosition('boom', 'SELECT 1')).toBeUndefined();
  });
});

describe('statements and calls', () => {
  it('finds the statement under the cursor', () => {
    const text = "SELECT 1;\nSELECT ';' AS s;\nSELECT 3";
    expect(statementAt(text, 3)).toBe('SELECT 1');
    expect(statementAt(text, 14)).toBe("SELECT ';' AS s");
    expect(statementAt(text, text.length)).toBe('SELECT 3');
  });

  it('finds the call and argument under the cursor, past strings and arrays', () => {
    const text = "SELECT * FROM vector_search('kb', [1, 2, 3], 'emb";
    expect(callAt(text, text.length)).toMatchObject({ name: 'vector_search', arg: 2 });
    expect(callAt('SELECT count(', 13)).toMatchObject({ name: 'count', arg: 0 });
    expect(callAt('SELECT 1', 8)).toBeUndefined();
  });

  it('lists the tables a statement reads', () => {
    expect(tablesInScope('SELECT * FROM collections.dbpedia d JOIN gh_events ON 1=1')).toEqual([
      { name: 'dbpedia', alias: 'd' },
      { name: 'gh_events', alias: undefined },
    ]);
    expect(tablesInScope("SELECT 1 FROM text_search('kb', 'x')")[0]).toEqual({ name: 'kb' });
  });

  it('writes a signature with optional parameters', () => {
    expect(signatureText({ name: 'f', params: ['a', 'b?', 'c?'] })).toBe('f(a [, b [, c]])');
  });
});

describe('examples and export', () => {
  it('builds examples from the schema', () => {
    const ex = sqlExamples(
      {
        tables: [
          {
            name: 'dbpedia',
            columns: [
              { name: '_id', type: 'Utf8' },
              { name: 'title', type: 'Utf8' },
              { name: 'body', type: 'Utf8' },
              { name: 'letter', type: 'Utf8' },
              { name: 'body_chars', type: 'Int64' },
            ],
          },
        ],
      },
      'datafusion',
    );
    expect(ex.map((e) => e.label)).toEqual([
      'Rows of dbpedia',
      'Count dbpedia',
      'dbpedia by letter',
      'Full-text search dbpedia',
    ]);
    expect(ex[3]?.sql).toContain("text_search('dbpedia', 'search terms', 'body', 10)");
    expect(sqlExamples({ tables: [] }, 'mysql')[0]?.sql).toBe('SHOW TABLES;');
  });

  it('copies as CSV and JSON', () => {
    const cols = [{ name: 'a' }, { name: 'b' }];
    expect(
      toCsv(cols, [
        [1, 'x,y'],
        [null, 'q"'],
      ]),
    ).toBe('a,b\n1,"x,y"\n,"q"""');
    expect(JSON.parse(toJsonRows(cols, [[1, null]]))).toEqual([{ a: 1, b: null }]);
  });

  it('keeps history per scope, newest first, without duplicates', () => {
    pushHistory('t1', { text: 'SELECT 1', at: 1, ok: true });
    pushHistory('t1', { text: 'SELECT 2', at: 2, ok: true });
    pushHistory('t1', { text: 'SELECT 1 ', at: 3, ok: false });
    expect(readHistory('t1').map((h) => h.text)).toEqual(['SELECT 1', 'SELECT 2']);
    expect(readHistory('t2')).toEqual([]);
  });
});

describe('SqlEditor', () => {
  it('is a labelled textbox that reports edits and runs on Ctrl+Enter', () => {
    const onChange = vi.fn();
    const onRun = vi.fn();
    render(<SqlEditor label="SQL" value="SELECT 1" onChange={onChange} onRun={onRun} />);
    const box = screen.getByLabelText('SQL');
    expect(box.getAttribute('role')).toBe('textbox');
    expect(codeEditorText(box)).toBe('SELECT 1');
    act(() => setCodeEditorText(box, 'SELECT 2'));
    expect(onChange).toHaveBeenLastCalledWith('SELECT 2');
    fireEvent.keyDown(box, { key: 'Enter', ctrlKey: true });
    expect(onRun).toHaveBeenCalledWith('SELECT 2', false);
  });

  it('highlights keywords with token classes', () => {
    const { container } = render(
      <SqlEditor label="SQL" dialect="datafusion" value="SELECT 'a' FROM t" onChange={() => {}} />,
    );
    expect(container.querySelector('.loams-tok-keyword')?.textContent).toBe('SELECT');
    expect(container.querySelector('.loams-tok-string')?.textContent).toBe("'a'");
  });
});

describe('SqlWorkbench', () => {
  it('runs, shows the rows and the error with its position', async () => {
    const run = vi
      .fn()
      .mockResolvedValueOnce({ columns: [{ name: 'n', type: 'Int64' }], rows: [[3]] })
      .mockRejectedValueOnce(new Error('ParserError("found: SELEC at Line: 1, Column: 1")'));
    render(<SqlWorkbench dialect="datafusion" run={run} historyScope="wb" initial="SELECT 3" />);
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    expect(await screen.findByText('1 row')).toBeTruthy();
    expect(screen.getByText('Int64')).toBeTruthy();
    act(() => setCodeEditorText(screen.getByLabelText('SQL'), 'SELEC 1'));
    fireEvent.click(screen.getByRole('button', { name: 'Run' }));
    const alert = await screen.findByRole('alert');
    expect(alert.textContent).toContain('line 1, column 1');
    expect(screen.getByTestId('error-span').textContent).toContain('SELEC');
  });

  it('shows examples before the first run', () => {
    render(
      <SqlWorkbench
        dialect="clickhouse"
        run={vi.fn()}
        historyScope="wb2"
        schema={{ tables: [{ name: 'nyc', columns: [{ name: 'fare', type: 'Float64' }] }] }}
      />,
    );
    expect(screen.getByRole('button', { name: /Rows of nyc/ })).toBeTruthy();
  });
});
