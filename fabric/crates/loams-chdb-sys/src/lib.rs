//! Loams's own FFI over `libchdb`: the `bindgen` declarations of the pinned
//! `chdb.h`, and nothing else.
//!
//! FL2 Ruling 1 chose this over `chdb-rust` 2.0 — which is "experimental,
//! unstable" and pins its own Arrow — because Loams needs Arrow input
//! registration and query cancellation exactly as the C ABI offers them. Task 0
//! measured what that ABI actually is, and it is not the shape a guess would
//! produce: `chdb_connect(int argc, char ** argv)` rather than a path string,
//! `chdb_query(conn, query, format)` with no `char **error` out-parameter, and a
//! `_n` length-suffixed variant of everything
//! ([`docs/plans/fl2-dependency-spike.md`](../../../../docs/plans/fl2-dependency-spike.md) §3).
//! A first attempt to drive the library through hand-written `ctypes` bindings
//! that guessed the 1.x shape **segfaulted**. So the bindings here are generated
//! from the header the pinned binary was built from, and nothing in Loams ever
//! declares a prototype by hand.
//!
//! The header is `chdb.h`, vendored from `chdb-io/chdb-core` at tag v26.9.0
//! (`programs/local/chdb.h`, 1176 lines) because the v26.9.0 release tarball
//! ships `libchdb.so` and no header at all: Task 0 §2. The binary is pinned by
//! digest in `build.rs` — `linux-x86_64-libchdb.tar.gz` is
//! `c6398bcc71dc58d12fb81548540aacd5d8248830ec81cec537f51c114670543b`, which
//! unpacks to a single 554 MB stripped ELF — and the header is covered by review
//! instead. Ruling 2's `LIBCHDB_DIR` overrides the fetch for offline builds.
//!
//! This is the workspace's only crate with `unsafe` in it (see its
//! `Cargo.toml`: `-F unsafe_code` cannot be lowered from inside a crate, so the
//! workspace's `forbid` is restated as `allow` here and nowhere else). Every
//! `unsafe` block in Loams is a direct call into one of the declarations
//! `build.rs` generates from that header, or one of the few libc calls the House
//! worker's process needs and no safe crate offers: adopting its inherited
//! sockets and receiving the forwarder's ([`inherited`]), dropping its
//! capabilities ([`capabilities`], HS1 Task 6) and `_exit` ([`process`]).
//!
//! The [`ffi`] module below is the safe layer: it owns every `unsafe` block in
//! the workspace and hands out [`ffi::Connection`], [`ffi::Stream`],
//! [`ffi::ArrowStream`] and [`ffi::Error`] instead of raw handles, so that
//! `loams-chdb` — and anything else — can stay under the workspace's
//! `unsafe_code = "forbid"`. Each of its types states what it owns and what
//! happens when it drops.

#![allow(unsafe_code)]
// Ruling 1: the sanctioned exception, see the module docs.
// bindgen keeps the C names, so the generated declarations are `chdb_result`
// and `CHDBSuccess` rather than `ChdbResult` and `Success`. Rust's own casing
// lints are the one thing that must give way: the whole point of Ruling 1 is
// that the Rust name *is* the C symbol, so a renamed binding could no longer be
// checked against `chdb.h`, or against `nm`, by reading it.
#![allow(non_camel_case_types, non_upper_case_globals)]

include!(concat!(env!("OUT_DIR"), "/bindings.rs"));

