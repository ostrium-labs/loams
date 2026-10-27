// The app conformance scenario (SDKP P0d): a notes app, recorded once from a
// real `loams dev` and replayed by every SDK through its **public facade**.
//
//   LOAMS_TEST_ENDPOINT=http://127.0.0.1:8080 node sdks/conformance/record-fixtures.mjs --source app
//
// `sdks/fixtures/app-scenario.md` is the same scenario in prose, for an agent
// writing a language's replay.
//
// ## How a step is written
//
// Every step names the facade call that makes it (`call`, as `module.call` in
// the facade index's names) and the request it sends, as the proto3 JSON body
// an SDK's facade produces. A step's request may be a function of the steps
// recorded before it: a read at a write's token, the next page of a scroll, the
// `search_after` of the last hit. The recorder evaluates it once, against the
// real server, and writes the result down, so a replay sends literal bytes.
//
// Every request whose message declares `idempotency_key` carries a fixed one
// (`app-…`), reads included (`ExecuteRequest` is keyed whether or not it
// writes). An SDK mints a UUIDv7 per logical call, so its replay passes the
// recorded key explicitly; the fixture server compares the request body, and a
// fresh key would not match.
//
// The `search_*` steps send exactly the builder corpus's `expect` plus the
// namespace: `verify-corpus.mjs` checks that, so the builders and the scenario
// cannot drift apart (`scenario_step` in each `sdks/fixtures/builders/` case).

import { readFileSync } from 'node:fs';
import { dirname, join } from 'node:path';
import { fileURLToPath } from 'node:url';

const here = dirname(fileURLToPath(import.meta.url));
const BUILDERS = join(here, '..', 'fixtures', 'builders');

/** The namespace the scenario runs in. */
export const APP_NAMESPACE = 'app';

const JSON_CT = 'application/json';
const NS = '/loams.collection.v1.NamespaceService';
const COL = '/loams.collection.v1.CollectionService';
const DOC = '/loams.collection.v1.DocumentService';
const QUERY = '/loams.collection.v1.QueryService';
const GRAPH_ADMIN = '/loams.graph.v1.GraphAdminService';
const GRAPH = '/loams.graph.v1.GraphService';
const INSTANCE = '/loams.instance.v1.InstanceService';
const SQL = '/loams.sql.v1.SqlService';
const STREAM = '/loams.stream.v1.StreamService';

/** A builder corpus case's `expect`, by name. */
function builderExpect(name) {
  return JSON.parse(readFileSync(join(BUILDERS, `${name}.json`), 'utf8')).expect;
}

/** A unary Connect JSON request. */
function post(path, body) {
  return {
    method: 'POST',
    path,
    headers: { 'content-type': JSON_CT },
    body: JSON.stringify(body),
  };
}

/** `{"uint": "<n>"}`, a `DocumentId`'s proto3 JSON. */
const uint = (n) => ({ uint: String(n) });

/** The five notes. `tags` is one keyword; `n` orders them; the embeddings are
 * unit-ish vectors in four dimensions so the vector ranking is unambiguous. */
export const NOTES = [
  { id: 1, title: 'Refund policy', body: 'refund policy for every order', tags: 'policy', n: 1, embedding: [1, 0, 0, 0] },
  { id: 2, title: 'Shipping times', body: 'shipping takes three days', tags: 'shipping', n: 2, embedding: [0.9, 0.1, 0, 0] },
  { id: 3, title: 'Refund window', body: 'the refund window is thirty days', tags: 'policy', n: 3, embedding: [0, 0, 1, 0] },
  { id: 4, title: 'Returns', body: 'returns need a receipt before a refund', tags: 'policy', n: 4, embedding: [0.5, 0.5, 0, 0] },
  { id: 5, title: 'Gift cards', body: 'gift cards never expire', tags: 'billing', n: 5, embedding: [0, 0, 0, 1] },
];

function upsertOp(note) {
  const { id, embedding, ...source } = note;
  return { upsert: { id: uint(id), source, vectors: { embedding } } };
}

/** The last step's response body. */
const body = (steps, at) => steps[at].response.body ?? {};

/** The consistency token a write step answered with. */
const tokenOf = (steps, at) => body(steps, at).token;

