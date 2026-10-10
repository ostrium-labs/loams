//! Capturing a committed transaction's change set from the engine (design §48 §6.3, Q672).
//!
//! GR1 Task 9 measured mechanism B, Grafeo's CDC, and rules it the mechanism (R9.1). The CDC
//! events alone are not a usable record (R0.6): a label event does not say whether a label was
//! added or removed, an edge delete carries no type or endpoints, and the events of one commit come
//! back in hash order. So [`CdcCapture`] uses the events only for *which* elements the commit
//! touched, and reads what each one now is from the store (R9.2).
//!
//! That read is the state at the commit's epoch only because nothing else writes the engine
//! between the commit and the capture: the caller holds the graph's write lane across both (R0.5),
//! and GR1 Task 11 holds the lane's exclusive gate from `begin_transaction` until the change set is
//! durable (R9.3).

use std::collections::{BTreeMap, BTreeSet};

use grafeo::cdc::{ChangeEvent, ChangeKind, EntityId};
use grafeo::{GrafeoDB, Session};
use grafeo_common::types::EpochId;

use super::{ChangeOp, ElementKind, PropMap};
use crate::engine::GraphError;

/// Turns one committed transaction into its change operations (shared contract, GR1 Task 9).
pub trait ChangeCapture {
    /// Prepares a session that is about to begin a write transaction.
    fn begin(&self, session: &mut Session);

    /// The operations of the transaction that committed at `commit_epoch`.
    ///
    /// Called once per commit, under the write lane, before anything else commits.
    ///
    /// # Errors
    ///
    /// [`GraphError::Engine`] when the engine reports a change the capture cannot express (an RDF
    /// triple, or an element kind this version does not know), or when the commit left the graph
    /// inconsistent (an edge to a deleted node, R9.8). The caller treats a failed capture like a
    /// failed append: the commit is not durable, so the graph reloads from the log (§48 §6.2).
    fn take(&self, commit_epoch: u64) -> Result<Vec<ChangeOp>, GraphError>;
}

/// Mechanism B: Grafeo's CDC log, resolved against the store (R9.1, R9.2).
///
/// The database must be opened with `Config::with_cdc()`, and every write session must have CDC on
/// (R0.6 (c)); [`CdcCapture::new`] refuses a database without it.
pub struct CdcCapture<'db> {
    db: &'db GrafeoDB,
}

impl std::fmt::Debug for CdcCapture<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CdcCapture").finish_non_exhaustive()
    }
}

/// What one commit did to one element, folded from its events in timestamp order.
#[derive(Debug, Default)]
struct Touch {
    /// The element was created by this commit.
    created: bool,
    /// A label was added or removed.
    labels_changed: bool,
    /// Property keys set or removed.
    keys: BTreeSet<String>,
}

impl<'db> CdcCapture<'db> {
    /// A capture over `db`.
    ///
    /// # Errors
    ///
    /// [`GraphError::Engine`] when `db` does not record CDC events by default.
    pub fn new(db: &'db GrafeoDB) -> Result<Self, GraphError> {
        if !db.is_cdc_enabled() {
            return Err(GraphError::engine(
                "change capture needs a database opened with CDC enabled",
            ));
        }
        Ok(Self { db })
    }

    /// The commit's events in the order they were recorded.
    ///
    /// `changes_between` groups events by element and sorts them by epoch only, so the order
    /// across elements is the hash map's. Within a process the HLC timestamp is strictly
    /// increasing and assigned as each mutation lands, so it restores the mutation order.
    fn events(&self, epoch: EpochId) -> Result<Vec<ChangeEvent>, GraphError> {
        let mut events = self
            .db
            .changes_between(epoch, epoch)
            .map_err(|err| GraphError::engine(format!("change capture: {err}")))?;
        events.sort_by_key(|event| event.timestamp);
        Ok(events)
    }
}