/// The safe layer over the generated declarations.
///
/// Everything the workspace's `unsafe_code = "forbid"` forces out of the rest of
/// Loams lands here, so the ownership rules are stated once:
///
/// * [`Connection`] owns the cell `chdb_connect` returned and closes it on drop.
///   Closing the *last* connection in a process shuts the engine down, which the
///   header warns about ("repeatedly closing the last connection and
///   reconnecting … is known to corrupt the process allocator on macOS"), so a
///   caller that wants the engine kept up holds one connection for the process.
/// * [`Stream`] and [`ArrowStream`] own a `chdb_result` and destroy it on drop.
///   Either can hand the handle to a [`CancelToken`] instead, which is how a
///   cancellation and a dropped stream never both destroy it.
/// * [`Error`] carries the engine's own words. A ClickHouse exception arrives as
///   text, and the caller decides what to make of it — `loams-chdb` parses it into
///   a code, a name and a message.
pub mod ffi {
    use std::ffi::{CStr, CString, c_void};
    use std::fmt;
    use std::ptr::{self, NonNull};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};

    use arrow::array::RecordBatchReader;
    use arrow::ffi::{FFI_ArrowArray, FFI_ArrowSchema};
    use arrow::ffi_stream::FFI_ArrowArrayStream;

    use super::*;

    /// chDB's `chdb_state_CHDBSuccess`.
    const CHDBSuccess: u32 = chdb_state_CHDBSuccess;

    /// A failure from the library, with the engine's own words when it had any.
    ///
    /// A ClickHouse exception arrives as text rather than as a code — `Code: 60.
    /// DB::Exception: … (UNKNOWN_TABLE)` — so the caller parses it.
    #[derive(Clone, Debug, Eq, PartialEq)]
    pub struct Error {
        /// What the engine or the ABI said.
        pub message: String,
    }

    impl Error {
        /// Wraps a message.
        fn new(message: impl Into<String>) -> Self {
            Self {
                message: message.into(),
            }
        }
    }

    impl fmt::Display for Error {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.write_str(&self.message)
        }
    }

    impl std::error::Error for Error {}

    /// A NUL-terminated copy of a Rust string.
    fn c_string(what: &str, value: &str) -> Result<CString, Error> {
        CString::new(value).map_err(|_| Error::new(format!("{what} cannot hold a NUL byte")))
    }

    /// Reads a NUL-terminated string the library owns.
    ///
    /// # Safety
    ///
    /// `raw` must be null or a NUL-terminated string that stays valid for the
    /// read.
    unsafe fn c_str_to_string(raw: *const std::ffi::c_char) -> Option<String> {
        if raw.is_null() {
            return None;
        }
        // SAFETY: the caller promises a NUL-terminated string.
        Some(
            unsafe { CStr::from_ptr(raw) }
                .to_string_lossy()
                .into_owned(),
        )
    }

    /// A chDB connection.
    ///
    /// `chdb_connect` returns a *cell* — a pointer libchdb owns — and every other
    /// call takes the connection inside it. The cell is what `chdb_close_conn`
    /// frees, so both halves are kept.
    pub struct Connection {
        /// The cell `chdb_connect` returned: a pointer to the connection, which is
        /// what `chdb_close_conn` frees.
        cell: NonNull<*mut chdb_connection_>,
        /// The connection itself, which is what every query call takes.
        handle: NonNull<chdb_connection_>,
    }

    // SAFETY: libchdb's header says queries are thread-safe and a connection is a
    // handle the library shares with its own threads. What a connection is not for
    // is two statements at once, which is the caller's rule to keep.
    unsafe impl Send for Connection {}
    // SAFETY: as above.
    unsafe impl Sync for Connection {}

    impl fmt::Debug for Connection {
        fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
            f.debug_struct("Connection")
                .field("cell", &self.cell)
                .field("handle", &self.handle)
                .finish()
        }
    }

    impl Drop for Connection {
        fn drop(&mut self) {
            // SAFETY: the cell came from `chdb_connect` and has not been closed.
            // This closes the connection, and if it is the last one in the process
            // the engine shuts down with it.
            unsafe { chdb_close_conn(self.cell.as_ptr()) };
        }
    }

    impl Connection {
        /// Connects, with the program name added in front of `args`.
        ///
        /// A null result is the library refusing: Task 1 measured that a second
        /// connection whose arguments differ from the first one's is refused, as
        /// is a second one with a different path.
        pub fn open(args: &[String]) -> Result<Self, Error> {
            let mut owned: Vec<CString> = Vec::with_capacity(args.len() + 1);
            owned.push(c_string("the program name", "chdb")?);
            for arg in args {
                owned.push(c_string("a connection argument", arg)?);
            }
            let mut pointers: Vec<*mut std::ffi::c_char> =
                owned.iter_mut().map(|arg| arg.as_ptr() as *mut _).collect();

            // SAFETY: `pointers` holds `owned.len()` valid pointers into `owned`,
            // which outlives the call.
            let cell = unsafe { chdb_connect(pointers.len() as i32, pointers.as_mut_ptr()) };
            if cell.is_null() {
                return Err(Error::new(format!(
                    "chdb_connect refused the arguments {args:?}: another engine is already \
                     running in this process, or the path is in use"
                )));
            }
            // SAFETY: a non-null `chdb_connect` result points at a cell holding the
            // connection.
            let handle = unsafe { *cell };
            if handle.is_null() {
                // SAFETY: frees the cell, which is all that was allocated.
                unsafe { chdb_close_conn(cell) };
                return Err(Error::new(
                    "chdb_connect returned a cell with no connection in it",
                ));
            }
            // SAFETY: both were checked non-null just above.
            Ok(Self {
                cell: unsafe { NonNull::new_unchecked(cell) },
                handle: unsafe { NonNull::new_unchecked(handle) },
            })
        }

        /// The handle every query call takes.
        fn handle(&self) -> chdb_connection {
            self.handle.as_ptr()
        }

        /// The error text of a result, if it carries one.
        ///
        /// # Safety
        ///
        /// `result` must be a live `chdb_result`.
        unsafe fn result_error(result: *mut chdb_result) -> Option<String> {
            // SAFETY: the caller promises a live result.
            unsafe { c_str_to_string(chdb_result_error(result)) }
        }

        /// A result's bytes, or its error.
        ///
        /// # Safety
        ///
        /// `result` must be a live `chdb_result` this function may destroy.
        unsafe fn finish(result: *mut chdb_result) -> Result<Vec<u8>, Error> {
            // SAFETY: the caller promises a live result; it is not touched after
            // this point.
            let error = unsafe { Self::result_error(result) };
            // SAFETY: as above.
            let bytes = unsafe { Self::result_bytes(result) };
            // SAFETY: as above, and the bytes above are a copy.
            unsafe { chdb_destroy_query_result(result) };
            match error {
                Some(message) => Err(Error::new(message)),
                None => Ok(bytes),
            }
        }

        /// A result's bytes.
        ///
        /// # Safety
        ///
        /// `result` must be a live `chdb_result`.
        unsafe fn result_bytes(result: *mut chdb_result) -> Vec<u8> {
            // SAFETY: the caller promises a live result.
            let (buffer, length) =
                unsafe { (chdb_result_buffer(result), chdb_result_length(result)) };
            // A result with an error has a null buffer, and `slice::from_raw_parts`
            // aborts on a null pointer rather than reading an empty slice.
            if buffer.is_null() || length == 0 {
                return Vec::new();
            }
            // SAFETY: `buffer` points at `length` bytes that stay valid until the
            // result is destroyed, and both are non-null and in range.
            unsafe { std::slice::from_raw_parts(buffer as *const u8, length) }.to_vec()
        }

        /// Runs a statement and returns its whole result.
        ///
        /// `params` go through the ABI's parameter form when there are any, so a
        /// value with an interior NUL is safe.
        pub fn query(
            &self,
            sql: &str,
            format: &str,
            params: &[(String, String)],
        ) -> Result<Vec<u8>, Error> {
            let query = c_string("the statement", sql)?;
            let format = c_string("the format", format)?;
            let params = Params::of(params)?;

            // SAFETY: the strings and the pointer and length vectors are live for
            // the call, and `count` is each vector's length.
            let result = if params.is_empty() {
                unsafe {
                    chdb_query_with_params_n(
                        self.handle(),
                        query.as_ptr(),
                        query.as_bytes().len(),
                        format.as_ptr(),
                        format.as_bytes().len(),
                        ptr::null(),
                        ptr::null(),
                        ptr::null(),
                        ptr::null(),
                        0,
                    )
                }
            } else {
                unsafe {
                    chdb_query_with_params_n(
                        self.handle(),
                        query.as_ptr(),
                        query.as_bytes().len(),
                        format.as_ptr(),
                        format.as_bytes().len(),
                        params.names.as_ptr(),
                        params.name_lengths.as_ptr(),
                        params.values.as_ptr(),
                        params.value_lengths.as_ptr(),
                        params.count(),
                    )
                }
            };
            if result.is_null() {
                return Err(Error::new(format!("{sql}: chdb_query returned no result")));
            }
            // SAFETY: the result is live and owned here.
            unsafe { Self::finish(result) }
        }

        /// Runs a statement that produces no rows, such as `SET` or `USE`.
        pub fn execute(&self, sql: &str) -> Result<(), Error> {
            self.query(sql, "", &[]).map(|_| ())
        }

        /// Runs a query and streams its result.
        pub fn stream(
            self: &Arc<Self>,
            sql: &str,
            format: &str,
            params: &[(String, String)],
        ) -> Result<Stream, Error> {
            let query = c_string("the statement", sql)?;
            let format = c_string("the format", format)?;
            let params = Params::of(params)?;

            // SAFETY: as in `query`, and the handle is stored in the `Stream` that
            // is returned.
            let result = if params.is_empty() {
                unsafe {
                    chdb_stream_query_n(
                        self.handle(),
                        query.as_ptr(),
                        query.as_bytes().len(),
                        format.as_ptr(),
                        format.as_bytes().len(),
                    )
                }
            } else {
                unsafe {
                    chdb_stream_query_with_params_n(
                        self.handle(),
                        query.as_ptr(),
                        query.as_bytes().len(),
                        format.as_ptr(),
                        format.as_bytes().len(),
                        params.names.as_ptr(),
                        params.name_lengths.as_ptr(),
                        params.values.as_ptr(),
                        params.value_lengths.as_ptr(),
                        params.count(),
                    )
                }
            };
            if result.is_null() {
                return Err(Error::new(format!(
                    "{sql}: chdb_stream_query returned no handle"
                )));
            }
            // SAFETY: the handle is live and owned here; an error result is turned
            // into an `Error` and destroyed before returning.
            if let Some(message) = unsafe { Self::result_error(result) } {
                // SAFETY: live and owned here.
                unsafe { chdb_destroy_query_result(result) };
                return Err(Error::new(message));
            }
            Ok(Stream::new(Arc::clone(self), result))
        }

        /// Runs a query and streams its result as Arrow blocks.
        pub fn arrow_stream(self: &Arc<Self>, sql: &str) -> Result<ArrowStream, Error> {
            let query = c_string("the statement", sql)?;
            // SAFETY: the statement is NUL-terminated and live for the call, and
            // the handle is stored in the `ArrowStream` that is returned.
            let handle = unsafe {
                chdb_stream_query_arrow_n(
                    self.handle(),
                    query.as_ptr(),
                    query.as_bytes().len(),
                    ptr::null(),
                )
            };
            if handle.is_null() {
                return Err(Error::new(format!(
                    "{sql}: chdb_stream_query_arrow returned no handle"
                )));
            }
            // SAFETY: the handle is live and owned here; an error result is turned
            // into an `Error` and destroyed before returning.
            if let Some(message) = unsafe { Self::result_error(handle) } {
                // SAFETY: live and owned here.
                unsafe { chdb_destroy_query_result(handle) };
                return Err(Error::new(message));
            }
            Ok(ArrowStream {
                conn: Arc::clone(self),
                handle: AtomicPtr::new(handle),
            })
        }

        /// Registers an exported Arrow stream as a table.
        ///
        /// The stream stays owned by the caller, which must keep it alive for as
        /// long as the table can be read: the engine pulls batches out of it
        /// through the cell's `internal_data` for the whole time.
        pub fn scan_arrow(&self, name: &str, stream: &FFI_ArrowArrayStream) -> Result<(), Error> {
            let table = c_string("the table name", name)?;
            // The cell is on the heap, not this frame's stack: the header says the
            // cell is the caller's, and a table that can be read for as long as it
            // is registered may keep it. Task 1 measured no crash from a stack cell
            // and no hang from a heap one, so the cell is not what makes
            // `chdb_arrow_scan` unusable — but a dangling pointer would be a
            // different kind of failure.
            let mut cell = Box::new(chdb_arrow_stream_ {
                internal_data: (stream as *const FFI_ArrowArrayStream) as *mut std::ffi::c_void,
            });
            // SAFETY: the stream is alive and owned by the caller for at least as
            // long as the table is registered, which the documentation of this
            // method requires, and the cell below points at it for the call.
            let state = unsafe { chdb_arrow_scan(self.handle(), table.as_ptr(), &mut *cell) };
            if state != CHDBSuccess {
                return Err(Error::new(format!(
                    "chdb_arrow_scan refused the table {name} (state {state})"
                )));
            }
            Ok(())
        }

        /// Unregisters a table registered with [`Connection::scan_arrow`].
        pub fn unregister_arrow(&self, name: &str) -> Result<(), Error> {
            let table = c_string("the table name", name)?;
            // SAFETY: the connection is open and the name is live for the call.
            let state = unsafe { chdb_arrow_unregister_table(self.handle(), table.as_ptr()) };
            if state != CHDBSuccess {
                return Err(Error::new(format!(
                    "chdb_arrow_unregister_table refused the table {name} (state {state})"
                )));
            }
            Ok(())
        }
    }

    /// A statement's parameters, as the ABI wants them: four parallel vectors of
    /// pointers and lengths, over strings this value owns.
    struct Params {
        /// The strings, kept alive so the pointers stay valid.
        _owned: Vec<CString>,
        names: Vec<*const std::ffi::c_char>,
        name_lengths: Vec<usize>,
        values: Vec<*const std::ffi::c_char>,
        value_lengths: Vec<usize>,
    }

    #[cfg(test)]
    mod params_tests {
        use super::Params;

        /// Each name goes with its own value (HS1 Task 5 fix round 1: with two
        /// parameters the names were `[n0, v0]` and the values `[n1, v1]`, so
        /// `{t:Identifier}` next to another parameter was "not set").
        #[test]
        fn each_name_goes_with_its_value() {
            let pairs = [
                ("u".to_string(), "http://x/".to_string()),
                ("t".to_string(), "system.one".to_string()),
                ("n".to_string(), String::new()),
            ];
            let params = Params::of(&pairs).expect("params");
            assert_eq!(params.count(), 3);
            let read = |pointer: *const std::ffi::c_char| {
                // SAFETY: every pointer is into a `CString` that `params` owns
                // and keeps alive for this call.
                unsafe { std::ffi::CStr::from_ptr(pointer) }
                    .to_string_lossy()
                    .into_owned()
            };
            for (at, (name, value)) in pairs.iter().enumerate() {
                assert_eq!(&read(params.names[at]), name);
                assert_eq!(params.name_lengths[at], name.len());
                assert_eq!(&read(params.values[at]), value);
                assert_eq!(params.value_lengths[at], value.len());
            }
        }
    }

    impl Params {
        fn of(params: &[(String, String)]) -> Result<Self, Error> {
            // Every name, then every value: the two halves are the ABI's two
            // vectors, in the same order.
            let mut owned = Vec::with_capacity(params.len() * 2);
            for (name, _) in params {
                owned.push(c_string("a parameter name", name)?);
            }
            for (_, value) in params {
                owned.push(c_string("a parameter value", value)?);
            }
            let (names, values) = owned.split_at(params.len());
            Ok(Self {
                names: names.iter().map(|name| name.as_ptr()).collect(),
                name_lengths: names.iter().map(|name| name.as_bytes().len()).collect(),
                values: values.iter().map(|value| value.as_ptr()).collect(),
                value_lengths: values.iter().map(|value| value.as_bytes().len()).collect(),
                _owned: owned,
            })
        }

        fn count(&self) -> usize {
            self.names.len()
        }

        fn is_empty(&self) -> bool {
            self.names.is_empty()
        }
    }

    /// A streaming `INSERT`: `chdb_stream_insert_n`, `chdb_stream_append`,
    /// `chdb_stream_done` (HS1 Task 2, Ruling R1.7).
    ///
    /// HS1 Task 1 measured that the pinned library takes `Native` and `Parquet`
    /// bodies appended in arbitrary, non-block-aligned chunks with no temporary
    /// file, and refuses `INSERT … SELECT … FROM input()` as the stream's statement.
    /// The header says the connection accepts no other statement while the stream
    /// is open, so the stream borrows the connection and the caller keeps it busy.
    ///
    /// The handle is destroyed exactly once, on drop, as the header requires "for
    /// every handle, even on error paths"; `chdb_destroy_insert_stream` cancels a
    /// stream that was never finished, so dropping one without `finish` inserts
    /// nothing (ClickHouse's semantics: blocks already flushed stay).
    #[derive(Debug)]
    pub struct InsertStream {
        _conn: Arc<Connection>,
        handle: NonNull<chdb_insert_stream_>,
    }

    // SAFETY: the handle is used from one thread at a time (`&mut self`), and the
    // library does not tie it to the thread that made it.
    unsafe impl Send for InsertStream {}

    /// What a finished `INSERT` reports.
    #[derive(Clone, Copy, Debug, Default, PartialEq)]
    pub struct InsertSummary {
        /// Rows written, per `chdb_result_rows_written`.
        pub rows: u64,
        /// Bytes written, per `chdb_result_bytes_written`.
        pub bytes: u64,
        /// Seconds, per `chdb_result_elapsed`.
        pub elapsed: f64,
    }

    impl InsertStream {
        /// The stream's error text, if the library recorded one.
        fn error(&self) -> Option<String> {
            // SAFETY: the handle is live until drop; the string is copied at once.
            unsafe { c_str_to_string(chdb_stream_insert_error(self.handle.as_ptr())) }
        }

        /// Appends one chunk of the body. The bytes are copied by the library.
        pub fn append(&mut self, data: &[u8]) -> Result<(), Error> {
            if data.is_empty() {
                return Ok(());
            }
            // SAFETY: the handle is live; `data` is valid for `data.len()` bytes
            // for the call, and the header says the library copies it.
            let state = unsafe {
                chdb_stream_append(
                    self.handle.as_ptr(),
                    data.as_ptr() as *const c_void,
                    data.len(),
                )
            };
            if state == CHDBSuccess {
                return Ok(());
            }
            Err(Error::new(self.error().unwrap_or_else(|| {
                "chdb_stream_append failed and recorded no error".to_string()
            })))
        }

        /// Ends the body and commits it.
        pub fn finish(self) -> Result<InsertSummary, Error> {
            // SAFETY: the handle is live; the result is owned here and destroyed
            // below, and the handle itself is destroyed on drop.
            let result = unsafe { chdb_stream_done(self.handle.as_ptr()) };
            if result.is_null() {
                return Err(Error::new(self.error().unwrap_or_else(|| {
                    "chdb_stream_done returned no result".to_string()
                })));
            }
            // SAFETY: a live result, read and then destroyed once.
            let (error, summary) = unsafe {
                (
                    Connection::result_error(result),
                    InsertSummary {
                        rows: chdb_result_rows_written(result),
                        bytes: chdb_result_bytes_written(result),
                        elapsed: chdb_result_elapsed(result),
                    },
                )
            };
            // SAFETY: as above; not touched again.
            unsafe { chdb_destroy_query_result(result) };
            match error.or_else(|| self.error()) {
                Some(message) => Err(Error::new(message)),
                None => Ok(summary),
            }
        }
    }

    impl Drop for InsertStream {
        fn drop(&mut self) {
            // SAFETY: the handle came from `chdb_stream_insert_n` and is destroyed
            // only here. The header: "Required for every handle, even on error
            // paths; cancels first if not finalized".
            unsafe { chdb_destroy_insert_stream(self.handle.as_ptr()) };
        }
    }

    impl Connection {
        /// Begins a streaming `INSERT`: `insert` is the statement without `FORMAT`
        /// or data (`INSERT INTO t`), `format` the body's format.
        pub fn insert_stream(
            self: &Arc<Self>,
            insert: &str,
            format: &str,
        ) -> Result<InsertStream, Error> {
            let query = c_string("the statement", insert)?;
            let format = c_string("the format", format)?;
            // SAFETY: both strings are live and NUL-terminated for the call, and
            // the lengths are theirs. The header says the result is never null;
            // it is checked anyway, because a null here would be destroyed below.
            let raw = unsafe {
                chdb_stream_insert_n(
                    self.handle(),
                    query.as_ptr(),
                    query.as_bytes().len(),
                    format.as_ptr(),
                    format.as_bytes().len(),
                )
            };
            let handle = NonNull::new(raw)
                .ok_or_else(|| Error::new(format!("{insert}: chdb_stream_insert returned null")))?;
            let stream = InsertStream {
                _conn: Arc::clone(self),
                handle,
            };
            match stream.error() {
                // Dropping the stream destroys the handle, as the header asks for
                // a failed init too.
                Some(message) => Err(Error::new(message)),
                None => Ok(stream),
            }
        }
    }

    /// What `chdb_classify_query_n` says about a statement (HS1 Task 4, FL2
    /// Ruling 6): its `chdb_query_class`, how many statements it holds, and its
    /// `chdb_query_analysis_flag`s.
    #[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
    pub struct Analysis {
        /// `chdb_query_class`: 0 read-only … 4 unknown.
        pub class: u32,
        /// Executable statements (0 when it does not parse).
        pub statements: u32,
        /// `chdb_query_analysis_flag`s, OR-ed.
        pub flags: u32,
    }

    impl Connection {
        /// Classifies `sql` with ClickHouse's own parser. Text that does not parse
        /// is class 4 (`CHDB_QUERY_UNKNOWN`) with no statements, not an error.
        pub fn classify(&self, sql: &str) -> Result<Analysis, Error> {
            let mut out = chdb_query_analysis_v1 {
                struct_size: std::mem::size_of::<chdb_query_analysis_v1>() as u32,
                statement_count: 0,
                flags: 0,
                query_class: chdb_query_class_CHDB_QUERY_UNKNOWN,
            };
            // SAFETY: `sql` is valid for `sql.len()` bytes for the call (the ABI is
            // length-based, so interior NULs are fine); no target database is
            // passed; `out` is a live, correctly sized struct whose `struct_size`
            // is set as the header requires.
            let state = unsafe {
                chdb_classify_query_n(
                    self.handle(),
                    sql.as_ptr() as *const std::ffi::c_char,
                    sql.len(),
                    ptr::null(),
                    0,
                    &mut out,
                )
            };
            if state != CHDBSuccess {
                return Err(Error::new("chdb_classify_query_n refused the call"));
            }
            Ok(Analysis {
                class: out.query_class,
                statements: out.statement_count,
                flags: out.flags,
            })
        }
    }

    /// One block of a streamed result, with the counters the block carries.
    #[derive(Clone, Debug, Default, PartialEq)]
    pub struct Block {
        /// The block's bytes.
        pub bytes: Vec<u8>,
        /// Rows in the result, per `chdb_result_rows_read`.
        pub rows_read: u64,
        /// Bytes in the result, per `chdb_result_bytes_read`.
        pub bytes_read: u64,
        /// Rows read from storage, per `chdb_result_storage_rows_read`.
        pub storage_rows_read: u64,
        /// Bytes read from storage, per `chdb_result_storage_bytes_read`.
        pub storage_bytes_read: u64,
        /// How long the query has been running, in seconds.
        pub elapsed: f64,
    }

    /// A streaming result: the handle, and the flag a cancellation sets.
    ///
    /// A `Stream` is shared rather than owned by one reader — a cancellation
    /// arrives from another thread — so the handle lives behind an `Arc` and only
    /// one of the two can take it.
    #[derive(Debug)]
    pub struct Stream {
        conn: Arc<Connection>,
        handle: AtomicPtr<chdb_result_>,
        cancelled: AtomicBool,
    }

    impl Stream {
        fn new(conn: Arc<Connection>, handle: *mut chdb_result) -> Self {
            Self {
                conn,
                handle: AtomicPtr::new(handle),
                cancelled: AtomicBool::new(false),
            }
        }

        /// The next block, or `None` at the end of the stream.
        ///
        /// This blocks: chDB produces blocks as the query runs.
        pub fn fetch(&self) -> Result<Option<Block>, Error> {
            let Some(handle) = self.peek() else {
                return Ok(None);
            };
            // SAFETY: the handle is live and owned by this `Stream`, and the block
            // the call returns is destroyed before this returns.
            let block = unsafe { chdb_stream_fetch_result(self.conn.handle(), handle) };
            if block.is_null() {
                return Err(Error::new("chdb_stream_fetch_result returned no block"));
            }
            // SAFETY: the block is live and owned here.
            let counters = unsafe {
                (
                    chdb_result_rows_read(block),
                    chdb_result_bytes_read(block),
                    chdb_result_storage_rows_read(block),
                    chdb_result_storage_bytes_read(block),
                    chdb_result_elapsed(block),
                )
            };
            // SAFETY: as above; the bytes are a copy.
            let error = unsafe { Connection::result_error(block) };
            // SAFETY: as above.
            let bytes = unsafe { Connection::result_bytes(block) };
            // SAFETY: the block is live and owned here, and nothing borrows from
            // it after this.
            unsafe { chdb_destroy_query_result(block) };
            if let Some(message) = error {
                return Err(Error::new(message));
            }
            if bytes.is_empty() {
                return Ok(None);
            }
            Ok(Some(Block {
                bytes,
                rows_read: counters.0,
                bytes_read: counters.1,
                storage_rows_read: counters.2,
                storage_bytes_read: counters.3,
                elapsed: counters.4,
            }))
        }

        /// Records a cancellation.
        pub fn cancel(&self) {
            self.cancelled.store(true, Ordering::SeqCst);
        }

        /// Whether a cancellation has been recorded.
        pub fn is_cancelled(&self) -> bool {
            self.cancelled.load(Ordering::SeqCst)
        }

        /// Takes the handle for a cancellation, if nobody else has.
        ///
        /// This is what keeps a cancellation and a dropped stream from both
        /// destroying the same handle: whichever asks first owns it.
        pub fn claim_for_cancel(&self) -> Option<CancelToken> {
            let handle = self.handle.swap(ptr::null_mut(), Ordering::SeqCst);
            if handle.is_null() {
                return None;
            }
            // SAFETY: the swap returned a pointer this `Stream` owned, so it is
            // live and no other thread can have claimed it.
            Some(CancelToken {
                conn: Arc::clone(&self.conn),
                handle,
            })
        }

        /// The handle, if it has not been taken.
        fn peek(&self) -> Option<*mut chdb_result> {
            let handle = self.handle.load(Ordering::SeqCst);
            if handle.is_null() {
                None
            } else {
                Some(handle as *mut chdb_result)
            }
        }
    }

    impl Drop for Stream {
        fn drop(&mut self) {
            let handle = self.handle.swap(ptr::null_mut(), Ordering::SeqCst);
            if !handle.is_null() {
                // SAFETY: the swap returned a pointer this `Stream` owned, so
                // nothing else can be using it.
                unsafe { chdb_destroy_query_result(handle as *mut chdb_result) };
            }
        }
    }

    /// A streaming result's handle, taken for a cancellation.
    ///
    /// Dropping it destroys the handle, which is also how a query nobody cancelled
    /// is released.
    #[derive(Debug)]
    pub struct CancelToken {
        conn: Arc<Connection>,
        handle: *mut chdb_result_,
    }

    // SAFETY: the handle is libchdb's, and a cancellation is exactly what it is
    // for.
    unsafe impl Send for CancelToken {}
    // SAFETY: as above.
    unsafe impl Sync for CancelToken {}

    impl CancelToken {
        /// Asks the engine to stop the statement, then releases the handle.
        ///
        /// Task 1 measured that this does **not** interrupt a running statement: it
        /// blocks until the statement has finished on its own and only then tears the
        /// stream down, after which a fetch says `"No active streaming query"`. So a
        /// caller that wants to answer at once has to run this somewhere else.
        pub fn cancel_and_release(mut self) {
            // SAFETY: this token owns the handle, and nothing else refers to it.
            unsafe {
                chdb_stream_cancel_query(self.conn.handle(), self.handle as *mut chdb_result)
            };
            self.release();
        }

        /// Destroys the handle without asking the engine anything.
        fn release(&mut self) {
            if !self.handle.is_null() {
                // SAFETY: the handle is this token's and is destroyed once.
                unsafe { chdb_destroy_query_result(self.handle as *mut chdb_result) };
                self.handle = ptr::null_mut();
            }
        }
    }

    impl Drop for CancelToken {
        fn drop(&mut self) {
            self.release();
        }
    }

    /// An Arrow result, streamed one block at a time.
    #[derive(Debug)]
    pub struct ArrowStream {
        conn: Arc<Connection>,
        handle: AtomicPtr<chdb_result_>,
    }

    impl ArrowStream {
        /// The next block, or `None` at the end of the stream.
        ///
        /// The returned reader owns the C stream chDB produced, and releases it
        /// when it drops.
        pub fn fetch(&self) -> Result<Option<ArrowBlockReader>, Error> {
            let handle = self.handle.load(Ordering::SeqCst);
            if handle.is_null() {
                return Ok(None);
            }
            let mut cell = Box::new(chdb_arrow_stream_ {
                internal_data: ptr::null_mut(),
            });
            // SAFETY: the cell is live and owned here for the call; the handle is
            // live and owned by this `ArrowStream`.
            let state = unsafe {
                chdb_stream_fetch_arrow(self.conn.handle(), handle as *mut chdb_result, &mut *cell)
            };
            // SAFETY: the cell is ours to read after the call, and `internal_data`
            // is the stream chDB left in it.
            let stream = cell.internal_data;
            if state != CHDBSuccess || stream.is_null() {
                // The pinned header's end-of-stream contract: the last `get_next`
                // of a stream returns a released array, which arrives as a state
                // other than `CHDBSuccess` with nothing in the cell.
                return Ok(None);
            }
            // SAFETY: `stream` is the `ArrowArrayStream` chDB left in the cell; the
            // header says the right to release it transfers to the caller, which
            // `ArrowBlockReader` holds and uses.
            let raw = NonNull::new(stream)
                .ok_or_else(|| Error::new("chDB reported an Arrow stream with a null address"))?;
            ArrowBlockReader::new(raw).map(Some)
        }

        /// The counters the statement handle carries.
        pub fn stats(&self) -> Block {
            let handle = self.handle.load(Ordering::SeqCst);
            if handle.is_null() {
                return Block::default();
            }
            // SAFETY: the handle is live and owned by this `ArrowStream`.
            unsafe {
                Block {
                    rows_read: chdb_result_rows_read(handle as *mut chdb_result),
                    bytes_read: chdb_result_bytes_read(handle as *mut chdb_result),
                    storage_rows_read: chdb_result_storage_rows_read(handle as *mut chdb_result),
                    storage_bytes_read: chdb_result_storage_bytes_read(handle as *mut chdb_result),
                    elapsed: chdb_result_elapsed(handle as *mut chdb_result),
                    ..Block::default()
                }
            }
        }
    }

    impl Drop for ArrowStream {
        fn drop(&mut self) {
            let handle = self.handle.swap(ptr::null_mut(), Ordering::SeqCst);
            if !handle.is_null() {
                // SAFETY: the swap returned a pointer this stream owned.
                unsafe { chdb_destroy_query_result(handle as *mut chdb_result) };
            }
        }
    }

    /// One block of an Arrow result: chDB's `ArrowArrayStream`, read in place.
    ///
    /// # Why this does not use `ArrowArrayStreamReader`
    ///
    /// arrow-rs's own importer copies the `ArrowArrayStream` into Rust and calls
    /// the callbacks with the address of that copy. chDB allocated the stream
    /// itself and puts its address in the cell, and Task 1 measured that driving
    /// such a stream through a copy **segfaults**: the callbacks are given a
    /// pointer they do not recognise. This type therefore reads the four callbacks
    /// out of the stream and calls each one with the address chDB put in the cell,
    /// which is what the C data interface means by it.
    ///
    /// On v26.9.0 that call does not return either — see `tests/arrow.rs` — but a
    /// call that never returns is recoverable by a caller, and one that jumps to a
    /// bad address is not.
    #[derive(Debug)]
    pub struct ArrowBlockReader {
        /// chDB's own `ArrowArrayStream`, released when this drops.
        raw: NonNull<c_void>,
        schema: arrow::datatypes::SchemaRef,
        /// The stream's callbacks, read out of the stream itself.
        get_next: unsafe extern "C" fn(*mut FFI_ArrowArrayStream, *mut FFI_ArrowArray) -> i32,
        get_last_error:
            Option<unsafe extern "C" fn(*mut FFI_ArrowArrayStream) -> *const std::ffi::c_char>,
        release: Option<unsafe extern "C" fn(*mut FFI_ArrowArrayStream)>,
        done: AtomicBool,
    }

    // SAFETY: the reader holds chDB's stream, whose callbacks are the C data
    // interface's, and the reader mutates only its own `done` flag.
    unsafe impl Send for ArrowBlockReader {}
    // SAFETY: as above, with the `AtomicBool` standing in for the mutable state.
    unsafe impl Sync for ArrowBlockReader {}

    impl ArrowBlockReader {
        fn new(raw: NonNull<c_void>) -> Result<Self, Error> {
            // SAFETY: `raw` is the `ArrowArrayStream` chDB left in the cell, which
            // is a live C data interface for as long as the caller has not released
            // it.
            let stream = unsafe { ptr::read(raw.as_ptr() as *const FFI_ArrowArrayStream) };
            let get_schema = stream
                .get_schema
                .ok_or_else(|| Error::new("chDB's Arrow stream has no get_schema callback"))?;
            let get_next = stream
                .get_next
                .ok_or_else(|| Error::new("chDB's Arrow stream has no get_next callback"))?;

            let mut schema = FFI_ArrowSchema::empty();
            // SAFETY: the callbacks and the schema are live; `raw` is the address
            // chDB gave for them.
            let status =
                unsafe { get_schema(raw.as_ptr() as *mut FFI_ArrowArrayStream, &mut schema) };
            if status != 0 {
                // SAFETY: `schema` is live either way, and the error is a C string
                // while the stream is alive.
                let last = unsafe {
                    last_error(
                        stream.get_last_error,
                        raw.as_ptr() as *mut FFI_ArrowArrayStream,
                    )
                };
                return Err(Error::new(format!(
                    "chDB's Arrow stream refused its schema with {status}{last}"
                )));
            }
            let schema = arrow::datatypes::Schema::try_from(&schema).map_err(|err| {
                Error::new(format!("the Arrow schema could not be imported: {err}"))
            })?;

            Ok(Self {
                raw,
                schema: Arc::new(schema),
                get_next,
                get_last_error: stream.get_last_error,
                release: stream.release,
                done: AtomicBool::new(false),
            })
        }

        /// The block's schema.
        pub fn schema(&self) -> arrow::datatypes::SchemaRef {
            Arc::clone(&self.schema)
        }

        /// The next record batch of the block, or `None` when the block is spent.
        pub fn next_batch(&self) -> Result<Option<arrow::array::RecordBatch>, Error> {
            if self.done.load(Ordering::SeqCst) {
                return Ok(None);
            }
            let mut array = FFI_ArrowArray::empty();
            // SAFETY: the callbacks and the array are live; `raw` is the address
            // chDB gave for them.
            let status = unsafe {
                (self.get_next)(self.raw.as_ptr() as *mut FFI_ArrowArrayStream, &mut array)
            };
            if status != 0 {
                // SAFETY: the stream is alive, so its error string is too.
                let last = unsafe {
                    last_error(
                        self.get_last_error,
                        self.raw.as_ptr() as *mut FFI_ArrowArrayStream,
                    )
                };
                return Err(Error::new(format!(
                    "chDB's Arrow stream failed with {status}{last}"
                )));
            }
            if array.is_released() {
                // The C data interface's end-of-stream signal.
                self.done.store(true, Ordering::SeqCst);
                return Ok(None);
            }
            // SAFETY: `array` was filled in by the producer's own callback, so it is
            // a live C array; the struct data type is what the block's schema says
            // its children are.
            let data = unsafe {
                arrow::ffi::from_ffi_and_data_type(
                    array,
                    arrow::datatypes::DataType::Struct(self.schema.fields().clone()),
                )
            }
            .map_err(|err| Error::new(format!("the Arrow array could not be imported: {err}")))?;
            let rows = data.len();
            // SAFETY: the data was imported as a struct, which is what a record
            // batch's columns are.
            let columns = arrow::array::StructArray::from(data).into_parts().1;
            arrow::array::RecordBatch::try_new_with_options(
                Arc::clone(&self.schema),
                columns,
                &arrow::record_batch::RecordBatchOptions::new().with_row_count(Some(rows)),
            )
            .map(Some)
            .map_err(|err| Error::new(format!("the Arrow batch could not be built: {err}")))
        }
    }

    impl Drop for ArrowBlockReader {
        fn drop(&mut self) {
            // SAFETY: the stream is live and owned by this reader, which is the
            // right the header transfers to the caller.
            if let Some(release) = self.release {
                unsafe { release(self.raw.as_ptr() as *mut FFI_ArrowArrayStream) };
            }
        }
    }

    /// The C string a stream's `get_last_error` reports, if any.
    ///
    /// # Safety
    ///
    /// `callback` and `stream` must be a live pair from a C data interface.
    unsafe fn last_error(
        callback: Option<
            unsafe extern "C" fn(*mut FFI_ArrowArrayStream) -> *const std::ffi::c_char,
        >,
        stream: *mut FFI_ArrowArrayStream,
    ) -> String {
        let Some(callback) = callback else {
            return String::new();
        };
        // SAFETY: as above; the returned string is the stream's own.
        let text = unsafe { callback(stream) };
        // SAFETY: a non-null error string is NUL-terminated and belongs to the stream.
        match unsafe { c_str_to_string(text) } {
            Some(text) => format!(": {text}"),
            None => String::new(),
        }
    }

    /// Exports an Arrow reader through the C stream interface.
    ///
    /// The reader is boxed into the stream, so it is consumed: the returned stream
    /// owns it and releases it when it drops.
    pub fn export_reader(reader: Box<dyn RecordBatchReader + Send>) -> FFI_ArrowArrayStream {
        // `FFI_ArrowArrayStream::new` boxes the reader into the stream's private
        // data and fills in the four callbacks, and the stream's own `Drop` calls
        // `release`, so dropping an exported stream is what frees the reader. That
        // is why [`Connection::scan_arrow`] takes the stream by value and why
        // `loams-chdb`'s handle keeps it.
        FFI_ArrowArrayStream::new(reader)
    }

    /// `chdb_version()`: the chDB release, which is not the ClickHouse version.
    pub fn chdb_version() -> String {
        // SAFETY: the string is a static in the library, so it is read and copied.
        unsafe { c_str_to_string(super::chdb_version()) }.unwrap_or_else(|| "unknown".to_string())
    }

    /// `chdb_set_signal_handlers_enabled`: whether chDB installs its handlers.
    pub fn set_signal_handlers_enabled(enabled: bool) {
        // SAFETY: the call takes an int and sets a process-wide flag.
        unsafe { chdb_set_signal_handlers_enabled(i32::from(enabled)) };
    }
}

