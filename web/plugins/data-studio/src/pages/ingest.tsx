import type { JsonObject, JsonValue } from '@bufbuild/protobuf';
import { Button, Card, Field, Input, Meter, Notice } from '@loams/ui';
import { useState } from 'react';
import { type DataClient, vectorFields } from '../client.js';
import { chunk, parseIngest } from '../ingest-parse.js';
import { errorText } from '../shared.js';
import { idFieldKey } from './collections.js';

export const BATCH_SIZE = 500;

export interface IngestOutcome {
  written: number;
  total: number;
  /** 1-based index of the batch that failed. */
  failedBatch?: number;
  error?: string;
}

/** Writes `docs` in batches of 500, stopping at the first error. */
export async function ingestDocuments(
  client: DataClient,
  ns: string,
  coll: string,
  docs: JsonObject[],
  idField: string,
  vectorNames: string[],
  onProgress: (written: number) => void,
): Promise<IngestOutcome> {
  const batches = chunk(docs, BATCH_SIZE);
  let written = 0;
  for (let b = 0; b < batches.length; b++) {
    try {
      const batch = (batches[b] ?? []).map((doc, i) => {
        const { [idField]: id, ...rest } = doc;
        if (id === undefined) {
          throw new Error(`Document ${b * BATCH_SIZE + i + 1} has no "${idField}" field.`);
        }
        const vectors: Record<string, JsonValue> = {};
        for (const name of vectorNames) {
          const v = rest[name];
          if (v !== undefined) vectors[name] = v;
          delete rest[name];
        }
        return { id, source: rest, vectors };
      });
      await client.write(ns, coll, batch, crypto.randomUUID());
      written += batch.length;
      onProgress(written);
    } catch (e) {
      return { written, total: docs.length, failedBatch: b + 1, error: errorText(e) };
    }
  }
  return { written, total: docs.length };
}

export function IngestTab({
  client,
  ns,
  coll,
  schema,
  onDone,
}: {
  client: DataClient;
  ns: string;
  coll: string;
  schema: JsonObject;
  onDone?: () => unknown;
}) {
  const stored = (() => {
    try {
      return localStorage.getItem(idFieldKey(ns, coll)) ?? 'id';
    } catch {
      return 'id';
    }
  })();
  const [idField, setIdField] = useState(stored);
  const [fileName, setFileName] = useState<string>();
  const [docs, setDocs] = useState<JsonObject[]>();
  const [parseError, setParseError] = useState<string>();
  const [progress, setProgress] = useState(0);
  const [outcome, setOutcome] = useState<IngestOutcome>();
  const [busy, setBusy] = useState(false);

  const pick = async (file: File | undefined) => {
    setOutcome(undefined);
    setProgress(0);
    setDocs(undefined);
    setParseError(undefined);
    if (!file) return;
    setFileName(file.name);
    try {
      setDocs(parseIngest(file.name, await file.text()));
    } catch (e) {
      setParseError(errorText(e));
    }
  };

  const start = async () => {
    if (!docs) return;
    setBusy(true);
    setOutcome(undefined);
    setProgress(0);
    const result = await ingestDocuments(
      client,
      ns,
      coll,
      docs,
      idField.trim() || 'id',
      vectorFields(schema).map((v) => v.name),
      setProgress,
    );
    setOutcome(result);
    setBusy(false);
    onDone?.();
  };

  return (
    <Card title="Ingest documents">
      <div className="ds-form">
        <Field
          label="File"
          hint="A .json file holding an array, or an .ndjson file with one object per line."
        >
          {(p) => (
            <input
              {...p}
              type="file"
              accept=".json,.ndjson,.jsonl"
              onChange={(e) => void pick(e.target.files?.[0])}
            />
          )}
        </Field>
        <Field label="ID field" hint="The field of each document that becomes its id.">
          {(p) => <Input {...p} value={idField} onChange={(e) => setIdField(e.target.value)} />}
        </Field>
        {parseError && (
          <Notice tone="danger" title="Could not read the file">
            {parseError}
          </Notice>
        )}
        {docs && (
          <p>
            <code>{fileName}</code>: {docs.length} documents in {chunk(docs, BATCH_SIZE).length}{' '}
            batches of up to {BATCH_SIZE}.
          </p>
        )}
        {(busy || outcome) && docs && (
          <Meter value={progress} max={Math.max(docs.length, 1)} label="Ingest progress" />
        )}
        {outcome?.error && (
          <Notice tone="danger" title={`Stopped at batch ${outcome.failedBatch}`}>
            <span>
              Batch {outcome.failedBatch} failed: {outcome.error} {outcome.written} of{' '}
              {outcome.total} documents were written.
            </span>
          </Notice>
        )}
        {outcome && !outcome.error && (
          <Notice tone="success" title="Ingest complete">
            Wrote {outcome.written} documents.
          </Notice>
        )}
        <div>
          <Button variant="primary" disabled={!docs || busy || docs.length === 0} onClick={start}>
            Start ingest
          </Button>
        </div>
      </div>
    </Card>
  );
}