/** A search step from a builder corpus case, in the scenario's namespace. */
function searchStep(name, call, extra = {}) {
  return {
    about: `The builder corpus case \`${name}\`, sent as \`${call}\`.`,
    call,
    builder: name,
    request: post(`${QUERY}/Search`, { namespace: APP_NAMESPACE, ...builderExpect(name), ...extra }),
    expect: { status: 200 },
  };
}

/**
 * `app_collections`: namespaces, collections, documents, consistency tokens,
 * idempotency, pagination and the typed `where` filter.
 */
const APP_COLLECTIONS = {
  name: 'app_collections',
  kind: 'app',
  transport: 'connect',
  required: false,
  pinnedBy: ['R3', 'R4', 'R6', 'R8', 'R11'],
  about:
    'The notes app, part one: create a namespace and a collection from the Schema builder, ' +
    'page the collections, upsert five notes with an idempotency key and replay it, read ' +
    'them back at the write\'s token, patch and delete, count with a typed `where`, scroll ' +
    'every page, and the two refusals an app meets first (`not_found`, `filter_conflict`).',
  steps: [
    {
      about: 'Feature detection: what this instance serves.',
      call: 'instance.get_instance',
      request: post(`${INSTANCE}/GetInstance`, {}),
      expect: { status: 200, apiVersions: ['loams.collection.v1'] },
    },
    {
      about: 'Create the namespace.',
      call: 'namespaces.create',
      request: post(`${NS}/CreateNamespace`, { namespace: APP_NAMESPACE }),
      expect: { status: 200 },
    },
    {
      about: 'Create it again: `already_exists`, which `retry_safe` callers treat as done.',
      call: 'namespaces.create',
      request: post(`${NS}/CreateNamespace`, { namespace: APP_NAMESPACE }),
      expect: { status: 409, reason: 'already_exists' },
    },
    {
      about: 'Create `notes` with the Schema builder\'s `schema_notes` output.',
      call: 'collections.create',
      builder: 'schema_notes',
      request: post(`${COL}/CreateCollection`, {
        namespace: APP_NAMESPACE,
        name: 'notes',
        schema: builderExpect('schema_notes'),
      }),
      expect: { status: 200 },
    },
    {
      about: 'Read it back.',
      call: 'collections.get',
      request: post(`${COL}/GetCollection`, { namespace: APP_NAMESPACE, collection: 'notes' }),
      expect: { status: 200 },
    },
    {
      about: 'A second collection, so the list has two pages.',
      call: 'collections.create',
      request: post(`${COL}/CreateCollection`, {
        namespace: APP_NAMESPACE,
        name: 'scratch',
        schema: builderExpect('schema_notes'),
      }),
      expect: { status: 200 },
    },
    {
      about: 'List one collection a page: the first page, with a `next_page_token`.',
      call: 'collections.list',
      request: post(`${COL}/ListCollections`, { namespace: APP_NAMESPACE, pageSize: 1 }),
      expect: { status: 200 },
    },
    {
      about: 'The second page, from the first page\'s token (`list_all` follows it).',
      call: 'collections.list',
      request: (steps) =>
        post(`${COL}/ListCollections`, {
          namespace: APP_NAMESPACE,
          pageSize: 1,
          pageToken: body(steps, steps.length - 1).nextPageToken,
        }),
      expect: { status: 200 },
    },
    {
      about: 'Upsert the five notes, keyed (R3).',
      call: 'documents.write',
      request: post(`${DOC}/WriteDocuments`, {
        namespace: APP_NAMESPACE,
        collection: 'notes',
        ops: NOTES.map(upsertOp),
        idempotencyKey: 'app-upsert-1',
      }),
      expect: { status: 200 },
    },
    {
      about: 'The same write with the same key: the first answer, not a second write.',
      call: 'documents.write',
      request: post(`${DOC}/WriteDocuments`, {
        namespace: APP_NAMESPACE,
        collection: 'notes',
        ops: NOTES.map(upsertOp),
        idempotencyKey: 'app-upsert-1',
      }),
      expect: { status: 200, identicalToStep: 8 },
    },
    {
      about: 'Read the notes at the write\'s token: read-your-writes (R4).',
      call: 'documents.get',
      request: (steps) =>
        post(`${DOC}/GetDocuments`, {
          namespace: APP_NAMESPACE,
          collection: 'notes',
          ids: NOTES.map((note) => uint(note.id)),
          consistency: { atLeast: tokenOf(steps, 8) },
        }),
      expect: { status: 200 },
    },
    {
      about: 'Patch note 2 (`documents.patch`).',
      call: 'documents.write',
      request: post(`${DOC}/WriteDocuments`, {
        namespace: APP_NAMESPACE,
        collection: 'notes',
        ops: [{ patch: { id: uint(2), source: { pinned: true } } }],
        idempotencyKey: 'app-patch-1',
      }),
      expect: { status: 200 },
    },
    {
      about: 'Delete note 5 (`documents.delete`).',
      call: 'documents.write',
      request: post(`${DOC}/WriteDocuments`, {
        namespace: APP_NAMESPACE,
        collection: 'notes',
        ops: [{ delete: { id: uint(5) } }],
        idempotencyKey: 'app-delete-1',
      }),
      expect: { status: 200 },
    },
    {
      about: 'Count the policy notes with a typed `where` (the Filter builder\'s `filter_term`).',
      call: 'documents.count',
      builder: 'filter_term',
      request: (steps) =>
        post(`${DOC}/CountDocuments`, {
          namespace: APP_NAMESPACE,
          collection: 'notes',
          where: builderExpect('filter_term'),
          consistency: { atLeast: tokenOf(steps, 12) },
        }),
      expect: { status: 200 },
    },
    {
      about: 'Scroll two at a time: the first page.',
      call: 'documents.scroll',
      request: (steps) =>
        post(`${DOC}/ScrollDocuments`, {
          namespace: APP_NAMESPACE,
          collection: 'notes',
          limit: 2,
          consistency: { atLeast: tokenOf(steps, 12) },
        }),
      expect: { status: 200 },
    },
    {
      about:
        'The second and last page, `after` the first page\'s `next`: it answers no `next`, ' +
        'which is where `scroll_all` stops.',
      call: 'documents.scroll',
      request: (steps) =>
        post(`${DOC}/ScrollDocuments`, {
          ...JSON.parse(steps[steps.length - 1].request.body),
          after: body(steps, steps.length - 1).next,
        }),
      expect: { status: 200 },
    },
    {
      about: 'A collection that is not there: `not_found` (R8).',
      call: 'collections.get',
      request: post(`${COL}/GetCollection`, { namespace: APP_NAMESPACE, collection: 'missing' }),
      expect: { status: 404, reason: 'not_found' },
    },
    {
      about: 'Both `filter` and `where`: `filter_conflict`, refused before anything is read.',
      call: 'documents.count',
      request: post(`${DOC}/CountDocuments`, {
        namespace: APP_NAMESPACE,
        collection: 'notes',
        filter: { term: { field: 'tags', value: 'policy' } },
        where: builderExpect('filter_term'),
      }),
      expect: { status: 400, reason: 'filter_conflict' },
    },
    {
      about: 'Drop `scratch`.',
      call: 'collections.drop',
      request: post(`${COL}/DropCollection`, { namespace: APP_NAMESPACE, collection: 'scratch' }),
      expect: { status: 200 },
    },
  ],
};

