//! The operations of the long-running project and branch RPCs (PG2 Task 5):
//! `loams.operations.v1.Operation`s, kept per project at
//! `O/<project_id>/<operation_id>` ([`OperationRec`]).
//!
//! The API service writes an operation `Pending`, in the same batch as the
//! record it acts on. The project's reconciler (Task 7) finds it by listing
//! the project's operations and moves it to `Running`, then `Succeeded` or
//! `Failed`, under its lease.

use ulid::Ulid;

pub use crate::model::{OperationError, OperationKind, OperationRec, OperationState};

use super::{NeonRead, PgService, ServiceError, check_namespace};
use crate::model::OperationKey;
use crate::store::PgControlStore;

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

impl<N: NeonRead> PgService<N> {
    /// An operation of a project in `namespace`.
    ///
    /// # Errors
    ///
    /// `project_not_found` when the project is not in the namespace;
    /// `not_found` when it has no such operation.
    pub async fn get_operation(
        &self,
        namespace: &str,
        project_id: &str,
        operation_id: &str,
    ) -> Result<OperationRec, ServiceError> {
        check_namespace(namespace)?;
        self.project(namespace, project_id).await?;
        if !operation_id.starts_with("op-") || operation_id.contains('/') {
            return Err(ServiceError::invalid(
                "operation_id",
                "an operation id starts with 'op-'",
            ));
        }
        let key = OperationKey {
            project_id: project_id.into(),
            id: operation_id.into(),
        };
        self.store
            .get::<OperationRec>(&key)
            .await?
            .map(|v| v.record)
            .ok_or_else(|| ServiceError::not_found("operation", operation_id))
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
