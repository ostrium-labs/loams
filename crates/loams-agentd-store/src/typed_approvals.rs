//! The factory loop's typed approvals (design §72 §5.2, D1431; plan FT1, FT0
//! Task 4): one row per approval in `typed_approvals`. A decision is kept, so
//! a tool that needs it can check it later; `use_typed_approval` spends an
//! approved one exactly once.

use rusqlite::{OptionalExtension, Row, params};

use crate::store::{DocsStore, StoreError, store_blocking};

/// One stored typed approval. `state` is `pending`, `approved`, `denied` or
/// `used`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TypedApprovalRow {
    pub id: String,
    pub chat_id: String,
    pub kind: String,
    pub summary: String,
    pub payload_hash: String,
    pub state: String,
    pub created_at: i64,
    pub decided_at: Option<i64>,
}

const COLUMNS: &str = "id, chat_id, kind, summary, payload_hash, state, created_at, decided_at";

fn row(row: &Row<'_>) -> rusqlite::Result<TypedApprovalRow> {
    Ok(TypedApprovalRow {
        id: row.get(0)?,
        chat_id: row.get(1)?,
        kind: row.get(2)?,
        summary: row.get(3)?,
        payload_hash: row.get(4)?,
        state: row.get(5)?,
        created_at: row.get(6)?,
        decided_at: row.get(7)?,
    })
}

impl DocsStore {
    /// Record a new typed approval.
    pub fn put_typed_approval(&self, approval: &TypedApprovalRow) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                &format!("INSERT INTO typed_approvals ({COLUMNS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8)"),
                params![
                    approval.id,
                    approval.chat_id,
                    approval.kind,
                    approval.summary,
                    approval.payload_hash,
                    approval.state,
                    approval.created_at,
                    approval.decided_at,
                ],
            )?;
            Ok(())
        })
    }

    pub fn typed_approval(&self, id: &str) -> Result<Option<TypedApprovalRow>, StoreError> {
        store_blocking(|| {
            Ok(self
                .conn()
                .query_row(
                    &format!("SELECT {COLUMNS} FROM typed_approvals WHERE id = ?1"),
                    params![id],
                    row,
                )
                .optional()?)
        })
    }

    /// Every typed approval still waiting for a person, oldest first.
    pub fn pending_typed_approvals(&self) -> Result<Vec<TypedApprovalRow>, StoreError> {
        store_blocking(|| {
            let conn = self.conn();
            let mut statement = conn.prepare(&format!(
                "SELECT {COLUMNS} FROM typed_approvals WHERE state = 'pending'
                 ORDER BY created_at, rowid"
            ))?;
            let rows = statement.query_map([], row)?;
            Ok(rows.collect::<Result<Vec<_>, _>>()?)
        })
    }

    /// Settle a pending approval as `approved` or `denied`. Answers whether
    /// it was pending.
    pub fn decide_typed_approval(
        &self,
        id: &str,
        state: &str,
        decided_at: i64,
    ) -> Result<bool, StoreError> {
        store_blocking(|| {
            let changed = self.conn().execute(
                "UPDATE typed_approvals SET state = ?2, decided_at = ?3
                 WHERE id = ?1 AND state = 'pending'",
                params![id, state, decided_at],
            )?;
            Ok(changed == 1)
        })
    }

    /// Spend an approved approval: `approved` becomes `used`, once. Answers
    /// whether this call spent it.
    pub fn use_typed_approval(&self, id: &str) -> Result<bool, StoreError> {
        store_blocking(|| {
            let changed = self.conn().execute(
                "UPDATE typed_approvals SET state = 'used' WHERE id = ?1 AND state = 'approved'",
                params![id],
            )?;
            Ok(changed == 1)
        })
    }

    /// Forget a deleted thread's typed approvals.
    pub fn delete_chat_typed_approvals(&self, chat_id: &str) -> Result<(), StoreError> {
        store_blocking(|| {
            self.conn().execute(
                "DELETE FROM typed_approvals WHERE chat_id = ?1",
                params![chat_id],
            )?;
            Ok(())
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn approval(id: &str) -> TypedApprovalRow {
        TypedApprovalRow {
            id: id.into(),
            chat_id: "chat".into(),
            kind: "merge".into(),
            summary: "Merge #7".into(),
            payload_hash: "ab".into(),
            state: "pending".into(),
            created_at: 1,
            decided_at: None,
        }
    }

    #[test]
    fn an_approval_is_decided_once_and_used_once() {
        let dir = tempfile::tempdir().unwrap();
        let store = DocsStore::open(dir.path()).unwrap();
        store.put_typed_approval(&approval("a")).unwrap();
        store.put_typed_approval(&approval("b")).unwrap();
        assert_eq!(store.pending_typed_approvals().unwrap().len(), 2);
        assert!(
            !store.use_typed_approval("a").unwrap(),
            "pending is not spendable"
        );
        assert!(store.decide_typed_approval("a", "approved", 5).unwrap());
        assert!(
            !store.decide_typed_approval("a", "denied", 6).unwrap(),
            "decided once"
        );
        assert!(store.use_typed_approval("a").unwrap());
        assert!(!store.use_typed_approval("a").unwrap(), "used once");
        let a = store.typed_approval("a").unwrap().unwrap();
        assert_eq!((a.state.as_str(), a.decided_at), ("used", Some(5)));
        assert_eq!(store.pending_typed_approvals().unwrap().len(), 1);
        store.delete_chat_typed_approvals("chat").unwrap();
        assert!(store.typed_approval("b").unwrap().is_none());
    }
}