/** `app_search`: text, vector and hybrid search over the notes. */
const APP_SEARCH = {
  name: 'app_search',
  kind: 'app',
  transport: 'connect',
  required: false,
  pinnedBy: ['R6', 'R8'],
  about:
    'The notes app, part two: every search the builder corpus spells, against the notes ' +
    '`app_collections` wrote (four of them, after its delete). Text with a filter, highlights ' +
    'and a projection; its next page through `search_after`; a vector search; hybrid with ' +
    'RRF, DBSF and a weighted sum; and the weighted sum refused for the wrong number of weights.',
  steps: [
    searchStep('search_text_filter_highlight_select', 'search.query'),
    {
      about:
        'The next page, `search_after` the last hit\'s `sort_values` (`query_all`). The builder ' +
        'corpus\'s `search_after_page` is this body.',
      call: 'search.query',
      builder: 'search_after_page',
      request: (steps) => {
        const hits = body(steps, 0).hits ?? [];
        return post(`${QUERY}/Search`, {
          namespace: APP_NAMESPACE,
          ...builderExpect('search_text_filter_highlight_select'),
          searchAfter: hits[hits.length - 1]?.sortValues ?? [],
        });
      },
      expect: { status: 200 },
    },
    searchStep('search_vector_ann_params', 'vector.search'),
    searchStep('search_hybrid_rrf', 'search.query'),
    searchStep('search_hybrid_dbsf', 'search.query'),
    searchStep('search_hybrid_weighted', 'search.query'),
    {
      about:
        'A weighted sum with one weight for two retrievers: refused. The builders refuse it ' +
        'before sending; this is the server\'s own refusal of the raw request.',
      call: 'search.query',
      request: post(`${QUERY}/Search`, {
        namespace: APP_NAMESPACE,
        ...builderExpect('search_hybrid_weighted'),
        fusion: { weightedSum: { weights: [1] } },
      }),
      expect: { status: 400, reason: 'invalid_argument' },
    },
  ],
};

