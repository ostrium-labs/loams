//! A typed client for the transfer and run API (§85 §6, §11) over a
//! [`Conn`]. It reconnects when the connection drops; retrying a request is
//! the caller's choice ([`ClientError::is_transient`]).

use std::collections::BTreeMap;
use std::io;
use std::sync::Arc;

use bytes::Bytes;
use futures::StreamExt;
use http::{Method, Request, StatusCode};
use serde::Serialize;
use serde::de::DeserializeOwned;
use tokio::sync::Mutex;

use crate::net::{ByteStream, Conn, ConnectOpts, Transport, connect};
use crate::run::{ClaimedRun, RunOutcome, RunView};
use crate::server::{
    AppendReply, ArtifactRequest, ClaimRequest, CompleteRequest, CreateRun, ErrorBody,
    FinishRequest, H_CHUNK_SHA, H_EPOCH, H_LOG_COMMITTED, H_RUN_STATE, LeaseReply, LeaseRequest,
    RenewReply, RenewRequest, RunSummary,
};
use crate::transfer::{ChunkPut, CreateTransfer, TransferSummary, TransferView};

/// Why a client call failed.
#[derive(Debug, thiserror::Error)]
pub enum ClientError {
    /// The server answered with an error.
    #[error("{status}: {} ({})", body.message, body.code)]
    Api { status: StatusCode, body: ErrorBody },
    /// The connection or the body failed.
    #[error("transport: {0}")]
    Io(#[from] io::Error),
    /// The answer could not be decoded.
    #[error("decode: {0}")]
    Decode(String),
}

impl ClientError {
    /// The server's error code, if it answered.
    pub fn code(&self) -> Option<&str> {
        match self {
            ClientError::Api { body, .. } => Some(body.code.as_str()),
            _ => None,
        }
    }

    /// Whether the same request may succeed later (D2431's retry).
    pub fn is_transient(&self) -> bool {
        match self {
            ClientError::Io(_) => true,
            ClientError::Api { status, .. } => {
                status.is_server_error() || *status == StatusCode::TOO_MANY_REQUESTS
            }
            ClientError::Decode(_) => false,
        }
    }
}

pub type ClientResult<T> = Result<T, ClientError>;

/// A log read in progress.
pub struct LogStream {
    /// The committed length when the read started.
    pub committed: u64,
    pub state: String,
    pub bytes: ByteStream,
}

impl std::fmt::Debug for LogStream {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LogStream")
            .field("committed", &self.committed)
            .field("state", &self.state)
            .finish_non_exhaustive()
    }
}

/// The client for one server and namespace.
#[derive(Clone)]
pub struct Client {
    opts: Arc<ConnectOpts>,
    origin: String,
    ns: String,
    token: Arc<String>,
    conn: Arc<Mutex<Option<Conn>>>,
}

impl std::fmt::Debug for Client {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Client")
            .field("origin", &self.origin)
            .field("ns", &self.ns)
            .finish_non_exhaustive()
    }
}

fn enc(seg: &str) -> String {
    // Path segments here are namespace names and ULIDs; encode anything else.
    let mut out = String::new();
    for b in seg.bytes() {
        if b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.' | b'~') {
            out.push(b as char);
        } else {
            out.push_str(&format!("%{b:02X}"));
        }
    }
    out
}

impl Client {
    pub fn new(opts: ConnectOpts, ns: &str, token: &str) -> io::Result<Self> {
        let origin = crate::net::race::parse_base(&opts.base)?.origin();
        Ok(Client {
            opts: Arc::new(opts),
            origin,
            ns: ns.to_string(),
            token: Arc::new(token.to_string()),
            conn: Arc::new(Mutex::new(None)),
        })
    }

    /// The connection, made or remade on demand.
    pub async fn conn(&self) -> io::Result<Conn> {
        let mut g = self.conn.lock().await;
        if let Some(c) = g.as_ref()
            && c.is_open()
        {
            return Ok(c.clone());
        }
        let c = connect(&self.opts).await?;
        *g = Some(c.clone());
        Ok(c)
    }

    /// The transport of the current connection.
    pub async fn transport(&self) -> io::Result<Transport> {
        Ok(self.conn().await?.transport())
    }

    /// Drops the current connection, so the next call reconnects (and races
    /// again).
    pub async fn reset(&self) {
        *self.conn.lock().await = None;
    }

    fn url(&self, path: &str) -> String {
        format!("{}/v1/namespaces/{}{path}", self.origin, enc(&self.ns))
    }

    async fn send(
        &self,
        method: Method,
        path: &str,
        headers: &[(&str, String)],
        body: Option<ByteStream>,
        content_type: Option<&str>,
    ) -> ClientResult<crate::net::Resp> {
        let mut b = Request::builder()
            .method(method)
            .uri(self.url(path))
            .header(
                http::header::AUTHORIZATION,
                format!("Bearer {}", self.token),
            );
        if let Some(ct) = content_type {
            b = b.header(http::header::CONTENT_TYPE, ct);
        }
        for (k, v) in headers {
            b = b.header(*k, v);
        }
        let req = b.body(()).map_err(|e| ClientError::Decode(e.to_string()))?;
        let conn = self.conn().await?;
        match conn.send(req, body).await {
            Ok(r) => Ok(r),
            Err(e) => {
                if !conn.is_open() {
                    self.reset().await;
                }
                Err(e.into())
            }
        }
    }

