//! The request and response rules the app modules share (runtime contract
//! R4 and R11, SDKP §2.1).
//!
//! * **R11, the default namespace.** Every app request carries a `namespace`
//!   field. [`prepare_request`] fills an empty one from the client's default,
//!   leaves an explicit one alone, and refuses a request with neither as
//!   `invalid_argument` before any network call.
//! * **R4, the session.** A read whose request has a `consistency` field and
//!   leaves it unset is sent `at_least` the session's token, in that field;
//!   every token a response carries (`consistency_token`, `token`,
//!   `read_token`, and the `loams-consistency-token` header) merges into the
//!   session ([`record_response`]).
//!
//! Which requests have which field is a fact of the protos. Rust has no
//! runtime reflection over the generated structs, so [`AppRequest`] is
//! implemented here for each app request message; the macro lists are checked
//! by the compiler (a field that is not there does not compile).

use connectrpc::ErrorCode as Code;
use serde::Serialize;

use crate::error::{ErrorKind, LoamsError};
use crate::request::{CONSISTENCY_TOKEN_HEADER, ConsistencySession};

/// What the runtime needs to know about an app request message.
pub trait AppRequest {
    /// The request's `namespace` field, when it has one.
    fn namespace_mut(&mut self) -> Option<&mut String> {
        None
    }

    /// Sets `consistency.at_least = token` when the request has a
    /// `consistency` field and the caller left it unset. Returns whether it
    /// did.
    fn read_at_least(&mut self, _token: &str) -> bool {
        false
    }
}

macro_rules! namespaced {
    ($($t:path),* $(,)?) => {$(
        impl AppRequest for $t {
            fn namespace_mut(&mut self) -> Option<&mut String> {
                Some(&mut self.namespace)
            }
        }
    )*};
}

macro_rules! collection_reads {
    ($($t:path),* $(,)?) => {$(
        impl AppRequest for $t {
            fn namespace_mut(&mut self) -> Option<&mut String> {
                Some(&mut self.namespace)
            }
            fn read_at_least(&mut self, token: &str) -> bool {
                if self.consistency.is_set() {
                    return false;
                }
                self.consistency = buffa::MessageField::some(
                    loams_proto::loams::collection::v1::Consistency {
                        at_least: token.to_owned(),
                        ..Default::default()
                    },
                );
                true
            }
        }
    )*};
}

macro_rules! graph_reads {
    ($($t:path),* $(,)?) => {$(
        impl AppRequest for $t {
            fn namespace_mut(&mut self) -> Option<&mut String> {
                Some(&mut self.namespace)
            }
            fn read_at_least(&mut self, token: &str) -> bool {
                if self.consistency.is_set() {
                    return false;
                }
                self.consistency = buffa::MessageField::some(
                    loams_proto::loams::graph::v1::Consistency {
                        level: Some(
                            loams_proto::loams::graph::v1::__buffa::oneof::consistency::Level::AtLeast(
                                token.to_owned(),
                            ),
                        ),
                        ..Default::default()
                    },
                );
                true
            }
        }
    )*};
}

mod impls {
    use super::AppRequest;
    use loams_proto::loams::collection::v1 as c;
    use loams_proto::loams::graph::v1 as g;
    use loams_proto::loams::sql::v1 as s;
    use loams_proto::loams::stream::v1 as st;

    // `loams.stream.v1` requests carry a namespace and no consistency.
    namespaced!(
        st::CreateStreamRequest,
        st::DescribeStreamRequest,
        st::ListStreamsRequest,
        st::ProduceRequest,
        st::FetchRequest,
        st::ProduceCloudEventsRequest,
        st::FetchCloudEventsRequest,
    );
    namespaced!(
        c::CreateNamespaceRequest,
        c::CreateCollectionRequest,
        c::ListCollectionsRequest,
        c::GetCollectionRequest,
        c::DropCollectionRequest,
        c::AddFieldsRequest,
        c::ListVersionsRequest,
        c::ScanRequest,
        c::UpdateAliasesRequest,
        c::SetHotRequest,
        c::WarmCollectionRequest,
        c::WriteDocumentsRequest,
        g::CreateGraphRequest,
        g::GetGraphRequest,
        g::ListGraphsRequest,
        g::DeleteGraphRequest,
        g::GetSchemaRequest,
        g::RestoreGraphRequest,
        g::ExportGraphRequest,
        g::ImportGraphRequest,
        g::ExecuteBatchRequest,
    );
    // SQL reads take the session's token too (SDKP-17): their consistency is
    // the collection one.
    collection_reads!(s::QueryRequest, s::QueryArrowRequest);
    collection_reads!(
        c::GetDocumentsRequest,
        c::ScrollDocumentsRequest,
        c::CountDocumentsRequest,
        c::DeleteByFilterRequest,
        c::PatchByFilterRequest,
        c::SearchRequest,
    );
    graph_reads!(g::ExecuteRequest, g::ExplainRequest);
}

