//! LV1 plan Task 4: pagination cursors. `LiveTxn::paginate` returns pages
//! with a continue cursor, its read set ends at the page's last key, and a
//! cursor is refused when forged or when it comes from another app or
//! another index. The codec tests need no store; the rest are
//! `live_test!`s, on the embedded store and on TiKV with `LOAMS_TEST_PD`.

use futures::future::BoxFuture;
use loams_kv::{CommitMode, TxnOptions};
use loams_live::catalog::{self, BY_CREATION_TIME};
use loams_live::cursor::{self, CursorKey, KeyBound};
use loams_live::query::{QueryArgs, doc_value, object_args};
use loams_live::system::{self, INSERT};
use loams_live::testing::TestStore;
use loams_live::{
    AppKeys, DocId, FnKind, Function, IndexId, IndexSpec, Limits, LiveError, LiveTxn, LiveValue,
    Runner, live_test, pb,
};

// ---- helpers ----

fn s(v: &str) -> LiveValue {
    LiveValue::Str(v.to_string())
}

fn obj(pairs: &[(&str, LiveValue)]) -> LiveValue {
    LiveValue::Object(
        pairs
            .iter()
            .map(|(k, v)| (k.to_string(), v.clone()))
            .collect(),
    )
}

fn field(v: &LiveValue, name: &str) -> LiveValue {
    match v {
        LiveValue::Object(f) => f.get(name).cloned().unwrap_or(LiveValue::Null),
        other => panic!("an object, not {other:?}"),
    }
}

async fn open(store: &TestStore) -> Runner {
    Runner::open(store.store(), &store.live_config("cursor"))
        .await
        .expect("the runner opens")
}

async fn insert(r: &Runner, table: &str, n: i64) -> DocId {
    let f = system::lookup(INSERT).expect("insert");
    let m = r
        .mutate(
            f,
            obj(&[
                ("table", s(table)),
                ("fields", obj(&[("n", LiveValue::I64(n))])),
            ]),
            None,
        )
        .await
        .expect("the insert commits");
    let LiveValue::Str(id) = m.result else {
        panic!("an id, not {:?}", m.result)
    };
    id.parse().expect("a document id")
}

async fn define(r: &Runner, table: &str, indexes: &[(&str, &[&str])]) {
    let table = table.to_string();
    let specs: Vec<IndexSpec> = indexes
        .iter()
        .map(|(n, f)| IndexSpec {
            name: (*n).to_string(),
            fields: f.iter().map(|s| (*s).to_string()).collect(),
        })
        .collect();
    let mut opts = TxnOptions::new("test.define");
    opts.commit_mode = Some(CommitMode::TwoPc);
    r.store()
        .run(opts, move |txn| {
            let (table, specs) = (table.clone(), specs.clone());
            Box::pin(async move {
                catalog::define_table(
                    txn,
                    &AppKeys::dedicated(),
                    &table,
                    &specs,
                    &Limits::default(),
                )
                .await
                .map_err(|e| loams_kv::TxnError::Fatal(e.to_string()))
            })
        })
        .await
        .expect("the table is defined");
}

/// `{ table, index?, order?, eq?, cursor: string | null, numItems }`:
/// one page of the range, as `{ page, continueCursor, isDone }`.
struct Pager;

impl Function for Pager {
    fn name(&self) -> &str {
        "test:page"
    }

    fn kind(&self) -> FnKind {
        FnKind::Query
    }

    fn call<'a>(
        &'a self,
        txn: &'a mut LiveTxn<'_>,
        args: LiveValue,
    ) -> BoxFuture<'a, Result<LiveValue, LiveError>> {
        Box::pin(async move {
            let mut args = object_args(
                "test:page",
                args,
                &["table", "index", "order", "eq", "cursor", "numItems"],
            )?;
            let cursor = match args.remove("cursor") {
                Some(LiveValue::Str(c)) => Some(c),
                _ => None,
            };
            let Some(LiveValue::I64(n)) = args.remove("numItems") else {
                return Err(LiveError::InvalidArgument("numItems".into()));
            };
            let query = QueryArgs::parse("test:page", LiveValue::Object(args))?;
            let page = match txn.table(&query.table).await? {
                Some(table) => {
                    let range = query.range(&table)?;
                    txn.paginate(range, cursor.as_deref(), u32::try_from(n).expect("n"))
                        .await?
                }
                None => txn.empty_page(cursor.as_deref()).await?,
            };
            Ok(obj(&[
                (
                    "page",
                    LiveValue::Array(page.docs.iter().map(doc_value).collect()),
                ),
                ("continueCursor", s(&page.continue_cursor)),
                ("isDone", LiveValue::Bool(page.is_done)),
            ]))
        })
    }
}