/**
 * `app_graph`: Loams Graph from the same app. `loams dev`'s default build has
 * no `graph` feature, so every step records the refusal an SDK must surface as
 * `FeatureNotInVariant` (R5); a build with `--features graph` records the
 * graph itself under the same steps.
 */
const APP_GRAPH = {
  name: 'app_graph',
  kind: 'app',
  transport: 'connect',
  required: false,
  pinnedBy: ['R5', 'R8'],
  about:
    'The notes app, part three: a graph of notes and tags, queried with GQL and walked from ' +
    'a search. Recorded from `loams dev`\'s default build, which carries no `graph` feature, ' +
    'so each step is the `feature_not_in_variant` refusal an SDK reports as ' +
    '`FeatureNotInVariant` — and the expand stage of a search is refused the same way.',
  steps: [
    {
      about: 'Create the graph.',
      call: 'graph.create',
      request: post(`${GRAPH_ADMIN}/CreateGraph`, {
        namespace: APP_NAMESPACE,
        name: 'notes_graph',
        idempotencyKey: 'app-graph-create-1',
      }),
      expect: { status: 501, reason: 'feature_not_in_variant' },
    },
    {
      about: 'Insert notes, tags and `TAGGED` edges with GQL and parameters, keyed.',
      call: 'graph.query',
      request: post(`${GRAPH}/Execute`, {
        namespace: APP_NAMESPACE,
        graph: 'notes_graph',
        statement:
          'INSERT (:Note {id: $id, title: $title})-[:TAGGED]->(:Tag {name: $tag})',
        parameters: { id: { int64: '1' }, title: { string: 'Refund policy' }, tag: { string: 'policy' } },
        idempotencyKey: 'app-graph-insert-1',
      }),
      expect: { status: 501, reason: 'feature_not_in_variant' },
    },
    {
      about: 'Read it back, read-only.',
      call: 'graph.query',
      request: post(`${GRAPH}/Execute`, {
        namespace: APP_NAMESPACE,
        graph: 'notes_graph',
        statement: 'MATCH (n:Note)-[:TAGGED]->(t:Tag) RETURN n.title, t.name',
        readOnly: true,
        idempotencyKey: 'app-graph-read-1',
      }),
      expect: { status: 501, reason: 'feature_not_in_variant' },
    },
    {
      ...searchStep('search_expand', 'search.query'),
      about:
        'The builder corpus case `search_expand`: a search that walks the graph from its hits. ' +
        'Without the graph feature the expand stage is refused as `invalid_argument` ("expand ' +
        'runs on Loams Graph, which this server does not run"), not `feature_not_in_variant`: ' +
        'the search itself is served.',
      expect: { status: 400, reason: 'invalid_argument' },
    },
    {
      about: 'Delete the graph.',
      call: 'graph.delete',
      request: post(`${GRAPH_ADMIN}/DeleteGraph`, {
        namespace: APP_NAMESPACE,
        name: 'notes_graph',
        idempotencyKey: 'app-graph-delete-1',
      }),
      expect: { status: 501, reason: 'feature_not_in_variant' },
    },
  ],
};

/**
 * `app_sql`: SQL over the notes `app_collections` wrote (four of them, after
 * its delete), with `$n` parameters (SDKP Part C). Part C: a language is held
 * to it once its suite names `<lang>_app_sql` (see `required.mjs`).
 */
