import type { JsonObject } from '@bufbuild/protobuf';
import {
  rememberNamespace as rememberShared,
  NamespacePicker as SharedPicker,
} from '@loams/desktop-ui';
import {
  Badge,
  Button,
  Card,
  Dialog,
  Empty,
  Field,
  Input,
  LoadingSwap,
  Select,
  SkeletonList,
  StatusTag,
  Table,
} from '@loams/ui';
import { type FormEvent, useState } from 'react';
import type { CollectionInfo, DataClient } from '../client.js';
import { ErrorNotice, errorText, PageHead, Tabs, useLoad } from '../shared.js';

const RECENT_KEY = 'loams.data.namespaces';

export function rememberNamespace(ns: string): void {
  rememberShared(RECENT_KEY, ns);
}

export function NamespacePicker({
  ns,
  navigate,
  href = (next) => `/data/${encodeURIComponent(next)}`,
}: {
  ns: string;
  navigate: (to: string) => void;
  /** Where another namespace opens; the Data explorer's own list by default. */
  href?: (ns: string) => string;
}) {
  return (
    <SharedPicker
      ns={ns}
      onOpen={(next) => navigate(href(next))}
      storageKey={RECENT_KEY}
      idPrefix="ds"
    />
  );
}

const FIELD_KINDS = ['text', 'keyword', 'i64', 'f64', 'bool', 'date', 'uuid', 'json'] as const;
type FieldKind = (typeof FIELD_KINDS)[number];
interface FieldDraft {
  name: string;
  kind: FieldKind;
}

/** The REST schema JSON for the form's fields and optional vector. */
export function buildSchema(
  fields: FieldDraft[],
  vector: { name: string; dim: number; distance: string } | undefined,
): JsonObject {
  return {
    fields: fields
      .filter((f) => f.name.trim())
      .map((f) => ({
        name: f.name.trim(),
        source_path: f.name.trim(),
        kind: f.kind === 'text' ? { text: { analyzer: 'standard', positions: true } } : f.kind,
        indexed: f.kind !== 'json',
        fast: f.kind !== 'text' && f.kind !== 'json',
      })),
    vectors: vector ? [{ name: vector.name, dim: vector.dim, distance: vector.distance }] : [],
    sparse_vectors: [],
    dynamic: 'ignore',
    max_fields: 1000,
  };
}

export function idFieldKey(ns: string, coll: string): string {
  return `loams.data.idfield.${ns}/${coll}`;
}

function NewCollection({
  client,
  ns,
  open,
  onClose,
  onCreated,
}: {
  client: DataClient;
  ns: string;
  open: boolean;
  onClose: () => void;
  onCreated: (name: string) => void;
}) {
  const [name, setName] = useState('');
  const [pk, setPk] = useState('id');
  const [fields, setFields] = useState<FieldDraft[]>([{ name: 'title', kind: 'text' }]);
  const [vecDim, setVecDim] = useState('');
  const [vecMetric, setVecMetric] = useState('cosine');
  const [error, setError] = useState<string>();
  const [busy, setBusy] = useState(false);

  const submit = async (e: FormEvent) => {
    e.preventDefault();
    setError(undefined);
    const dim = vecDim.trim() === '' ? undefined : Number(vecDim);
    if (dim !== undefined && (!Number.isInteger(dim) || dim < 1)) {
      setError('The vector dimension must be a positive whole number.');
      return;
    }
    setBusy(true);
    try {
      const schema = buildSchema(
        fields,
        dim === undefined ? undefined : { name: 'embedding', dim, distance: vecMetric },
      );
      await client.createCollection(ns, { name: name.trim(), schema });
      try {
        localStorage.setItem(idFieldKey(ns, name.trim()), pk.trim() || 'id');
      } catch {
        // storage is a convenience only
      }
      onCreated(name.trim());
    } catch (err) {
      setError(errorText(err));
    } finally {
      setBusy(false);
    }
  };

  return (
    <Dialog
      open={open}
      onClose={onClose}
      title="New collection"
      footer={
        <>
          <Button variant="quiet" onClick={onClose}>
            Cancel
          </Button>
          <Button
            variant="primary"
            type="submit"
            form="ds-new-collection"
            disabled={busy || !name.trim()}
          >
            Create collection
          </Button>
        </>
      }
    >
      <form id="ds-new-collection" className="ds-form" onSubmit={submit}>
        <Field label="Name">
          {(p) => (
            <Input
              {...p}
              value={name}
              onChange={(e) => setName(e.target.value)}
              placeholder="articles"
            />
          )}
        </Field>
        <Field
          label="Primary key field"
          hint="The field of each ingested document that holds its id."
        >
          {(p) => <Input {...p} value={pk} onChange={(e) => setPk(e.target.value)} />}
        </Field>
        <fieldset className="ds-fields">
          <legend>Fields</legend>
          {fields.map((f, i) => (
            // oxlint-disable-next-line react/no-array-index-key -- a positional draft list
            <div className="ds-field-row" key={i}>
              <Input
                aria-label={`Field ${i + 1} name`}
                value={f.name}
                onChange={(e) =>
                  setFields(fields.map((x, j) => (j === i ? { ...x, name: e.target.value } : x)))
                }
              />
              <Select
                aria-label={`Field ${i + 1} type`}
                value={f.kind}
                onChange={(e) =>
                  setFields(
                    fields.map((x, j) =>
                      j === i ? { ...x, kind: e.target.value as FieldKind } : x,
                    ),
                  )
                }
              >
                {FIELD_KINDS.map((k) => (
                  <option key={k}>{k}</option>
                ))}
              </Select>
              <Button
                variant="quiet"
                size="sm"
                aria-label={`Remove field ${i + 1}`}
                onClick={() => setFields(fields.filter((_, j) => j !== i))}
              >
                Remove
              </Button>
            </div>
          ))}
          <Button size="sm" onClick={() => setFields([...fields, { name: '', kind: 'keyword' }])}>
            Add field
          </Button>
        </fieldset>
        <div className="ds-field-row">
          <Field label="Vector dimension" hint="Leave empty for no vector.">
            {(p) => (
              <Input
                {...p}
                inputMode="numeric"
                value={vecDim}
                onChange={(e) => setVecDim(e.target.value)}
              />
            )}
          </Field>
          <Field label="Metric">
            {(p) => (
              <Select {...p} value={vecMetric} onChange={(e) => setVecMetric(e.target.value)}>
                <option>cosine</option>
                <option>dot</option>
                <option>euclid</option>
                <option>manhattan</option>
              </Select>
            )}
          </Field>
        </div>
        {error && <ErrorNotice title="Could not create the collection" message={error} />}
      </form>
    </Dialog>
  );
}