impl ChangeCapture for CdcCapture<'_> {
    /// Nothing to prepare: a session created on a CDC database records events by default, and the
    /// capture reads the result from the store at [`take`](ChangeCapture::take). Mechanism A would
    /// mark the WAL position here.
    fn begin(&self, _session: &mut Session) {}

    fn take(&self, commit_epoch: u64) -> Result<Vec<ChangeOp>, GraphError> {
        let mut nodes: BTreeMap<u64, Touch> = BTreeMap::new();
        let mut edges: BTreeMap<u64, Touch> = BTreeMap::new();
        for event in self.events(EpochId::new(commit_epoch))? {
            let touch = match event.entity_id {
                EntityId::Node(id) => nodes.entry(id.as_u64()).or_default(),
                EntityId::Edge(id) => edges.entry(id.as_u64()).or_default(),
                other => {
                    return Err(GraphError::engine(format!(
                        "change capture: unsupported element {other:?}"
                    )));
                }
            };
            match event.kind {
                ChangeKind::Create => touch.created = true,
                // A label event carries the node's labels (after an add, before a remove) and no
                // properties (R0.6).
                ChangeKind::Update if event.labels.is_some() => touch.labels_changed = true,
                ChangeKind::Update => {
                    touch
                        .keys
                        .extend(event.before.iter().flat_map(|m| m.keys().cloned()));
                    touch
                        .keys
                        .extend(event.after.iter().flat_map(|m| m.keys().cloned()));
                }
                ChangeKind::Delete => {}
                other => {
                    return Err(GraphError::engine(format!(
                        "change capture: unsupported change {other:?}"
                    )));
                }
            }
        }

        let store = self.db.store();
        let mut upsert_nodes = Vec::new();
        let mut upsert_edges = Vec::new();
        let mut set_props = Vec::new();
        let mut delete_edges = Vec::new();
        let mut delete_nodes = Vec::new();
        for (&id, touch) in &nodes {
            match store.get_node(grafeo::NodeId::new(id)) {
                Some(node) => {
                    let props: PropMap = node
                        .properties
                        .iter()
                        .map(|(k, v)| (k.as_str().to_string(), v.clone()))
                        .collect();
                    if touch.created || touch.labels_changed {
                        let mut labels: Vec<String> =
                            node.labels.iter().map(ToString::to_string).collect();
                        labels.sort();
                        upsert_nodes.push(ChangeOp::UpsertNode { id, labels, props });
                    } else if let Some(op) =
                        set_props_op(id, ElementKind::Node, &touch.keys, &props)
                    {
                        set_props.push(op);
                    }
                }
                None if touch.created => {}
                None => delete_nodes.push(ChangeOp::DeleteNode { id }),
            }
        }
        for (&id, touch) in &edges {
            match store.get_edge(grafeo::EdgeId::new(id)) {
                Some(edge) => {
                    // Grafeo 0.5.43 can commit an edge whose endpoint the same commit deleted
                    // (`DETACH DELETE` of a node whose edge was inserted earlier in the
                    // transaction, R9.8). Such a commit is refused here rather than logged: the
                    // caller reloads the graph from the log, which does not have it.
                    for end in [edge.src, edge.dst] {
                        if store.get_node(end).is_none() {
                            return Err(GraphError::engine(format!(
                                "change capture: the commit left edge {id} pointing at deleted \
                                 node {}",
                                end.as_u64()
                            )));
                        }
                    }
                    let props: PropMap = edge
                        .properties
                        .iter()
                        .map(|(k, v)| (k.as_str().to_string(), v.clone()))
                        .collect();
                    if touch.created {
                        upsert_edges.push(ChangeOp::UpsertEdge {
                            id,
                            ty: edge.edge_type.to_string(),
                            src: edge.src.as_u64(),
                            dst: edge.dst.as_u64(),
                            props,
                        });
                    } else if let Some(op) =
                        set_props_op(id, ElementKind::Edge, &touch.keys, &props)
                    {
                        set_props.push(op);
                    }
                }
                None if touch.created => {}
                None => delete_edges.push(ChangeOp::DeleteEdge { id }),
            }
        }

        let mut ops = upsert_nodes;
        ops.extend(upsert_edges);
        ops.extend(set_props);
        ops.extend(delete_edges);
        ops.extend(delete_nodes);
        Ok(ops)
    }
}

/// The property change of an element that existed before the commit and still does: each key an
/// event named, with its value now, or as removed. `None` when no key was touched. A key set and
/// then restored inside the commit is still written: the value before the commit is not kept.
fn set_props_op(
    id: u64,
    kind: ElementKind,
    keys: &BTreeSet<String>,
    now: &PropMap,
) -> Option<ChangeOp> {
    if keys.is_empty() {
        return None;
    }
    let mut set = PropMap::new();
    let mut removed = Vec::new();
    for key in keys {
        match now.get(key) {
            Some(value) => {
                set.insert(key.clone(), value.clone());
            }
            None => removed.push(key.clone()),
        }
    }
    Some(ChangeOp::SetProps {
        id,
        kind,
        set,
        removed,
    })
}
