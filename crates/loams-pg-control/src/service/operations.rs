//! The operations of the long-running project and branch RPCs (PG2 Task 5):
//! `loams.operations.v1.Operation`s, kept per project at
//! `O/<project_id>/<operation_id>` ([`OperationRec`]).
//!
//! The API service writes an operation `Pending`, with its indexes
//! (`Q/i/<id>` for `GetOperation`, `Q/n/<ns>/<id>` for `ListOperations` and
//! `WatchOperations`), in the same batch as the record it acts on. The project's reconciler (Task 7) finds it by listing
//! the project's operations and moves it to `Running`, then `Succeeded` or
//! `Failed`, under its lease.

use ulid::Ulid;

pub use crate::model::{OperationError, OperationKind, OperationRec, OperationState};

use super::{NeonRead, PgService, ServiceError, check_namespace, list_error, page};
use crate::model::{
    NamespaceOperationRec, NamespaceOperations, OperationIdKey, OperationIdRec, OperationKey,
};
use crate::store::{Batch, PgControlStore, StoreError};

/// A new operation id: "op-" and 26 lower-case hex characters (D146), the
/// first 104 bits of a fresh ULID (time first, so ids sort by creation).
pub fn new_operation_id() -> String {
    let hex = format!("{:032x}", Ulid::generate().0);
    format!("op-{}", &hex[..26])
}

/// A `Pending` operation of `kind` on a project (and a branch).
pub(crate) fn pending(
    kind: OperationKind,
    namespace: &str,
    project_id: &str,
    branch_id: Option<&str>,
    now_ms: u64,
) -> OperationRec {
    OperationRec {
        id: new_operation_id(),
        kind,
        namespace: namespace.into(),
        project_id: project_id.into(),
        branch_id: branch_id.map(Into::into),
        state: OperationState::Pending,
        error: None,
        created_at_ms: now_ms,
        updated_at_ms: now_ms,
    }
}

/// Adds `op` and its two indexes (by id, and in its namespace) to
/// `batch`, all created; returns the operation's index.
pub(crate) fn add_operation(batch: &mut Batch, op: &OperationRec) -> Result<usize, StoreError> {
    let at = batch.put(op, None)?;
    batch.put(
        &OperationIdRec {
            id: op.id.clone(),
            namespace: op.namespace.clone(),
            project_id: op.project_id.clone(),
        },
        None,
    )?;
    batch.put(
        &NamespaceOperationRec {
            namespace: op.namespace.clone(),
            id: op.id.clone(),
            project_id: op.project_id.clone(),
        },
        None,
    )?;
    Ok(at)
}

impl<N: NeonRead> PgService<N> {
    /// `GetOperation`: an operation by its id alone, through `Q/i/`. The
    /// caller (Task 9) authorizes it by its `namespace` and project.
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a malformed id; `not_found`.
    pub async fn get_operation(&self, operation_id: &str) -> Result<OperationRec, ServiceError> {
        if !operation_id.starts_with("op-") || operation_id.contains('/') {
            return Err(ServiceError::invalid(
                "operation_id",
                "an operation id starts with 'op-'",
            ));
        }
        let missing = || ServiceError::not_found("operation", operation_id);
        let index = self
            .store
            .get::<OperationIdRec>(&OperationIdKey {
                id: operation_id.into(),
            })
            .await?
            .ok_or_else(missing)?;
        let key = OperationKey {
            project_id: index.record.project_id,
            id: operation_id.into(),
        };
        self.store
            .get::<OperationRec>(&key)
            .await?
            .map(|v| v.record)
            .ok_or_else(missing)
    }

    /// `ListOperations` for one namespace: a page in id (creation) order,
    /// through `Q/n/<ns>/`, and the next page's token (empty on the last).
    ///
    /// # Errors
    ///
    /// `invalid_argument` for a bad namespace, size or token.
    pub async fn list_operations(
        &self,
        namespace: &str,
        page_size: i32,
        page_token: &str,
    ) -> Result<(Vec<OperationRec>, String), ServiceError> {
        check_namespace(namespace)?;
        let prefix = NamespaceOperations {
            namespace: namespace.into(),
        };
        let (index, next) = self
            .store
            .list::<NamespaceOperationRec>(&prefix, page(page_size, page_token)?)
            .await
            .map_err(list_error)?;
        let mut out = Vec::with_capacity(index.len());
        for entry in index {
            let key = OperationKey {
                project_id: entry.record.project_id,
                id: entry.record.id,
            };
            // An index whose operation was collected meanwhile is skipped.
            if let Some(op) = self.store.get::<OperationRec>(&key).await? {
                out.push(op.record);
            }
        }
        Ok((out, next.unwrap_or_default()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn operation_ids_follow_d146() {
        let id = new_operation_id();
        assert_eq!(id.len(), 3 + 26);
        assert!(
            id[3..]
                .bytes()
                .all(|b| b.is_ascii_hexdigit() && !b.is_ascii_uppercase())
        );
        assert_ne!(new_operation_id(), new_operation_id());
    }
}