const APP_SQL = {
  name: 'app_sql',
  kind: 'app',
  part: 'C',
  transport: 'connect',
  required: false,
  pinnedBy: ['R8'],
  about:
    'The notes app, part four: SQL over the `notes` collection. A grouped count with a `$1` ' +
    'parameter, its rows as records; a capped read that answers `truncated`; the same ' +
    'statement as Arrow IPC; and the two refusals (`sql_param_count`, a statement that is not ' +
    'a query).',
  steps: [
    {
      about: 'Count the notes per tag past `n > $1`, ordered: `rows` in `columns` order.',
      call: 'sql.query',
      request: post(`${SQL}/Query`, {
        namespace: APP_NAMESPACE,
        sql: 'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags',
        params: [1],
      }),
      expect: { status: 200 },
    },
    {
      about: 'At most two rows of four: `truncated`.',
      call: 'sql.query',
      request: post(`${SQL}/Query`, {
        namespace: APP_NAMESPACE,
        sql: 'SELECT title, n FROM notes ORDER BY n',
        maxRows: 2,
      }),
      expect: { status: 200 },
    },
    {
      about: 'The first statement as one Arrow IPC stream (`arrowIpc`, base64 in JSON).',
      call: 'sql.query_arrow',
      request: post(`${SQL}/QueryArrow`, {
        namespace: APP_NAMESPACE,
        sql: 'SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags',
        params: [1],
      }),
      expect: { status: 200 },
    },
    {
      about: 'A `$1` with no value: `invalid_argument`, reason `sql_param_count`.',
      call: 'sql.query',
      request: post(`${SQL}/Query`, {
        namespace: APP_NAMESPACE,
        sql: 'SELECT title FROM notes WHERE n > $1',
      }),
      expect: { status: 400, reason: 'sql_param_count' },
    },
    {
      about: 'A statement that is not a query: `invalid_argument`.',
      call: 'sql.query',
      request: post(`${SQL}/Query`, { namespace: APP_NAMESPACE, sql: 'DELETE FROM notes' }),
      expect: { status: 400, reason: 'invalid_argument' },
    },
  ],
};

/** A UTF-8 string as the base64 a `bytes` field takes in proto3 JSON. */
const b64 = (text) => Buffer.from(text, 'utf8').toString('base64');

/** The two CloudEvents `app_streams` produces, as one JSON batch. */
const EVENTS = "[{\"specversion\":\"1.0\",\"id\":\"evt-1\",\"source\":\"/notes\",\"type\":\"note.created\",\"datacontenttype\":\"application/json\",\"data\":{\"id\":1}},{\"specversion\":\"1.0\",\"id\":\"evt-2\",\"source\":\"/notes\",\"type\":\"note.deleted\",\"datacontenttype\":\"application/json\",\"data\":{\"id\":5}}]";

/**
 * `app_streams`: the namespace's logs (SDKP Part C). A stream is created
 * (twice: idempotent by name), listed and described; records are produced with
 * a producer key, the same batch again is answered as a duplicate, and fetched
 * back from offset 0 and past the end; CloudEvents go to a second stream and
 * come back as a JSON batch; a fetch from a stream that does not exist is
 * `not_found`. `streams.subscribe` (R14) is a loop over `fetch`, so the two
 * fetches are the pages it reads.
 */