/// Taking ownership of the sockets the House worker inherits (HS1 Tasks 2 and 6).
///
/// The worker finds its `hsw1` socket on fd 3 and, in the `netns` sandbox, its
/// forwarder socket on fd 4, both put there by the front's `posix_spawn`; the
/// forwarder then receives one connected socket per connection over fd 4
/// (`SCM_RIGHTS`). Turning a raw fd into an `OwnedFd` is `unsafe` in Rust (the
/// caller asserts that nothing else owns it), and the workspace forbids `unsafe`
/// outside this crate (FL2 Ruling 8), so the calls live here. The adopting calls
/// take no fd argument: there is exactly one fd each may adopt, at most once per
/// process.
pub mod inherited {
    use std::io;
    use std::mem::MaybeUninit;
    use std::os::fd::{AsRawFd, FromRawFd, OwnedFd, RawFd};
    use std::os::unix::net::UnixStream;
    use std::sync::atomic::{AtomicBool, Ordering};

    /// The fd the worker's socket is on (`loams_house_ipc::WORKER_SOCKET_FD`).
    pub const WORKER_SOCKET_FD: RawFd = 3;

    /// The fd the sandboxed worker's forwarder socket is on
    /// (`loams_house_ipc::FORWARDER_SOCKET_FD`).
    pub const FORWARDER_SOCKET_FD: RawFd = 4;