/// One page, through the runner, at the store's current time.
async fn page(r: &Runner, args: LiveValue) -> Result<loams_live::Queried, LiveError> {
    let at = r.store().now().await.expect("now");
    r.query(&Pager, args, at).await
}

fn page_args(table: &str, index: &str, order: &str, cursor: Option<&str>, n: i64) -> LiveValue {
    obj(&[
        ("table", s(table)),
        ("index", s(index)),
        ("order", s(order)),
        ("cursor", cursor.map_or(LiveValue::Null, s)),
        ("numItems", LiveValue::I64(n)),
    ])
}

fn ns(result: &LiveValue) -> Vec<i64> {
    match field(result, "page") {
        LiveValue::Array(docs) => docs
            .iter()
            .map(|d| match field(d, "n") {
                LiveValue::I64(n) => n,
                other => panic!("n is an int64, not {other:?}"),
            })
            .collect(),
        other => panic!("a page is an array, not {other:?}"),
    }
}

fn next_cursor(result: &LiveValue) -> String {
    match field(result, "continueCursor") {
        LiveValue::Str(c) => c,
        other => panic!("continueCursor is a string, not {other:?}"),
    }
}

fn is_done(result: &LiveValue) -> bool {
    field(result, "isDone") == LiveValue::Bool(true)
}

fn assert_bad_cursor(result: Result<impl std::fmt::Debug, LiveError>, what: &str) {
    match result {
        Err(e) => {
            assert_eq!(
                e.code(),
                pb::ErrorCode::ERROR_CODE_INVALID_ARGUMENT,
                "{what}: {e}"
            );
            assert_eq!(e.reason(), Some("live_bad_cursor"), "{what}: {e}");
        }
        Ok(v) => panic!("{what}: the cursor was accepted: {v:?}"),
    }
}

/// The index entry key of document `id` (its `n` is `n`) in index `index`
/// of its table, as the store writes it.
fn entry_key(index: IndexId, n: i64, creation_ms: u64, id: &DocId) -> Vec<u8> {
    let value = LiveValue::I64(n);
    let elem = value.index_elem().expect("an index element");
    AppKeys::dedicated().index_entry(index, &[elem], creation_ms, id)
}

async fn creation_ms(r: &Runner, id: DocId) -> u64 {
    let get = system::lookup(system::GET).expect("get");
    let at = r.store().now().await.expect("now");
    let doc = r
        .query(&*get, obj(&[("id", s(&id.to_string()))]), at)
        .await
        .expect("the get runs")
        .result;
    match field(&doc, "_creationTime") {
        LiveValue::I64(ms) => u64::try_from(ms).expect("a creation time"),
        other => panic!("_creationTime is an int64, not {other:?}"),
    }
}

// ---- the codec ----

#[test]
fn cursor_roundtrips_and_names_its_index() {
    let key = CursorKey::from_bytes([7; 32]);
    for bound in [
        KeyBound {
            index: IndexId::BY_CREATION_TIME,
            key: Vec::new(),
        },
        KeyBound {
            index: IndexId(9),
            key: b"\x03\x00\x00\x00\x01\x00\x00\x00\x09\x15\x01".to_vec(),
        },
    ] {
        let text = cursor::encode(&key, &bound);
        assert!(
            text.bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
            "base64url without padding: {text}"
        );
        assert_eq!(cursor::decode(&key, &text).expect("decodes"), bound);
    }
}

#[test]
fn every_flipped_bit_is_rejected() {
    let key = CursorKey::from_bytes([3; 32]);
    let bound = KeyBound {
        index: IndexId(4),
        key: b"\x03\x00\x00\x00\x01\x00\x00\x00\x04\x15\x2a".to_vec(),
    };
    let text = cursor::encode(&key, &bound);
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(text.as_bytes())
        .expect("base64url");
    for bit in 0..raw.len() * 8 {
        let mut forged = raw.clone();
        forged[bit / 8] ^= 1 << (bit % 8);
        let forged = data_encoding::BASE64URL_NOPAD.encode(&forged);
        assert_bad_cursor(cursor::decode(&key, &forged), &format!("bit {bit}"));
    }
    for garbage in ["", "!!", "AAAA", &"A".repeat(100_000)] {
        assert_bad_cursor(cursor::decode(&key, garbage), garbage);
    }
    let other = CursorKey::from_bytes([4; 32]);
    assert_bad_cursor(cursor::decode(&other, &text), "another key");
}

// ---- paginate ----