    async fn collect(resp: crate::net::Resp) -> ClientResult<(StatusCode, Bytes)> {
        let mut body = resp.body;
        let mut out = Vec::new();
        while let Some(b) = body.next().await {
            out.extend_from_slice(&b?);
            if out.len() > 8 << 20 {
                return Err(ClientError::Decode("response over 8 MiB".into()));
            }
        }
        Ok((resp.status, Bytes::from(out)))
    }

    fn check(status: StatusCode, body: &[u8]) -> ClientResult<()> {
        if status.is_success() {
            return Ok(());
        }
        let body = serde_json::from_slice::<ErrorBody>(body).unwrap_or_else(|_| ErrorBody {
            code: "http".into(),
            message: String::from_utf8_lossy(body).into_owned(),
            ..ErrorBody::default()
        });
        Err(ClientError::Api { status, body })
    }

    async fn json<T: DeserializeOwned>(
        &self,
        method: Method,
        path: &str,
        req: Option<&(impl Serialize + ?Sized)>,
        headers: &[(&str, String)],
    ) -> ClientResult<T> {
        let body = match req {
            Some(r) => {
                let data = serde_json::to_vec(r).map_err(|e| ClientError::Decode(e.to_string()))?;
                Some(futures::stream::once(async move { Ok(Bytes::from(data)) }).boxed())
            }
            None => None,
        };
        let resp = self
            .send(method, path, headers, body, req.map(|_| "application/json"))
            .await?;
        let (status, body) = Self::collect(resp).await?;
        Self::check(status, &body)?;
        serde_json::from_slice(&body).map_err(|e| ClientError::Decode(e.to_string()))
    }

    // ------------------------------------------------------------ transfers

    pub async fn create(&self, tid: &str, req: &CreateTransfer) -> ClientResult<TransferView> {
        self.json(Method::PUT, &format!("/transfers/{tid}"), Some(req), &[])
            .await
    }

    pub async fn view(&self, tid: &str) -> ClientResult<TransferView> {
        self.json(Method::GET, &format!("/transfers/{tid}"), None::<&()>, &[])
            .await
    }

    pub async fn list(&self) -> ClientResult<Vec<TransferSummary>> {
        self.json(Method::GET, "/transfers", None::<&()>, &[]).await
    }

    pub async fn lease(&self, tid: &str, holder: &str) -> ClientResult<LeaseReply> {
        let req = LeaseRequest {
            holder: holder.to_string(),
        };
        self.json(
            Method::POST,
            &format!("/transfers/{tid}/lease"),
            Some(&req),
            &[],
        )
        .await
    }

    /// Streams one chunk's bytes up.
    pub async fn put_chunk(
        &self,
        tid: &str,
        i: u32,
        sha: &str,
        epoch: u64,
        body: ByteStream,
    ) -> ClientResult<ChunkPut> {
        let headers = [(H_CHUNK_SHA, sha.to_string()), (H_EPOCH, epoch.to_string())];
        let resp = self
            .send(
                Method::PUT,
                &format!("/transfers/{tid}/chunks/{i}"),
                &headers,
                Some(body),
                Some("application/octet-stream"),
            )
            .await?;
        let (status, body) = Self::collect(resp).await?;
        Self::check(status, &body)?;
        serde_json::from_slice(&body).map_err(|e| ClientError::Decode(e.to_string()))
    }

    /// Opens one committed chunk: its hash and its bytes, still streaming.
    pub async fn get_chunk(&self, tid: &str, i: u32) -> ClientResult<(String, ByteStream)> {
        let resp = self
            .send(
                Method::GET,
                &format!("/transfers/{tid}/chunks/{i}"),
                &[],
                None,
                None,
            )
            .await?;
        if !resp.status.is_success() {
            let (status, body) = Self::collect(resp).await?;
            Self::check(status, &body)?;
            return Err(ClientError::Decode("unreachable".into()));
        }
        let sha = resp
            .headers
            .get(H_CHUNK_SHA)
            .and_then(|v| v.to_str().ok())
            .ok_or_else(|| ClientError::Decode("missing chunk hash".into()))?
            .to_string();
        Ok((sha, resp.body))
    }

    pub async fn complete(
        &self,
        tid: &str,
        epoch: u64,
        file_sha256: &str,
    ) -> ClientResult<TransferView> {
        let req = CompleteRequest {
            file_sha256: file_sha256.to_string(),
            epoch,
        };
        self.json(
            Method::POST,
            &format!("/transfers/{tid}/complete"),
            Some(&req),
            &[],
        )
        .await
    }