    /// Whether [`take_worker_socket`] has run, successfully or not.
    static TAKEN: AtomicBool = AtomicBool::new(false);

    /// Whether [`take_forwarder_socket`] has run, successfully or not.
    static FORWARDER_TAKEN: AtomicBool = AtomicBool::new(false);

    fn refuse(fd: RawFd, kind: io::ErrorKind, why: impl Into<String>) -> io::Error {
        io::Error::new(kind, format!("fd {fd}: {}", why.into()))
    }

    /// Takes ownership of fd 3, once.
    ///
    /// It must be open, **inherited** (close-on-exec clear: an fd this process
    /// opened itself, as everything Rust and libchdb open is, has it set) and a
    /// socket (`fstat`, `S_ISSOCK`). A second call fails, whatever the first did.
    pub fn take_worker_socket() -> io::Result<OwnedFd> {
        take(WORKER_SOCKET_FD, &TAKEN)
    }

    /// Takes ownership of fd 4, once, on the same terms as
    /// [`take_worker_socket`] (HS1 Task 6).
    pub fn take_forwarder_socket() -> io::Result<OwnedFd> {
        take(FORWARDER_SOCKET_FD, &FORWARDER_TAKEN)
    }

    fn take(fd: RawFd, taken: &AtomicBool) -> io::Result<OwnedFd> {
        if taken.swap(true, Ordering::SeqCst) {
            return Err(refuse(fd, io::ErrorKind::AlreadyExists, "already taken"));
        }
        // SAFETY: `fcntl(F_GETFD)` reads the fd's flags and touches no memory; on
        // a closed fd it answers -1 with EBADF.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFD) };
        if flags < 0 {
            let err = io::Error::last_os_error();
            return Err(refuse(fd, err.kind(), format!("not open: {err}")));
        }
        if flags & libc::FD_CLOEXEC != 0 {
            return Err(refuse(
                fd,
                io::ErrorKind::InvalidInput,
                "close-on-exec is set, so this process opened it; it was not inherited",
            ));
        }
        is_socket(fd)?;
        // SAFETY: the fd is open, inherited across `exec` (close-on-exec clear), so
        // no Rust object in this process owns it, and `taken` makes this the only
        // `OwnedFd` ever made from it here.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    fn is_socket(fd: RawFd) -> io::Result<()> {
        let mut stat = MaybeUninit::<libc::stat>::uninit();
        // SAFETY: `stat` is a properly sized, writable buffer for the call.
        if unsafe { libc::fstat(fd, stat.as_mut_ptr()) } < 0 {
            let err = io::Error::last_os_error();
            return Err(refuse(fd, err.kind(), format!("fstat failed: {err}")));
        }
        // SAFETY: `fstat` returned 0, so it filled the buffer.
        let mode = unsafe { stat.assume_init() }.st_mode;
        if mode & libc::S_IFMT != libc::S_IFSOCK {
            return Err(refuse(
                fd,
                io::ErrorKind::InvalidInput,
                format!("not a socket (mode {mode:o})"),
            ));
        }
        Ok(())
    }