/// Applies R11 and R4 to one request before its first attempt.
///
/// # Errors
///
/// `invalid_argument` naming `namespace` when the request has a namespace
/// field, leaves it empty, and the client has no default.
pub fn prepare_request<R: AppRequest>(
    request: &mut R,
    default_namespace: Option<&str>,
    session_token: Option<&str>,
    rpc: &str,
) -> Result<(), LoamsError> {
    if let Some(namespace) = request.namespace_mut()
        && namespace.is_empty()
    {
        match default_namespace.filter(|ns| !ns.is_empty()) {
            Some(fallback) => fallback.clone_into(namespace),
            None => {
                return Err(LoamsError::new(
                    ErrorKind::InvalidArgument,
                    Code::InvalidArgument,
                    format!(
                        "{rpc} needs a namespace: pass one, or give the client a default with with_namespace"
                    ),
                )
                .with_rpc(rpc)
                .with_client_reason(crate::error::NAMESPACE_REQUIRED)
                .with_metadata("field", "namespace"));
            }
        }
    }
    if let Some(token) = session_token.filter(|t| !t.is_empty()) {
        request.read_at_least(token);
    }
    Ok(())
}

/// The response fields a consistency token is read from (R4).
pub const TOKEN_FIELDS: [&str; 3] = ["consistencyToken", "token", "readToken"];

/// Every consistency token a response carried, in its JSON form.
#[must_use]
pub fn response_tokens<M: Serialize>(headers: &http::HeaderMap, message: &M) -> Vec<String> {
    let mut tokens = Vec::new();
    if let Ok(serde_json::Value::Object(json)) = serde_json::to_value(message) {
        for field in TOKEN_FIELDS {
            if let Some(serde_json::Value::String(token)) = json.get(field)
                && !token.is_empty()
            {
                tokens.push(token.clone());
            }
        }
    }
    if let Some(token) = headers
        .get(CONSISTENCY_TOKEN_HEADER)
        .and_then(|value| value.to_str().ok())
        .filter(|token| !token.is_empty())
    {
        tokens.push(token.to_owned());
    }
    tokens
}

/// Merges every token a response carried into the session's default scope
/// (R4). Strings that are not `v1:` tokens are ignored by the session itself.
pub fn record_response<M: Serialize>(
    session: &ConsistencySession,
    headers: &http::HeaderMap,
    message: &M,
) {
    record_response_in(session, "", headers, message);
}

/// [`record_response`] into `scope`, the call's proto package (SDKP-17).
pub fn record_response_in<M: Serialize>(
    session: &ConsistencySession,
    scope: &str,
    headers: &http::HeaderMap,
    message: &M,
) {
    for token in response_tokens(headers, message) {
        session.record_in(Some(&token), scope);
    }
}

/// The app packages (R11): their `namespace` is a Loams namespace.
pub const APP_PACKAGES: [&str; 4] = [
    "loams.collection.v1",
    "loams.graph.v1",
    "loams.sql.v1",
    "loams.stream.v1",
];

/// The packages whose reads take the session's token as `at_least` (R4,
/// SDKP-17): collections and SQL. A graph's token names one graph's stream and
/// a Graph read refuses any other, so graph reads never take the session's.
pub const SESSION_PACKAGES: [&str; 2] = ["loams.collection.v1", "loams.sql.v1"];

/// The session token a read in `package` should be held to, if any.
#[must_use]
pub fn session_token_for(session: Option<&ConsistencySession>, package: &str) -> Option<String> {
    if !SESSION_PACKAGES.contains(&package) {
        return None;
    }
    session.and_then(|session| session.current_in(package))
}