live_test!(paginate_returns_pages_and_is_done);
async fn paginate_returns_pages_and_is_done(store: TestStore) {
    let r = open(&store).await;
    // A unique field orders the documents; creation times can tie within a
    // millisecond.
    define(&r, "t", &[("by_n", &["n"])]).await;
    for n in 0..7 {
        insert(&r, "t", n).await;
    }
    let mut asc_end = String::new();
    for (order, want) in [
        ("asc", [vec![0, 1, 2], vec![3, 4, 5], vec![6]]),
        ("desc", [vec![6, 5, 4], vec![3, 2, 1], vec![0]]),
    ] {
        let mut cursor: Option<String> = None;
        for (i, expected) in want.iter().enumerate() {
            let q = page(&r, page_args("t", "by_n", order, cursor.as_deref(), 3))
                .await
                .expect("the page runs");
            assert_eq!(&ns(&q.result), expected, "{order} page {i}");
            assert_eq!(is_done(&q.result), i == 2, "{order} page {i} isDone");
            cursor = Some(next_cursor(&q.result));
        }
        // Past the end: an empty page, still done, and its cursor still
        // marks the end (a later insert shows up there).
        let end = page(&r, page_args("t", "by_n", order, cursor.as_deref(), 3))
            .await
            .expect("the page runs");
        assert_eq!(ns(&end.result), Vec::<i64>::new(), "{order} past the end");
        assert!(is_done(&end.result), "{order} past the end is done");
        if order == "asc" {
            asc_end = next_cursor(&end.result);
        }
    }
    insert(&r, "t", 7).await;
    let more = page(&r, page_args("t", "by_n", "asc", Some(&asc_end), 3))
        .await
        .expect("the page runs");
    assert_eq!(ns(&more.result), vec![7], "the end cursor resumes");
    // A full page that ends the range: not done until a page comes back
    // short.
    let all = page(&r, page_args("t", "by_n", "asc", None, 8))
        .await
        .expect("the page runs");
    assert_eq!(ns(&all.result).len(), 8);
    assert!(!is_done(&all.result));
    // An empty table: an empty, done first page.
    define(&r, "empty", &[("by_n", &["n"])]).await;
    let none = page(&r, page_args("empty", "by_n", "asc", None, 3))
        .await
        .expect("the page runs");
    assert_eq!(ns(&none.result), Vec::<i64>::new());
    assert!(is_done(&none.result));
    // numItems is at least 1.
    assert!(matches!(
        page(&r, page_args("t", "by_n", "asc", None, 0)).await,
        Err(LiveError::InvalidArgument(_))
    ));
}

live_test!(paginate_read_set_ends_at_last_key);
async fn paginate_read_set_ends_at_last_key(store: TestStore) {
    let r = open(&store).await;
    define(&r, "t", &[("by_n", &["n"])]).await;
    // n = 0, 2, 4, …, 18.
    for n in (0..20).step_by(2) {
        insert(&r, "t", n).await;
    }
    let first = page(&r, page_args("t", "by_n", "asc", None, 3))
        .await
        .expect("the first page");
    assert_eq!(ns(&first.result), vec![0, 2, 4]);
    let second = page(
        &r,
        page_args("t", "by_n", "asc", Some(&next_cursor(&first.result)), 3),
    )
    .await
    .expect("the second page");
    assert_eq!(ns(&second.result), vec![6, 8, 10]);

    let index = IndexId(IndexId::FIRST_USER);
    // An insert inside the first page invalidates it, one after its last
    // key does not.
    let inside = insert(&r, "t", 3).await;
    let inside_key = entry_key(index, 3, creation_ms(&r, inside).await, &inside);
    let after = insert(&r, "t", 5).await;
    let after_key = entry_key(index, 5, creation_ms(&r, after).await, &after);
    assert!(first.read_set.covers(&inside_key), "n = 3 is in page 1");
    assert!(
        !first.read_set.covers(&after_key),
        "n = 5 is after page 1's last key"
    );
    // The second page starts after the first page's last key and ends at
    // its own.
    assert!(
        !second.read_set.covers(&inside_key),
        "n = 3 is before page 2"
    );
    assert!(second.read_set.covers(&after_key), "n = 5 is in page 2");
    let late = insert(&r, "t", 11).await;
    let late_key = entry_key(index, 11, creation_ms(&r, late).await, &late);
    assert!(
        !second.read_set.covers(&late_key),
        "n = 11 is after page 2's last key"
    );
    // The last page depends on everything after its start: an insert at the
    // end belongs to it.
    let mut cursor = next_cursor(&second.result);
    let last = loop {
        let q = page(&r, page_args("t", "by_n", "asc", Some(&cursor), 3))
            .await
            .expect("a page");
        if is_done(&q.result) {
            break q;
        }
        cursor = next_cursor(&q.result);
    };
    let tail = insert(&r, "t", 100).await;
    let tail_key = entry_key(index, 100, creation_ms(&r, tail).await, &tail);
    assert!(
        last.read_set.covers(&tail_key),
        "the last page reads to the end"
    );
}

