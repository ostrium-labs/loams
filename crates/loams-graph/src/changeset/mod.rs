//! What a committed write transaction changed, in Loams' own terms (design §48 §6.3, §6.4).
//!
//! A [`ChangeOp`] list is the body of a `GraphChangeSet`: the log record that makes a write
//! durable (D742). It names elements by Loams element ids and carries whole values, so a reader of
//! the log needs no Grafeo to understand it (I6). The record envelope and its codec are GR1 Task 10;
//! this module holds the operations, how they are captured from the engine ([`capture`]), and how
//! they are applied to a fresh engine ([`apply_to_store`]), which is what replay, recovery and
//! followers do (Tasks 14 and 15).
//!
//! The operations describe a transaction's **net effect**, not its sequence of mutations. Capture
//! resolves every element the transaction touched against the store as the transaction left it
//! (R9.2), so an element created and deleted inside one transaction produces nothing, and a node
//! whose labels changed is written whole. The order inside one change set is canonical: node
//! upserts, then edge upserts, then property changes, then edge deletes, then node deletes, which
//! is an order that can always be applied (an edge's endpoints exist before it, and a node's edges
//! are gone before it).

pub mod capture;

use std::collections::BTreeMap;

use grafeo::{EdgeId, GrafeoDB, NodeId, Value};

use crate::engine::GraphError;

/// A property map with a stable order, so two equal maps encode to equal bytes (Task 10).
pub type PropMap = BTreeMap<String, Value>;

/// Which kind of element a [`ChangeOp::SetProps`] names.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ElementKind {
    /// A node.
    Node,
    /// An edge (a relationship).
    Edge,
}

/// One change to the graph (shared contract; design §48 §6.4).
///
/// Ids are Loams element ids. Under R9.4 they are the engine's own `NodeId`/`EdgeId` values, kept
/// stable across replay by creating elements with caller-chosen ids.
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeOp {
    /// The node as it now is: exactly these labels and these properties. Creates it if absent.
    UpsertNode {
        /// The node's id.
        id: u64,
        /// Every label, sorted.
        labels: Vec<String>,
        /// Every property.
        props: PropMap,
    },
    /// The node is gone. Its edges are deleted by their own [`ChangeOp::DeleteEdge`]s, earlier in
    /// the same change set.
    DeleteNode {
        /// The node's id.
        id: u64,
    },
    /// The edge as it now is. Creates it if absent; an edge's type and endpoints never change.
    UpsertEdge {
        /// The edge's id.
        id: u64,
        /// The edge's type.
        ty: String,
        /// The source node's id.
        src: u64,
        /// The destination node's id.
        dst: u64,
        /// Every property.
        props: PropMap,
    },
    /// The edge is gone.
    DeleteEdge {
        /// The edge's id.
        id: u64,
    },
    /// Some properties of an existing element changed and its labels did not.
    SetProps {
        /// The element's id.
        id: u64,
        /// Node or edge.
        kind: ElementKind,
        /// Properties set, with their new values.
        set: PropMap,
        /// Properties removed.
        removed: Vec<String>,
    },
}

/// Applies change operations to a graph that is not serving yet, keeping element ids (R0.7, R9.4).
///
/// This writes through Grafeo's store directly (`create_node_with_id`, `create_edge_with_id` and
/// the plain property and label setters). Those calls bypass Grafeo's transactions, its MVCC
/// versions, its CDC log and its own WAL, so they are only for an engine no client reads from while
/// they run: replay into a fresh engine, recovery, or a follower applying under its exclusive gate
/// (R9.3). An engine filled this way must be checkpointed before its local files are trusted
/// (R9.5).
///
/// Operations are applied in order. Replaying a change set twice gives the same graph: an upsert
/// replaces, and a delete of an absent element is a no-op.
///
/// # Errors
///
/// [`GraphError::Engine`] when an operation names an element that must exist and does not (a
/// `SetProps` on a missing element, an edge whose endpoint is missing) or Grafeo refuses an id.
pub fn apply_to_store(db: &GrafeoDB, ops: &[ChangeOp]) -> Result<(), GraphError> {
    let store = db.store();
    for op in ops {
        match op {
            ChangeOp::UpsertNode { id, labels, props } => {
                let node = NodeId::new(*id);
                match store.get_node(node) {
                    None => {
                        let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
                        store.create_node_with_id(node, &labels).map_err(|err| {
                            GraphError::engine(format!("replay: node {id}: {err}"))
                        })?;
                        for (key, value) in props {
                            store.set_node_property(node, key, value.clone());
                        }
                    }
                    Some(existing) => {
                        for label in &existing.labels {
                            if !labels.iter().any(|l| l.as_str() == label.as_str()) {
                                store.remove_label(node, label);
                            }
                        }
                        for label in labels {
                            store.add_label(node, label);
                        }
                        for (key, _) in existing.properties.iter() {
                            if !props.contains_key(key.as_str()) {
                                store.remove_node_property(node, key.as_str());
                            }
                        }
                        for (key, value) in props {
                            store.set_node_property(node, key, value.clone());
                        }
                    }
                }
            }
            ChangeOp::DeleteNode { id } => {
                store.delete_node(NodeId::new(*id));
            }
            ChangeOp::UpsertEdge {
                id,
                ty,
                src,
                dst,
                props,
            } => {
                let edge = EdgeId::new(*id);
                match store.get_edge(edge) {
                    None => {
                        for end in [src, dst] {
                            if store.get_node(NodeId::new(*end)).is_none() {
                                return Err(GraphError::engine(format!(
                                    "replay: edge {id} names missing node {end}"
                                )));
                            }
                        }
                        store
                            .create_edge_with_id(edge, NodeId::new(*src), NodeId::new(*dst), ty)
                            .map_err(|err| {
                                GraphError::engine(format!("replay: edge {id}: {err}"))
                            })?;
                        for (key, value) in props {
                            store.set_edge_property(edge, key, value.clone());
                        }
                    }
                    Some(existing) => {
                        for (key, _) in existing.properties.iter() {
                            if !props.contains_key(key.as_str()) {
                                store.remove_edge_property(edge, key.as_str());
                            }
                        }
                        for (key, value) in props {
                            store.set_edge_property(edge, key, value.clone());
                        }
                    }
                }
            }
            ChangeOp::DeleteEdge { id } => {
                store.delete_edge(EdgeId::new(*id));
            }
            ChangeOp::SetProps {
                id,
                kind,
                set,
                removed,
            } => match kind {
                ElementKind::Node => {
                    let node = NodeId::new(*id);
                    if store.get_node(node).is_none() {
                        return Err(GraphError::engine(format!(
                            "replay: properties of missing node {id}"
                        )));
                    }
                    for key in removed {
                        store.remove_node_property(node, key);
                    }
                    for (key, value) in set {
                        store.set_node_property(node, key, value.clone());
                    }
                }
                ElementKind::Edge => {
                    let edge = EdgeId::new(*id);
                    if store.get_edge(edge).is_none() {
                        return Err(GraphError::engine(format!(
                            "replay: properties of missing edge {id}"
                        )));
                    }
                    for key in removed {
                        store.remove_edge_property(edge, key);
                    }
                    for (key, value) in set {
                        store.set_edge_property(edge, key, value.clone());
                    }
                }
            },
        }
    }
    Ok(())
}