    /// Receives one byte and the one socket that comes with it (`SCM_RIGHTS`)
    /// over `channel`: the forwarder's connection from the front (HS1 Task 6).
    ///
    /// `Ok(None)` is the end of the channel. A message with no descriptor, more
    /// than one, or one that is not a socket is an error, and every descriptor
    /// it carried is closed.
    pub fn receive_socket(channel: &UnixStream) -> io::Result<Option<OwnedFd>> {
        const SPACE: usize = 64;
        let mut byte = [0u8; 1];
        // u64 words: a control buffer must be aligned for `cmsghdr`.
        let mut control = [0u64; SPACE / 8];
        let mut iov = libc::iovec {
            iov_base: byte.as_mut_ptr().cast(),
            iov_len: byte.len(),
        };
        // SAFETY: an all-zero `msghdr` is a valid empty header; the fields set
        // below point at buffers that outlive the call.
        let mut msg: libc::msghdr = unsafe { std::mem::zeroed() };
        msg.msg_iov = &mut iov;
        msg.msg_iovlen = 1;
        msg.msg_control = control.as_mut_ptr().cast();
        msg.msg_controllen = SPACE as _;
        let received = loop {
            // SAFETY: `msg` describes writable buffers of the stated sizes, and
            // `MSG_CMSG_CLOEXEC` marks any received descriptor close-on-exec.
            let n = unsafe { libc::recvmsg(channel.as_raw_fd(), &mut msg, libc::MSG_CMSG_CLOEXEC) };
            if n >= 0 {
                break n;
            }
            let err = io::Error::last_os_error();
            if err.kind() != io::ErrorKind::Interrupted {
                return Err(err);
            }
        };
        // Every descriptor the kernel installed is owned here first, so an
        // error below closes them all.
        let mut fds = Vec::new();
        // SAFETY: the CMSG_* macros walk the control buffer the kernel just
        // filled, within `msg_controllen`.
        let mut cmsg = unsafe { libc::CMSG_FIRSTHDR(&msg) };
        while !cmsg.is_null() {
            // SAFETY: `cmsg` is a header inside the filled control buffer.
            let header = unsafe { &*cmsg };
            if header.cmsg_level == libc::SOL_SOCKET && header.cmsg_type == libc::SCM_RIGHTS {
                // SAFETY: as above; the data length is what the header says.
                let data = unsafe { libc::CMSG_DATA(cmsg) };
                let len = header.cmsg_len as usize - unsafe { libc::CMSG_LEN(0) } as usize;
                for at in 0..len / std::mem::size_of::<RawFd>() {
                    // SAFETY: `at` is within the header's data; the read is
                    // unaligned because `CMSG_DATA` promises no alignment.
                    let fd = unsafe { data.cast::<RawFd>().add(at).read_unaligned() };
                    // SAFETY: the kernel installed this descriptor in this
                    // process for this message; nothing else owns it.
                    fds.push(unsafe { OwnedFd::from_raw_fd(fd) });
                }
            }
            // SAFETY: as for `CMSG_FIRSTHDR`.
            cmsg = unsafe { libc::CMSG_NXTHDR(&msg, cmsg) };
        }
        if received == 0 && fds.is_empty() {
            return Ok(None);
        }
        if msg.msg_flags & libc::MSG_CTRUNC != 0 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "the control message was truncated",
            ));
        }
        if fds.len() != 1 {
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                format!("expected one socket, received {}", fds.len()),
            ));
        }
        let fd = fds.remove(0);
        is_socket(fd.as_raw_fd())?;
        Ok(Some(fd))
    }
}