// A request is recorded in the proto3 JSON an SDK sends: a field at its zero
// value (`partition: 0`, `offset: "0"`) is left out, an `optional` one set to
// zero (`ProduceCloudEvents.partition`) is kept.
const APP_STREAMS = {
  name: 'app_streams',
  kind: 'app',
  part: 'C',
  transport: 'connect',
  required: false,
  pinnedBy: ['R3', 'R8', 'R14'],
  about:
    'The notes app, part five: streams. Create `note-events` (and again: the same stream), list and ' +
    'describe it, produce two records keyed by a producer id and the same batch again (a ' +
    'duplicate), fetch them from offset 0 and past the end, produce and fetch two CloudEvents ' +
    'on `note-audit`, and fetch a stream that does not exist (`not_found`).',
  steps: [
    {
      about: 'Create `note-events`, one partition.',
      call: 'streams.create',
      request: post(`${STREAM}/CreateStream`, { namespace: APP_NAMESPACE, name: 'note-events', partitions: 1 }),
      expect: { status: 200 },
    },
    {
      about: 'Create it again: idempotent by name, the same stream with `created` unset.',
      call: 'streams.create',
      request: post(`${STREAM}/CreateStream`, { namespace: APP_NAMESPACE, name: 'note-events', partitions: 1 }),
      expect: { status: 200 },
    },
    {
      about: 'The namespace\'s streams (`list_all` pages through `next_page_token`).',
      call: 'streams.list',
      request: post(`${STREAM}/ListStreams`, { namespace: APP_NAMESPACE }),
      expect: { status: 200 },
    },
    {
      about: 'Two records, keyed by `producer_id` + `producer_seq` so a retry cannot append twice.',
      call: 'streams.produce',
      request: post(`${STREAM}/Produce`, {
        namespace: APP_NAMESPACE,
        stream: 'note-events',
        records: [
          { key: b64('1'), value: b64('{"note":1,"op":"created"}') },
          { key: b64('5'), value: b64('{"note":5,"op":"deleted"}') },
        ],
        producerId: 'app-producer-1',
        producerSeq: '1',
      }),
      expect: { status: 200 },
    },
    {
      about: 'The same batch again: answered as a `duplicate`, nothing appended.',
      call: 'streams.produce',
      request: post(`${STREAM}/Produce`, {
        namespace: APP_NAMESPACE,
        stream: 'note-events',
        records: [
          { key: b64('1'), value: b64('{"note":1,"op":"created"}') },
          { key: b64('5'), value: b64('{"note":5,"op":"deleted"}') },
        ],
        producerId: 'app-producer-1',
        producerSeq: '1',
      }),
      expect: { status: 200 },
    },
    {
      about: 'Fetch from offset 0: both records; `next_offset` is where `subscribe` reads next.',
      call: 'streams.fetch',
      request: post(`${STREAM}/Fetch`, {
        namespace: APP_NAMESPACE,
        stream: 'note-events',
        maxRecords: 10,
      }),
      expect: { status: 200 },
    },
    {
      about: 'Fetch past the end: no records, the page `subscribe` backs off on.',
      call: 'streams.fetch',
      request: post(`${STREAM}/Fetch`, {
        namespace: APP_NAMESPACE,
        stream: 'note-events',
        offset: '2',
        maxRecords: 10,
      }),
      expect: { status: 200 },
    },
    {
      about: 'Describe it: each partition\'s offsets.',
      call: 'streams.get',
      request: post(`${STREAM}/DescribeStream`, { namespace: APP_NAMESPACE, stream: 'note-events' }),
      expect: { status: 200 },
    },
    {
      about: 'Create `note-audit` for CloudEvents.',
      call: 'streams.create',
      request: post(`${STREAM}/CreateStream`, { namespace: APP_NAMESPACE, name: 'note-audit', partitions: 1 }),
      expect: { status: 200 },
    },
    {
      about: 'Two CloudEvents as one JSON batch: each `appended`.',
      call: 'streams.produce_events',
      request: post(`${STREAM}/ProduceCloudEvents`, {
        namespace: APP_NAMESPACE,
        stream: 'note-audit',
        partition: 0,
        jsonBatch: b64(EVENTS),
      }),
      expect: { status: 200 },
    },
    {
      about: 'Fetch them back as a JSON batch.',
      call: 'streams.fetch_events',
      request: post(`${STREAM}/FetchCloudEvents`, {
        namespace: APP_NAMESPACE,
        stream: 'note-audit',
      }),
      expect: { status: 200 },
    },
    {
      about: 'A stream that does not exist: `not_found`.',
      call: 'streams.fetch',
      request: post(`${STREAM}/Fetch`, {
        namespace: APP_NAMESPACE,
        stream: 'no-such-stream',
      }),
      expect: { status: 404, reason: 'not_found' },
    },
  ],
};

/** The app scenarios this module records, in order: each builds on the last. */
export const APP_SCENARIOS = [APP_COLLECTIONS, APP_SEARCH, APP_GRAPH, APP_SQL, APP_STREAMS];

/**
 * The response fields a re-recording may move: consistency tokens (offsets),
 * ids and times the server generates, and the per-query performance block.
 * Scores and rankings are **not** here: a change to them is a ranking change.
 */
export const APP_VOLATILE = [
  'token',
  'readToken',
  'consistencyToken',
  'collectionId',
  'namespaceId',
  'createdAtMs',
  'updatedAtMs',
  'instanceId',
  'performance',
  'manifestVersion',
  'version',
  'seqNo',
  'positions',
  'pin',
  // `app_streams`: the record times and the stream id the server assigns.
  'timestampMs',
  'streamId',
];