    pub async fn abandon(&self, tid: &str) -> ClientResult<TransferView> {
        self.json(
            Method::POST,
            &format!("/transfers/{tid}/abandon"),
            None::<&()>,
            &[],
        )
        .await
    }

    // ------------------------------------------------------------ runs

    pub async fn create_run(
        &self,
        rid: &str,
        job: &str,
        params: BTreeMap<String, String>,
    ) -> ClientResult<RunView> {
        let req = CreateRun {
            job: job.to_string(),
            params,
        };
        self.json(Method::PUT, &format!("/runs/{rid}"), Some(&req), &[])
            .await
    }

    pub async fn run(&self, rid: &str) -> ClientResult<RunView> {
        self.json(Method::GET, &format!("/runs/{rid}"), None::<&()>, &[])
            .await
    }

    pub async fn runs(&self) -> ClientResult<Vec<RunSummary>> {
        self.json(Method::GET, "/runs", None::<&()>, &[]).await
    }

    pub async fn cancel_run(&self, rid: &str) -> ClientResult<RunView> {
        self.json(
            Method::POST,
            &format!("/runs/{rid}/cancel"),
            None::<&()>,
            &[],
        )
        .await
    }

    pub async fn claim(&self, runner: &str, jobs: &[String]) -> ClientResult<Option<ClaimedRun>> {
        let req = ClaimRequest {
            runner: runner.to_string(),
            jobs: jobs.to_vec(),
        };
        // The claim route sits beside the runs, not under them.
        self.json(Method::POST, "/runner/claim", Some(&req), &[])
            .await
    }

    pub async fn renew_run(&self, rid: &str, runner: &str, epoch: u64) -> ClientResult<bool> {
        let req = RenewRequest {
            runner: runner.to_string(),
            epoch,
        };
        let r: RenewReply = self
            .json(Method::POST, &format!("/runs/{rid}/renew"), Some(&req), &[])
            .await?;
        Ok(r.cancel_requested)
    }

    pub async fn append_log(
        &self,
        rid: &str,
        epoch: u64,
        offset: u64,
        data: Bytes,
    ) -> ClientResult<u64> {
        let body = futures::stream::once(async move { Ok(data) }).boxed();
        let resp = self
            .send(
                Method::PUT,
                &format!("/runs/{rid}/log?offset={offset}"),
                &[(H_EPOCH, epoch.to_string())],
                Some(body),
                Some("application/octet-stream"),
            )
            .await?;
        let (status, body) = Self::collect(resp).await?;
        Self::check(status, &body)?;
        let r: AppendReply =
            serde_json::from_slice(&body).map_err(|e| ClientError::Decode(e.to_string()))?;
        Ok(r.committed)
    }

    /// Reads the log from `from`; with `follow`, the stream stays open and
    /// carries new bytes until the run ends (D2442).
    pub async fn read_log(&self, rid: &str, from: u64, follow: bool) -> ClientResult<LogStream> {
        let resp = self
            .send(
                Method::GET,
                &format!("/runs/{rid}/log?from={from}&follow={follow}"),
                &[],
                None,
                None,
            )
            .await?;
        if !resp.status.is_success() {
            let (status, body) = Self::collect(resp).await?;
            Self::check(status, &body)?;
            return Err(ClientError::Decode("unreachable".into()));
        }
        let committed = resp
            .headers
            .get(H_LOG_COMMITTED)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.parse().ok())
            .unwrap_or(from);
        let state = resp
            .headers
            .get(H_RUN_STATE)
            .and_then(|v| v.to_str().ok())
            .unwrap_or_default()
            .to_string();
        Ok(LogStream {
            committed,
            state,
            bytes: resp.body,
        })
    }

    pub async fn add_artifact(
        &self,
        rid: &str,
        epoch: u64,
        name: &str,
        tid: &str,
        size: u64,
    ) -> ClientResult<()> {
        let req = ArtifactRequest {
            name: name.to_string(),
            tid: tid.to_string(),
            size,
            epoch,
        };
        let data = serde_json::to_vec(&req).map_err(|e| ClientError::Decode(e.to_string()))?;
        let body = futures::stream::once(async move { Ok(Bytes::from(data)) }).boxed();
        let resp = self
            .send(
                Method::POST,
                &format!("/runs/{rid}/artifacts"),
                &[],
                Some(body),
                Some("application/json"),
            )
            .await?;
        let (status, body) = Self::collect(resp).await?;
        Self::check(status, &body)
    }

    pub async fn finish_run(
        &self,
        rid: &str,
        epoch: u64,
        outcome: RunOutcome,
        exit_code: Option<i32>,
        message: Option<String>,
    ) -> ClientResult<RunView> {
        let req = FinishRequest {
            outcome,
            exit_code,
            message,
            epoch,
        };
        self.json(
            Method::POST,
            &format!("/runs/{rid}/finish"),
            Some(&req),
            &[],
        )
        .await
    }
}