/** The Search entry's route (UX-2): the collections of a namespace, each opening its Search. */
export const SEARCH_PATH = '/search';

/**
 * `/data` and `/data/:ns`: the collections of one namespace. With `target="search"` it is
 * the Search entry (`/search`, `?ns=`), where a collection opens on its Search tab.
 */
export function CollectionsPage({
  client,
  ns,
  navigate,
  target = 'documents',
}: {
  client: DataClient;
  ns: string;
  navigate: (to: string) => void;
  target?: 'documents' | 'search';
}) {
  const search = target === 'search';
  const open = (name: string) =>
    `/data/${encodeURIComponent(ns)}/${encodeURIComponent(name)}${search ? '/search' : ''}`;
  const [list, reload] = useLoad(() => client.listCollections(ns), [client, ns]);
  const [creating, setCreating] = useState(false);
  const [nsError, setNsError] = useState<string>();

  const createNs = async () => {
    setNsError(undefined);
    try {
      await client.createNamespace(ns);
      rememberNamespace(ns);
      reload();
    } catch (e) {
      setNsError(errorText(e));
    }
  };
  if (list.state === 'ready') rememberNamespace(ns);

  return (
    <div className="lc-page ds-page">
      <PageHead
        title={search ? 'Search' : 'Data explorer'}
        subtitle={
          search
            ? 'Pick a collection to search its documents.'
            : 'Browse, query and load the collections of a namespace.'
        }
        actions={
          search ? undefined : (
            <>
              <Button onClick={createNs}>New namespace</Button>
              <Button variant="primary" onClick={() => setCreating(true)}>
                New collection
              </Button>
            </>
          )
        }
      />
      <div className="ds-toolbar">
        <NamespacePicker
          key={ns}
          ns={ns}
          navigate={navigate}
          href={search ? (next) => `${SEARCH_PATH}?ns=${encodeURIComponent(next)}` : undefined}
        />
        {!search && <Tabs ns={ns} active="documents" navigate={navigate} />}
      </div>
      {nsError && <ErrorNotice title="Could not create the namespace" message={nsError} />}
      <Card title={`Collections in ${ns}`} flush>
        {list.state === 'error' ? (
          <div className="ds-pad">
            <ErrorNotice title="Could not list collections" message={list.message} retry={reload} />
          </div>
        ) : (
          <LoadingSwap
            loading={list.state === 'loading'}
            skeleton={<SkeletonList className="ds-pad" label="Loading collections" rows={4} />}
          >
            {list.state === 'ready' && (
              <Table<CollectionInfo>
                caption="Collections"
                rows={list.data}
                rowKey={(c) => c.name}
                onRowClick={(c) => navigate(open(c.name))}
                empty={
                  <Empty
                    title="No collections yet"
                    actions={
                      search ? undefined : (
                        <Button size="sm" onClick={() => setCreating(true)}>
                          New collection
                        </Button>
                      )
                    }
                  >
                    Create a collection, then ingest a file into it.
                  </Empty>
                }
                columns={[
                  {
                    key: 'name',
                    header: 'Name',
                    cell: (c) => (
                      <a className="ds-link" href={`#${open(c.name)}`}>
                        <code>{c.name}</code>
                      </a>
                    ),
                  },
                  {
                    key: 'docs',
                    header: 'Documents',
                    numeric: true,
                    cell: (c) => c.liveDocCount.toLocaleString('en-US'),
                  },
                  {
                    key: 'ver',
                    header: 'Versions',
                    numeric: true,
                    cell: (c) => c.manifestVersion.toString(),
                  },
                  {
                    key: 'hot',
                    header: 'Hot',
                    cell: (c) =>
                      c.hot?.enabled ? (
                        <StatusTag status="done">hot</StatusTag>
                      ) : (
                        <Badge>cold</Badge>
                      ),
                  },
                ]}
              />
            )}
          </LoadingSwap>
        )}
      </Card>
      <NewCollection
        client={client}
        ns={ns}
        open={creating}
        onClose={() => setCreating(false)}
        onCreated={(name) => {
          setCreating(false);
          navigate(`/data/${encodeURIComponent(ns)}/${encodeURIComponent(name)}`);
        }}
      />
    </div>
  );
}