live_test!(forged_cursor_rejected);
async fn forged_cursor_rejected(store: TestStore) {
    let r = open(&store).await;
    define(&r, "t", &[("by_n", &["n"])]).await;
    for n in 0..5 {
        insert(&r, "t", n).await;
    }
    let first = page(&r, page_args("t", "by_n", "asc", None, 2))
        .await
        .expect("the first page");
    let good = next_cursor(&first.result);
    page(&r, page_args("t", "by_n", "asc", Some(&good), 2))
        .await
        .expect("the genuine cursor works");
    // The app's cursor key is a catalog record: a reopened runner (another
    // node, a restart) takes the same cursors.
    let reopened = open(&store).await;
    page(&reopened, page_args("t", "by_n", "asc", Some(&good), 2))
        .await
        .expect("the cursor survives a reopen");
    let raw = data_encoding::BASE64URL_NOPAD
        .decode(good.as_bytes())
        .expect("base64url");
    // One bit flipped: in the version, the index id, the key and the tag.
    for bit in [0, 8 * 3 + 1, 8 * (raw.len() - 20), 8 * raw.len() - 1] {
        let mut forged = raw.clone();
        forged[bit / 8] ^= 1 << (bit % 8);
        let forged = data_encoding::BASE64URL_NOPAD.encode(&forged);
        assert_bad_cursor(
            page(&r, page_args("t", "by_n", "asc", Some(&forged), 2)).await,
            &format!("bit {bit}"),
        );
    }
    // A genuine cursor of another index of the same app.
    assert_bad_cursor(
        page(&r, page_args("t", BY_CREATION_TIME, "asc", Some(&good), 2)).await,
        "another index",
    );
    // A genuine cursor of the same index id on another table.
    define(&r, "u", &[("by_n", &["n"])]).await;
    insert(&r, "u", 1).await;
    assert_bad_cursor(
        page(&r, page_args("u", "by_n", "asc", Some(&good), 2)).await,
        "another table",
    );
}

live_test!(cursor_from_other_app_rejected);
async fn cursor_from_other_app_rejected(store: TestStore) {
    let other = store.fresh_root().await;
    let (a, b) = (open(&store).await, open(&other).await);
    for n in 0..4 {
        insert(&a, "t", n).await;
        insert(&b, "t", n).await;
    }
    let from_a = page(&a, page_args("t", BY_CREATION_TIME, "asc", None, 2))
        .await
        .expect("a page of app a");
    let cursor = next_cursor(&from_a.result);
    page(
        &a,
        page_args("t", BY_CREATION_TIME, "asc", Some(&cursor), 2),
    )
    .await
    .expect("app a takes its own cursor");
    // Same table id, same index, same key layout: only the app's key tells
    // them apart.
    assert_bad_cursor(
        page(
            &b,
            page_args("t", BY_CREATION_TIME, "asc", Some(&cursor), 2),
        )
        .await,
        "app b",
    );
}

live_test!(paginate_on_a_missing_table);
async fn paginate_on_a_missing_table(store: TestStore) {
    let r = open(&store).await;
    // A table that does not exist yet: an empty, done page with a start
    // cursor, which pages the table from the beginning once it exists.
    let none = page(&r, page_args("later", "by_n", "asc", None, 2))
        .await
        .expect("an empty page");
    assert_eq!(ns(&none.result), Vec::<i64>::new());
    assert!(is_done(&none.result));
    let start = next_cursor(&none.result);
    page(&r, page_args("later", "by_n", "asc", Some(&start), 2))
        .await
        .expect("a start cursor on a missing table");
    define(&r, "later", &[("by_n", &["n"])]).await;
    for n in 0..3 {
        insert(&r, "later", n).await;
    }
    let first = page(&r, page_args("later", "by_n", "asc", Some(&start), 2))
        .await
        .expect("the start cursor pages the new table");
    assert_eq!(ns(&first.result), vec![0, 1]);
    // A cursor with a position names a key of an existing table, so it is
    // refused on a missing one.
    assert_bad_cursor(
        page(
            &r,
            page_args(
                "missing",
                "by_n",
                "asc",
                Some(&next_cursor(&first.result)),
                2,
            ),
        )
        .await,
        "a positioned cursor on a missing table",
    );
}