/// Dropping the House worker's capabilities (HS1 Task 6).
///
/// A worker in the `netns` sandbox makes its own user namespace, which gives it
/// every capability *in that namespace*: it needs `CAP_NET_ADMIN` there to raise
/// `lo`, and nothing after. `capset(2)` has no safe wrapper in the workspace's
/// dependencies, so the call lives here (FL2 Ruling 8).
pub mod capabilities {
    use std::io;

    /// `_LINUX_CAPABILITY_VERSION_3`.
    const VERSION_3: u32 = 0x2008_0522;

    #[repr(C)]
    struct Header {
        version: u32,
        pid: i32,
    }

    #[repr(C)]
    #[derive(Clone, Copy, Default)]
    struct Data {
        effective: u32,
        permitted: u32,
        inheritable: u32,
    }

    /// Clears the effective, permitted and inheritable sets of the calling
    /// thread, and the ambient set. Threads started afterwards inherit the empty
    /// sets; call it while the process has one thread.
    pub fn drop_all() -> io::Result<()> {
        // SAFETY: `prctl(PR_CAP_AMBIENT, PR_CAP_AMBIENT_CLEAR_ALL)` takes no
        // pointers.
        let rc = unsafe {
            libc::prctl(
                libc::PR_CAP_AMBIENT,
                libc::PR_CAP_AMBIENT_CLEAR_ALL,
                0,
                0,
                0,
            )
        };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        let mut header = Header {
            version: VERSION_3,
            pid: 0,
        };
        let data = [Data::default(); 2];
        // SAFETY: `header` and `data` are the version-3 layout `capset` reads
        // (one header, two data words), and both outlive the call.
        let rc =
            unsafe { libc::syscall(libc::SYS_capset, &mut header as *mut Header, data.as_ptr()) };
        if rc != 0 {
            return Err(io::Error::last_os_error());
        }
        Ok(())
    }
}

/// Leaving the House worker's process at once (HS1 Task 2 review I3).
pub mod process {
    /// `_exit(code)`: ends the process now, without `atexit` handlers or static
    /// destructors. libchdb's shutdown path is slow and has hung (FL2 Ruling 11,
    /// §49 §10.1), and a worker whose socket has closed must be gone at once, even
    /// while a statement holds its main thread.
    pub fn exit_now(code: i32) -> ! {
        // SAFETY: `_exit` takes no pointers and never returns; skipping Rust's own
        // cleanup is the point.
        unsafe { libc::_exit(code) }
    }
}

#[cfg(test)]
mod tests {
    //! The header and the binary are pinned separately — the header by review,
    //! the binary by SHA-256 — so this is where a drift between them is caught:
    //! a symbol Task 0 measured on `libchdb.so` that `bindgen` no longer
    //! generates is a missing declaration, and the only way to find that out is
    //! to name it.
    //!
    //! The list is `nm -D --defined-only libchdb.so`, filtered to the `chdb_`
    //! prefix, minus the five exports the pinned header does not declare as part
    //! of the stable ABI:
    //!
    //! - `chdb_streaming_result_error`, `chdb_streaming_fetch_result`,
    //!   `chdb_streaming_cancel_query` and `chdb_destroy_result` are the
    //!   deprecated duplicates that sit inside the header's
    //!   `#ifndef CHDB_NO_DEPRECATED` block, behind `connect_chdb` and
    //!   `query_conn`. They are generated too, and nothing should call them.
    //! - `chdb_adbc_init` is exported by the binary and **declared nowhere in the
    //!   v26.9.0 header**. Task 0 did not count it in the 52 either; it is
    //!   recorded here because a symbol with no declaration is exactly the shape
    //!   of the drift this test exists to catch.

    // The generated declarations are crate-root items, so a child module names
    // them rather than inheriting them.
    use super::*;

    /// Every `chdb_` symbol FL2 Task 0 measured on the pinned `libchdb.so`, with
    /// the address of the binding it must correspond to.
    ///
    /// Naming a binding is the assertion. `bindgen` generated the declaration
    /// under the C symbol's own name — no `#[link_name]`, no mangling — so if a
    /// header edit drops or renames one of these, this stops compiling rather
    /// than quietly losing a call. The addresses are only there so the test has
    /// something to assert on; a function item's address is never null.
    ///
    /// This is a function rather than a `const` because rustc refuses to cast a
    /// function item to an integer during const evaluation.
    fn measured_exports() -> Vec<(&'static str, *const ())> {
        vec![
            ("chdb_arrow_array_scan", chdb_arrow_array_scan as *const ()),
            ("chdb_arrow_scan", chdb_arrow_scan as *const ()),
            (
                "chdb_arrow_unregister_table",
                chdb_arrow_unregister_table as *const (),
            ),
            (
                "chdb_backup_database_n",
                chdb_backup_database_n as *const (),
            ),
            ("chdb_classify_query_n", chdb_classify_query_n as *const ()),
            ("chdb_close_conn", chdb_close_conn as *const ()),
            ("chdb_connect", chdb_connect as *const ()),
            (
                "chdb_destroy_insert_stream",
                chdb_destroy_insert_stream as *const (),
            ),
            (
                "chdb_destroy_query_result",
                chdb_destroy_query_result as *const (),
            ),
            (
                "chdb_insert_arrow_array",
                chdb_insert_arrow_array as *const (),
            ),
            (
                "chdb_insert_arrow_stream",
                chdb_insert_arrow_stream as *const (),
            ),
            ("chdb_query", chdb_query as *const ()),
            ("chdb_query_arrow", chdb_query_arrow as *const ()),
            ("chdb_query_arrow_n", chdb_query_arrow_n as *const ()),
            ("chdb_query_cmdline", chdb_query_cmdline as *const ()),
            ("chdb_query_n", chdb_query_n as *const ()),
            (
                "chdb_query_with_params",
                chdb_query_with_params as *const (),
            ),
            (
                "chdb_query_with_params_n",
                chdb_query_with_params_n as *const (),
            ),
            (
                "chdb_reset_signal_handlers",
                chdb_reset_signal_handlers as *const (),
            ),
            (
                "chdb_restore_database_n",
                chdb_restore_database_n as *const (),
            ),
            ("chdb_result_buffer", chdb_result_buffer as *const ()),
            (
                "chdb_result_bytes_read",
                chdb_result_bytes_read as *const (),
            ),
            (
                "chdb_result_bytes_written",
                chdb_result_bytes_written as *const (),
            ),
            ("chdb_result_elapsed", chdb_result_elapsed as *const ()),
            ("chdb_result_error", chdb_result_error as *const ()),
            ("chdb_result_length", chdb_result_length as *const ()),
            ("chdb_result_rows_read", chdb_result_rows_read as *const ()),
            (
                "chdb_result_rows_written",
                chdb_result_rows_written as *const (),
            ),
            (
                "chdb_result_storage_bytes_read",
                chdb_result_storage_bytes_read as *const (),
            ),
            (
                "chdb_result_storage_rows_read",
                chdb_result_storage_rows_read as *const (),
            ),
            (
                "chdb_set_signal_handlers_enabled",
                chdb_set_signal_handlers_enabled as *const (),
            ),
            ("chdb_shutdown", chdb_shutdown as *const ()),
            ("chdb_stream_append", chdb_stream_append as *const ()),
            (
                "chdb_stream_cancel_insert",
                chdb_stream_cancel_insert as *const (),
            ),
            (
                "chdb_stream_cancel_query",
                chdb_stream_cancel_query as *const (),
            ),
            ("chdb_stream_done", chdb_stream_done as *const ()),
            (
                "chdb_stream_fetch_arrow",
                chdb_stream_fetch_arrow as *const (),
            ),
            (
                "chdb_stream_fetch_result",
                chdb_stream_fetch_result as *const (),
            ),
            ("chdb_stream_insert", chdb_stream_insert as *const ()),
            (
                "chdb_stream_insert_error",
                chdb_stream_insert_error as *const (),
            ),
            ("chdb_stream_insert_n", chdb_stream_insert_n as *const ()),
            (
                "chdb_stream_insert_with_params",
                chdb_stream_insert_with_params as *const (),
            ),
            (
                "chdb_stream_insert_with_params_n",
                chdb_stream_insert_with_params_n as *const (),
            ),
            ("chdb_stream_query", chdb_stream_query as *const ()),
            (
                "chdb_stream_query_arrow",
                chdb_stream_query_arrow as *const (),
            ),
            (
                "chdb_stream_query_arrow_n",
                chdb_stream_query_arrow_n as *const (),
            ),
            (
                "chdb_stream_query_arrow_with_params",
                chdb_stream_query_arrow_with_params as *const (),
            ),
            (
                "chdb_stream_query_arrow_with_params_n",
                chdb_stream_query_arrow_with_params_n as *const (),
            ),
            ("chdb_stream_query_n", chdb_stream_query_n as *const ()),
            (
                "chdb_stream_query_with_params",
                chdb_stream_query_with_params as *const (),
            ),
            (
                "chdb_stream_query_with_params_n",
                chdb_stream_query_with_params_n as *const (),
            ),
            ("chdb_version", chdb_version as *const ()),
        ]
    }

    #[test]
    fn every_measured_export_has_a_binding() {
        let measured = measured_exports();
        assert_eq!(
            measured.len(),
            52,
            "the measured list no longer holds 52 symbols: {} entries",
            measured.len()
        );
        for (name, address) in &measured {
            assert!(
                name.starts_with("chdb_"),
                "{name} is not a chdb_ symbol, and build.rs's allowlist is `chdb_.*`"
            );
            assert!(!address.is_null(), "{name} has no binding");
        }
        let mut names: Vec<&str> = measured.iter().map(|(name, _)| *name).collect();
        names.sort_unstable();
        let before = names.len();
        names.dedup();
        assert_eq!(
            names.len(),
            before,
            "the measured list repeats a symbol name"
        );
    }

    #[test]
    fn deprecated_family_is_generated_but_unused() {
        // The four deprecated `chdb_` exports exist in the binary and in the
        // header's `CHDB_NO_DEPRECATED` block, and they are generated here
        // because `bindgen` reads the block as written. `loams-chdb` must never
        // call them, so this test exists to make deleting them from the
        // bindings a deliberate act rather than an accident of a header edit.
        let deprecated = [
            chdb_streaming_fetch_result as *const (),
            chdb_streaming_result_error as *const (),
            chdb_streaming_cancel_query as *const (),
            chdb_destroy_result as *const (),
        ];
        for address in deprecated {
            assert!(!address.is_null());
        }
    }

    #[test]
    fn header_version_matches_the_pinned_release() {
        // `CHDB_VERSION` is the header's own macro, and the build script pins the
        // same release: a header from another tag would fail here rather than
        // binding declarations the digest-pinned binary does not have.
        assert_eq!(
            CHDB_VERSION,
            b"26.9.0\0".as_slice(),
            "the vendored header is not the header of the pinned release"
        );
    }
}
