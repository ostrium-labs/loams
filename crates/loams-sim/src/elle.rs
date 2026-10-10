//! An Elle-style checker for list-append histories (LV1 plan Task 2; Kingsbury
//! and Alvaro, "Elle: Inferring Isolation Anomalies from Experimental
//! Observations", VLDB 2020).
//!
//! **The model.** Each key holds a list. A transaction is a sequence of
//! micro-operations ([`Mop`]): `append(key, value)` adds `value` to the end
//! of the key's list, and `read(key)` returns the whole list. Every value is
//! appended at most once per key, so a read names exactly which writes it
//! saw, and the longest read of a key gives the key's version order (every
//! other read of it must be a prefix of that one).
//!
//! **The graph.** Between committed transactions:
//! - `ww`: `T1` appended the value just before `T2`'s in a key's order;
//! - `wr`: `T2` read a list whose last value (written by someone else) `T1`
//!   appended;
//! - `rw`: `T1` read a list that ends just before `T2`'s value (an
//!   anti-dependency: `T2` overwrote what `T1` saw).
//!
//! **The anomalies.** Snapshot isolation allows a dependency cycle only
//! when two of its `rw` edges are adjacent (Cerone and Gotsman, "Analysing
//! Snapshot Isolation", PODC 2016: a history is snapshot isolated when
//! `(ww ∪ wr) ; rw?` is acyclic). So it forbids [`AnomalyKind::G0`] (only
//! `ww`), [`AnomalyKind::G1c`] (`ww` and `wr`), [`AnomalyKind::GSingle`]
//! (exactly one `rw`) and [`AnomalyKind::GNonadjacent`] (two or more `rw`,
//! no two in a row, the cycle read round), as well as aborted reads
//! ([`AnomalyKind::G1a`]), intermediate reads ([`AnomalyKind::G1b`]), reads
//! that disagree on a key's order and transactions that do not see their
//! own writes. A cycle with two adjacent `rw` edges ([`AnomalyKind::G2`],
//! write skew) is allowed by snapshot isolation and forbidden by
//! serializability, so it is reported separately: [`check_list_append`]
//! checks snapshot isolation and ignores G2, and [`check_serializable`]
//! fails on G2 too.
//!
//! **Outcomes.** An [`Outcome::Ok`] transaction committed and its reads are
//! known. An [`Outcome::Fail`] one did not commit. An [`Outcome::Info`] one
//! may have committed (an unknown outcome): it counts as committed when a
//! read saw one of its values, and its own reads are never used.
//!
//! The checker adds no real-time or process-order edges, so it checks
//! snapshot isolation, not strong (real-time) snapshot isolation.

use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::fmt;

/// A key of a list-append history.
pub type Key = u64;

/// A value appended to a key's list.
pub type Elem = u64;

/// One micro-operation of a transaction.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Mop {
    /// Appends `value` to `key`'s list.
    Append { key: Key, value: Elem },
    /// Reads `key`'s list; `None` when the read's result is unknown (a
    /// transaction that failed or whose outcome is unknown).
    Read { key: Key, list: Option<Vec<Elem>> },
}

impl Mop {
    /// `append(key, value)`.
    pub fn append(key: Key, value: Elem) -> Self {
        Mop::Append { key, value }
    }

    /// `read(key) = list`.
    pub fn read(key: Key, list: impl Into<Vec<Elem>>) -> Self {
        Mop::Read {
            key,
            list: Some(list.into()),
        }
    }

    /// `read(key)` with an unknown result.
    pub fn read_unknown(key: Key) -> Self {
        Mop::Read { key, list: None }
    }
}

/// How a transaction ended.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Outcome {
    /// It committed; its reads are known.
    Ok,
    /// It did not commit.
    Fail,
    /// It may have committed (an unknown outcome).
    Info,
}

/// One transaction of a history.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Op {
    /// The client that ran it (for reports only).
    pub process: u64,
    pub outcome: Outcome,
    pub mops: Vec<Mop>,
}

impl Op {
    /// A committed transaction.
    pub fn ok(process: u64, mops: Vec<Mop>) -> Self {
        Op {
            process,
            outcome: Outcome::Ok,
            mops,
        }
    }

    /// A transaction that did not commit.
    pub fn fail(process: u64, mops: Vec<Mop>) -> Self {
        Op {
            process,
            outcome: Outcome::Fail,
            mops,
        }
    }

    /// A transaction whose outcome is unknown.
    pub fn info(process: u64, mops: Vec<Mop>) -> Self {
        Op {
            process,
            outcome: Outcome::Info,
            mops,
        }
    }
}

/// A list-append history: transactions in any order. An anomaly names a
/// transaction by its index in `ops`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct History {
    pub ops: Vec<Op>,
}

impl History {
    /// An empty history.
    pub fn new() -> Self {
        History::default()
    }

    /// Adds `op`; returns its index.
    pub fn push(&mut self, op: Op) -> usize {
        self.ops.push(op);
        self.ops.len() - 1
    }
}

impl FromIterator<Op> for History {
    fn from_iter<I: IntoIterator<Item = Op>>(iter: I) -> Self {
        History {
            ops: iter.into_iter().collect(),
        }
    }
}

/// What kind of anomaly a history shows.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum AnomalyKind {
    /// A value was appended to a key twice (the history is malformed).
    DuplicateAppend,
    /// A read returned a list with a value twice.
    DuplicateElements,
    /// A read returned a value nobody appended.
    GarbageRead,
    /// Two reads of a key disagree on its order (neither is a prefix of the
    /// other): a lost or reordered append.
    IncompatibleOrder,
    /// A transaction's read does not reflect its own earlier read and
    /// appends.
    Internal,
    /// Aborted read: a read saw a value of a transaction that did not
    /// commit.
    G1a,
    /// Intermediate read: a read ends with a value its writer later
    /// overwrote with another append to the same key.
    G1b,
    /// Write cycle: a cycle of `ww` edges only.
    G0,
    /// Circular information flow: a cycle of `ww` and `wr` edges.
    G1c,
    /// Read skew: a cycle with exactly one `rw` edge.
    GSingle,
    /// A cycle with two or more `rw` edges, no two of them adjacent (the
    /// cycle read round). A closed walk: it may pass a transaction twice.
    GNonadjacent,
    /// Write skew: a cycle with two adjacent `rw` edges. Allowed by
    /// snapshot isolation.
    G2,
}

impl AnomalyKind {
    /// Whether snapshot isolation allows it (only G2 does).
    pub fn allowed_by_snapshot_isolation(self) -> bool {
        self == AnomalyKind::G2
    }
}

/// A dependency between two committed transactions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum Dep {
    Ww,
    Wr,
    Rw,
}

/// One edge of a cycle: `from` → `to` through `key`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Step {
    pub from: usize,
    pub to: usize,
    pub dep: Dep,
    pub key: Key,
}

/// An anomaly: its kind, the transactions involved (indices into
/// [`History::ops`]), the cycle for G0, G1c, G-single and G2, and an
/// explanation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Anomaly {
    pub kind: AnomalyKind,
    pub ops: Vec<usize>,
    pub cycle: Vec<Step>,
    pub detail: String,
}

impl fmt::Display for Anomaly {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}: {}", self.kind, self.detail)?;
        if !self.cycle.is_empty() {
            let steps: Vec<String> = self
                .cycle
                .iter()
                .map(|s| format!("T{} -{:?}({})-> T{}", s.from, s.dep, s.key, s.to))
                .collect();
            write!(f, " [cycle: {}]", steps.join(", "))?;
        }
        Ok(())
    }
}

impl std::error::Error for Anomaly {}

impl Anomaly {
    /// Whether two `rw` edges of the cycle are adjacent (the cycle read
    /// round): the only cycles snapshot isolation allows.
    pub fn has_adjacent_rw(&self) -> bool {
        let n = self.cycle.len();
        (0..n).any(|i| self.cycle[i].dep == Dep::Rw && self.cycle[(i + 1) % n].dep == Dep::Rw)
    }
}

/// Edge counts of the dependency graph (each counted once per pair, kind
/// and key).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Edges {
    pub ww: usize,
    pub wr: usize,
    pub rw: usize,
}

/// Everything [`analyze`] found: at most one anomaly of each kind, the
/// committed transactions and the graph's edge counts.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Analysis {
    pub anomalies: Vec<Anomaly>,
    pub committed: usize,
    pub edges: Edges,
}

impl Analysis {
    /// The first anomaly snapshot isolation forbids.
    pub fn snapshot_isolation(&self) -> Option<&Anomaly> {
        self.anomalies
            .iter()
            .find(|a| !a.kind.allowed_by_snapshot_isolation())
    }

    /// The G2 cycle, if any.
    pub fn g2(&self) -> Option<&Anomaly> {
        self.anomalies.iter().find(|a| a.kind == AnomalyKind::G2)
    }

    /// The anomaly of `kind`, if any.
    pub fn of(&self, kind: AnomalyKind) -> Option<&Anomaly> {
        self.anomalies.iter().find(|a| a.kind == kind)
    }
}

/// Checks that `history` is snapshot isolated: no anomaly but G2.
pub fn check_list_append(history: &History) -> Result<(), Anomaly> {
    match analyze(history).snapshot_isolation() {
        Some(a) => Err(a.clone()),
        None => Ok(()),
    }
}

/// Checks that `history` is serializable: no anomaly at all, G2 included.
pub fn check_serializable(history: &History) -> Result<(), Anomaly> {
    let analysis = analyze(history);
    match analysis.snapshot_isolation().or_else(|| analysis.g2()) {
        Some(a) => Err(a.clone()),
        None => Ok(()),
    }
}

/// Analyzes `history`: every kind of anomaly it shows, once each.
pub fn analyze(history: &History) -> Analysis {
    let mut found = Found::default();
    let ops = &history.ops;

    // Who appended what, and each transaction's last append per key.
    let mut writer: HashMap<(Key, Elem), usize> = HashMap::new();
    let mut last_append: HashMap<(usize, Key), Elem> = HashMap::new();
    for (i, op) in ops.iter().enumerate() {
        for mop in &op.mops {
            if let Mop::Append { key, value } = *mop {
                if let Some(&first) = writer.get(&(key, value)) {
                    found.add(
                        AnomalyKind::DuplicateAppend,
                        vec![first, i],
                        format!("T{first} and T{i} both append {value} to key {key}"),
                    );
                } else {
                    writer.insert((key, value), i);
                }
                last_append.insert((i, key), value);
            }
        }
    }

    // Reads: duplicates, garbage, aborted and intermediate reads; Info
    // transactions that were seen committed; every read's list per key.
    let mut committed: Vec<bool> = ops.iter().map(|op| op.outcome == Outcome::Ok).collect();
    let mut lists: BTreeMap<Key, Vec<(usize, &[Elem])>> = BTreeMap::new();
    for (i, op) in ops.iter().enumerate() {
        if op.outcome != Outcome::Ok {
            continue;
        }
        for mop in &op.mops {
            let Mop::Read {
                key,
                list: Some(list),
            } = mop
            else {
                continue;
            };
            let key = *key;
            lists.entry(key).or_default().push((i, list));
            let mut seen = HashSet::new();
            for &e in list {
                if !seen.insert(e) {
                    found.add(
                        AnomalyKind::DuplicateElements,
                        vec![i],
                        format!("T{i} read key {key} as {list:?}, with {e} twice"),
                    );
                }
                match writer.get(&(key, e)) {
                    None => found.add(
                        AnomalyKind::GarbageRead,
                        vec![i],
                        format!("T{i} read {e} in key {key}, which nobody appended"),
                    ),
                    Some(&w) => match ops[w].outcome {
                        Outcome::Fail => found.add(
                            AnomalyKind::G1a,
                            vec![w, i],
                            format!("T{i} read {e} in key {key}, appended by T{w}, which failed"),
                        ),
                        Outcome::Info => committed[w] = true,
                        Outcome::Ok => {}
                    },
                }
            }
            if let Some(&e) = external(list, key, i, &writer).last()
                && let Some(&w) = writer.get(&(key, e))
                && last_append.get(&(w, key)) != Some(&e)
            {
                found.add(
                    AnomalyKind::G1b,
                    vec![w, i],
                    format!(
                        "T{i} read key {key} as {list:?}, ending in {e}, an intermediate append of T{w}"
                    ),
                );
            }
        }
        internal(op, i, &mut found);
    }

    // Each key's version order: the longest read, of which every other
    // read must be a prefix.
    let mut order: HashMap<Key, &[Elem]> = HashMap::new();
    for (&key, reads) in &lists {
        let Some(&(li, longest)) = reads.iter().max_by_key(|(_, l)| l.len()) else {
            continue;
        };
        for &(i, list) in reads {
            if !longest.starts_with(list) {
                found.add(
                    AnomalyKind::IncompatibleOrder,
                    vec![i, li],
                    format!(
                        "T{i} read key {key} as {list:?} and T{li} as {longest:?}; neither is a prefix of the other"
                    ),
                );
            }
        }
        order.insert(key, longest);
    }

    // The dependency graph between committed transactions.
    let mut graph = Graph::new(ops.len());
    for (&key, &versions) in &order {
        for pair in versions.windows(2) {
            if let (Some(&a), Some(&b)) = (writer.get(&(key, pair[0])), writer.get(&(key, pair[1])))
                && committed[a]
                && committed[b]
            {
                graph.add(a, b, Dep::Ww, key);
            }
        }
    }
    for (&key, reads) in &lists {
        let Some(&versions) = order.get(&key) else {
            continue;
        };
        for &(r, list) in reads {
            let seen = external(list, key, r, &writer);
            if let Some(&e) = seen.last()
                && let Some(&w) = writer.get(&(key, e))
                && committed[w]
            {
                graph.add(w, r, Dep::Wr, key);
            }
            if versions.starts_with(seen)
                && let Some(&next) = versions.get(seen.len())
                && let Some(&w) = writer.get(&(key, next))
                && committed[w]
            {
                graph.add(r, w, Dep::Rw, key);
            }
        }
    }

    // Cycles: G0 (ww), G1c (ww and wr), G-single (one rw) and G2.
    if let Some(cycle) = graph.cycle(&[Dep::Ww]) {
        found.cycle(AnomalyKind::G0, cycle);
    }
    if let Some(cycle) = graph.cycle(&[Dep::Ww, Dep::Wr])
        && cycle.iter().any(|s| s.dep == Dep::Wr)
    {
        found.cycle(AnomalyKind::G1c, cycle);
    }
    if let Some(cycle) = graph.single_rw_cycle() {
        found.cycle(AnomalyKind::GSingle, cycle);
    }
    if let Some(cycle) = graph.nonadjacent_cycle() {
        found.cycle(AnomalyKind::GNonadjacent, cycle);
    }
    if let Some(cycle) = graph.adjacent_rw_cycle() {
        found.cycle(AnomalyKind::G2, cycle);
    }

    Analysis {
        anomalies: found.anomalies.into_values().collect(),
        committed: committed.iter().filter(|c| **c).count(),
        edges: graph.counts,
    }
}

/// What `reader` saw of other transactions in its read `list` of `key`:
/// the list without its own appends at the end.
fn external<'a>(
    list: &'a [Elem],
    key: Key,
    reader: usize,
    writer: &HashMap<(Key, Elem), usize>,
) -> &'a [Elem] {
    let own = list
        .iter()
        .rev()
        .take_while(|e| writer.get(&(key, **e)) == Some(&reader))
        .count();
    &list[..list.len() - own]
}

/// A transaction's reads reflect its own earlier read and appends: after a
/// read of `L` and appends `a…`, the key reads `L ++ a…`; with no earlier
/// read, the list ends with the appends.
fn internal(op: &Op, i: usize, found: &mut Found) {
    let mut state: HashMap<Key, (Option<Vec<Elem>>, Vec<Elem>)> = HashMap::new();
    for mop in &op.mops {
        match mop {
            Mop::Append { key, value } => state.entry(*key).or_default().1.push(*value),
            Mop::Read { key, list } => {
                let Some(list) = list else { continue };
                let (known, own) = state.entry(*key).or_default();
                let fine = match known {
                    Some(prev) => {
                        prev.len() + own.len() == list.len()
                            && list.starts_with(prev)
                            && list.ends_with(own)
                    }
                    None => list.ends_with(own),
                };
                if !fine {
                    found.add(
                        AnomalyKind::Internal,
                        vec![i],
                        format!(
                            "T{i} read key {key} as {list:?} after reading {known:?} and appending {own:?}"
                        ),
                    );
                }
                *known = Some(list.clone());
                own.clear();
            }
        }
    }
}

/// The anomalies found so far, the first of each kind.
#[derive(Default)]
struct Found {
    anomalies: BTreeMap<AnomalyKind, Anomaly>,
}

impl Found {
    fn add(&mut self, kind: AnomalyKind, ops: Vec<usize>, detail: String) {
        self.anomalies.entry(kind).or_insert(Anomaly {
            kind,
            ops,
            cycle: Vec::new(),
            detail,
        });
    }

    fn cycle(&mut self, kind: AnomalyKind, cycle: Vec<Step>) {
        let ops = cycle.iter().map(|s| s.from).collect();
        let rw = cycle.iter().filter(|s| s.dep == Dep::Rw).count();
        let detail = format!(
            "a cycle of {} transactions with {rw} anti-dependency edges",
            cycle.len()
        );
        self.anomalies.entry(kind).or_insert(Anomaly {
            kind,
            ops,
            cycle,
            detail,
        });
    }
}

/// The dependency graph: per transaction, its outgoing edges.
struct Graph {
    out: Vec<Vec<(usize, Dep, Key)>>,
    seen: HashSet<(usize, usize, Dep, Key)>,
    counts: Edges,
}

impl Graph {
    fn new(n: usize) -> Self {
        Graph {
            out: vec![Vec::new(); n],
            seen: HashSet::new(),
            counts: Edges::default(),
        }
    }

    fn add(&mut self, from: usize, to: usize, dep: Dep, key: Key) {
        if from == to || !self.seen.insert((from, to, dep, key)) {
            return;
        }
        self.out[from].push((to, dep, key));
        match dep {
            Dep::Ww => self.counts.ww += 1,
            Dep::Wr => self.counts.wr += 1,
            Dep::Rw => self.counts.rw += 1,
        }
    }

    /// The strongly connected components over edges of `deps` (iterative
    /// Tarjan): each node's component id.
    fn components(&self, deps: &[Dep]) -> Vec<usize> {
        let n = self.out.len();
        let mut index = vec![usize::MAX; n];
        let mut low = vec![0; n];
        let mut on_stack = vec![false; n];
        let mut comp = vec![usize::MAX; n];
        let mut stack = Vec::new();
        let mut next = 0;
        let mut comps = 0;
        for root in 0..n {
            if index[root] != usize::MAX {
                continue;
            }
            // (node, position in its edge list)
            let mut work = vec![(root, 0usize)];
            index[root] = next;
            low[root] = next;
            next += 1;
            stack.push(root);
            on_stack[root] = true;
            while let Some(&mut (v, ref mut pos)) = work.last_mut() {
                if let Some(&(w, dep, _)) = self.out[v].get(*pos) {
                    *pos += 1;
                    if !deps.contains(&dep) {
                        continue;
                    }
                    if index[w] == usize::MAX {
                        index[w] = next;
                        low[w] = next;
                        next += 1;
                        stack.push(w);
                        on_stack[w] = true;
                        work.push((w, 0));
                    } else if on_stack[w] {
                        low[v] = low[v].min(index[w]);
                    }
                    continue;
                }
                work.pop();
                if let Some(&(parent, _)) = work.last() {
                    low[parent] = low[parent].min(low[v]);
                }
                if low[v] == index[v] {
                    while let Some(w) = stack.pop() {
                        on_stack[w] = false;
                        comp[w] = comps;
                        if w == v {
                            break;
                        }
                    }
                    comps += 1;
                }
            }
        }
        comp
    }

    /// The shortest path from `from` to `to` over edges of `deps` that stay
    /// in component `within` of `comp`.
    fn path(
        &self,
        from: usize,
        to: usize,
        deps: &[Dep],
        comp: &[usize],
        within: usize,
    ) -> Option<Vec<Step>> {
        let mut parent: HashMap<usize, Step> = HashMap::new();
        let mut queue = VecDeque::from([from]);
        let mut visited = HashSet::from([from]);
        while let Some(v) = queue.pop_front() {
            for &(w, dep, key) in &self.out[v] {
                if !deps.contains(&dep) || comp[w] != within || !visited.insert(w) {
                    continue;
                }
                parent.insert(
                    w,
                    Step {
                        from: v,
                        to: w,
                        dep,
                        key,
                    },
                );
                if w == to {
                    let mut steps = Vec::new();
                    let mut at = to;
                    while at != from {
                        let step = parent[&at];
                        steps.push(step);
                        at = step.from;
                    }
                    steps.reverse();
                    return Some(steps);
                }
                queue.push_back(w);
            }
        }
        None
    }

    /// A cycle over edges of `deps`, if any.
    fn cycle(&self, deps: &[Dep]) -> Option<Vec<Step>> {
        let comp = self.components(deps);
        for v in 0..self.out.len() {
            for &(w, dep, key) in &self.out[v] {
                if deps.contains(&dep)
                    && comp[w] == comp[v]
                    && let Some(back) = self.path(w, v, deps, &comp, comp[v])
                {
                    let mut cycle = vec![Step {
                        from: v,
                        to: w,
                        dep,
                        key,
                    }];
                    cycle.extend(back);
                    return Some(cycle);
                }
            }
        }
        None
    }

    /// A cycle with exactly one `rw` edge: an `rw` edge `a → b` and a path
    /// from `b` back to `a` over `ww` and `wr` edges.
    fn single_rw_cycle(&self) -> Option<Vec<Step>> {
        let all = self.components(&[Dep::Ww, Dep::Wr, Dep::Rw]);
        let rest = [Dep::Ww, Dep::Wr];
        for a in 0..self.out.len() {
            for &(b, dep, key) in &self.out[a] {
                if dep != Dep::Rw || all[a] != all[b] {
                    continue;
                }
                if let Some(back) = self.path(b, a, &rest, &all, all[a]) {
                    let mut cycle = vec![Step {
                        from: a,
                        to: b,
                        dep,
                        key,
                    }];
                    cycle.extend(back);
                    return Some(cycle);
                }
            }
        }
        None
    }

    /// A cycle with two or more `rw` edges and no two adjacent: from each
    /// `rw` edge `a → b`, a search over (transaction, whether the last edge
    /// was `rw`, `rw` edges so far, capped at 2) that never takes an `rw`
    /// edge right after another and reaches `a` over a `ww` or `wr` edge
    /// with two `rw` edges. What it finds is a closed walk, which may pass
    /// a transaction twice; any such walk breaks snapshot isolation (the
    /// relation `(ww ∪ wr) ; rw?` then has a cycle).
    fn nonadjacent_cycle(&self) -> Option<Vec<Step>> {
        let all = self.components(&[Dep::Ww, Dep::Wr, Dep::Rw]);
        for a in 0..self.out.len() {
            for &(b, dep, key) in &self.out[a] {
                if dep != Dep::Rw || all[a] != all[b] {
                    continue;
                }
                let first = Step {
                    from: a,
                    to: b,
                    dep,
                    key,
                };
                // State: (node, last edge was rw, rw edges so far ≤ 2).
                type State = (usize, bool, u8);
                let start: State = (b, true, 1);
                let goal: State = (a, false, 2);
                let mut parent: HashMap<State, (State, Step)> = HashMap::new();
                let mut visited = HashSet::from([start]);
                let mut queue = VecDeque::from([start]);
                while let Some(state @ (v, last_rw, rws)) = queue.pop_front() {
                    for &(w, dep, key) in &self.out[v] {
                        if all[w] != all[a] || (dep == Dep::Rw && last_rw) {
                            continue;
                        }
                        let is_rw = dep == Dep::Rw;
                        let next: State = (w, is_rw, (rws + u8::from(is_rw)).min(2));
                        if !visited.insert(next) {
                            continue;
                        }
                        let step = Step {
                            from: v,
                            to: w,
                            dep,
                            key,
                        };
                        parent.insert(next, (state, step));
                        if next == goal {
                            let mut steps = Vec::new();
                            let mut at = goal;
                            while at != start {
                                let (prev, step) = parent[&at];
                                steps.push(step);
                                at = prev;
                            }
                            steps.push(first);
                            steps.reverse();
                            return Some(steps);
                        }
                        queue.push_back(next);
                    }
                }
            }
        }
        None
    }

    /// A cycle with two adjacent `rw` edges `a → b → c` and a path from `c`
    /// back to `a` (a closed walk, like [`nonadjacent_cycle`]'s).
    ///
    /// [`nonadjacent_cycle`]: Graph::nonadjacent_cycle
    fn adjacent_rw_cycle(&self) -> Option<Vec<Step>> {
        let all_deps = [Dep::Ww, Dep::Wr, Dep::Rw];
        let all = self.components(&all_deps);
        for a in 0..self.out.len() {
            for &(b, dep, key) in &self.out[a] {
                if dep != Dep::Rw || all[a] != all[b] {
                    continue;
                }
                for &(c, dep2, key2) in &self.out[b] {
                    if dep2 != Dep::Rw || all[c] != all[a] {
                        continue;
                    }
                    let back = if c == a {
                        Some(Vec::new())
                    } else {
                        self.path(c, a, &all_deps, &all, all[a])
                    };
                    if let Some(back) = back {
                        let mut cycle = vec![
                            Step {
                                from: a,
                                to: b,
                                dep,
                                key,
                            },
                            Step {
                                from: b,
                                to: c,
                                dep: dep2,
                                key: key2,
                            },
                        ];
                        cycle.extend(back);
                        return Some(cycle);
                    }
                }
            }
        }
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kind(h: &History) -> Option<AnomalyKind> {
        check_list_append(h).err().map(|a| a.kind)
    }

    /// Read skew: T1 sees T2's append to y but not its append to x.
    #[test]
    fn elle_detects_known_g_single() {
        let h: History = [
            Op::ok(0, vec![Mop::append(1, 1), Mop::append(2, 1)]),
            Op::ok(1, vec![Mop::append(1, 2), Mop::append(2, 2)]),
            Op::ok(2, vec![Mop::read(1, [1]), Mop::read(2, [1, 2])]),
            Op::ok(3, vec![Mop::read(1, [1, 2])]),
        ]
        .into_iter()
        .collect();
        let a = check_list_append(&h).expect_err("read skew is flagged");
        assert_eq!(a.kind, AnomalyKind::GSingle, "{a}");
        assert_eq!(a.cycle.len(), 2, "{a}");
        assert_eq!(
            a.cycle.iter().filter(|s| s.dep == Dep::Rw).count(),
            1,
            "{a}"
        );
        let mut ops = a.ops.clone();
        ops.sort_unstable();
        assert_eq!(ops, vec![1, 2], "{a}");
    }

    /// Write skew: each reads both keys empty and appends to one of them.
    /// Snapshot isolation allows it; serializability does not.
    #[test]
    fn elle_detects_known_g2_write_skew() {
        let h: History = [
            Op::ok(
                0,
                vec![Mop::read(1, []), Mop::read(2, []), Mop::append(1, 1)],
            ),
            Op::ok(
                1,
                vec![Mop::read(1, []), Mop::read(2, []), Mop::append(2, 1)],
            ),
            Op::ok(2, vec![Mop::read(1, [1]), Mop::read(2, [1])]),
        ]
        .into_iter()
        .collect();
        let analysis = analyze(&h);
        let g2 = analysis.g2().expect("write skew is G2");
        assert_eq!(g2.cycle.iter().filter(|s| s.dep == Dep::Rw).count(), 2);
        assert!(g2.has_adjacent_rw(), "{g2}");
        assert_eq!(check_list_append(&h), Ok(()), "SI allows write skew");
        let a = check_serializable(&h).expect_err("serializability does not");
        assert_eq!(a.kind, AnomalyKind::G2, "{a}");
    }

    /// A snapshot-isolated history with concurrency, a failed and an
    /// unknown transaction, own-write reads and no cycle.
    #[test]
    fn elle_accepts_known_si_history() {
        let h: History = [
            Op::ok(0, vec![Mop::append(1, 1), Mop::read(1, [1])]),
            Op::ok(1, vec![Mop::read(1, [1]), Mop::append(2, 1)]),
            // Concurrent with 1, which does not see its append to 1.
            Op::ok(2, vec![Mop::read(3, []), Mop::append(1, 2)]),
            Op::ok(
                0,
                vec![
                    Mop::read(1, [1, 2]),
                    Mop::append(1, 3),
                    Mop::read(1, [1, 2, 3]),
                    Mop::append(2, 2),
                ],
            ),
            Op::fail(1, vec![Mop::append(3, 9), Mop::read_unknown(1)]),
            // Never seen: does not count as committed.
            Op::info(2, vec![Mop::append(3, 8)]),
            // Seen: counts as committed.
            Op::info(3, vec![Mop::append(3, 1)]),
            Op::ok(
                1,
                vec![
                    Mop::read(1, [1, 2, 3]),
                    Mop::read(2, [1, 2]),
                    Mop::read(3, [1]),
                ],
            ),
        ]
        .into_iter()
        .collect();
        let analysis = analyze(&h);
        assert_eq!(analysis.anomalies, Vec::new());
        assert_eq!(analysis.committed, 6);
        assert!(analysis.edges.ww > 0 && analysis.edges.wr > 0 && analysis.edges.rw > 0);
        assert_eq!(check_serializable(&h), Ok(()));
    }

    /// T1 -rw-> T2 -wr-> T3 -rw-> T4 -wr-> T1: two anti-dependencies, never
    /// adjacent. Snapshot isolation forbids it (G-nonadjacent).
    #[test]
    fn elle_detects_g_nonadjacent() {
        let h: History = [
            Op::ok(0, vec![Mop::read(1, []), Mop::read(4, [1])]),
            Op::ok(1, vec![Mop::append(1, 1), Mop::append(2, 1)]),
            Op::ok(2, vec![Mop::read(2, [1]), Mop::read(3, [])]),
            Op::ok(3, vec![Mop::append(3, 1), Mop::append(4, 1)]),
            Op::ok(4, vec![Mop::read(1, [1]), Mop::read(3, [1])]),
        ]
        .into_iter()
        .collect();
        let a = check_list_append(&h).expect_err("a non-adjacent cycle is flagged");
        assert_eq!(a.kind, AnomalyKind::GNonadjacent, "{a}");
        assert_eq!(
            a.cycle.iter().filter(|s| s.dep == Dep::Rw).count(),
            2,
            "{a}"
        );
        assert!(!a.has_adjacent_rw(), "{a}");
        assert_eq!(analyze(&h).g2(), None, "not reported as allowed G2");
    }

    /// T1 -rw-> T2 -rw-> T3 -wr-> T1: the two anti-dependencies are
    /// adjacent, so snapshot isolation allows it (G2), serializability not.
    #[test]
    fn elle_accepts_g2_with_adjacent_rw() {
        let h: History = [
            Op::ok(0, vec![Mop::read(1, []), Mop::read(3, [1])]),
            Op::ok(1, vec![Mop::append(1, 1), Mop::read(2, [])]),
            Op::ok(2, vec![Mop::append(2, 1), Mop::append(3, 1)]),
            Op::ok(3, vec![Mop::read(1, [1]), Mop::read(2, [1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(check_list_append(&h), Ok(()));
        let a = check_serializable(&h).expect_err("not serializable");
        assert_eq!(a.kind, AnomalyKind::G2, "{a}");
        assert!(a.has_adjacent_rw(), "{a}");
    }

    #[test]
    fn elle_detects_g0_write_cycle() {
        let h: History = [
            Op::ok(0, vec![Mop::append(1, 1), Mop::append(2, 1)]),
            Op::ok(1, vec![Mop::append(1, 2), Mop::append(2, 2)]),
            Op::ok(2, vec![Mop::read(1, [1, 2])]),
            Op::ok(3, vec![Mop::read(2, [2, 1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&h), Some(AnomalyKind::G0));
    }

    #[test]
    fn elle_detects_g1c_circular_information_flow() {
        let h: History = [
            Op::ok(0, vec![Mop::append(1, 1), Mop::read(2, [1])]),
            Op::ok(1, vec![Mop::append(2, 1), Mop::read(1, [1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&h), Some(AnomalyKind::G1c));
    }

    #[test]
    fn elle_detects_aborted_and_intermediate_reads() {
        let aborted: History = [
            Op::fail(0, vec![Mop::append(1, 1)]),
            Op::ok(1, vec![Mop::read(1, [1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&aborted), Some(AnomalyKind::G1a));

        let intermediate: History = [
            Op::ok(0, vec![Mop::append(1, 1), Mop::append(1, 2)]),
            Op::ok(1, vec![Mop::read(1, [1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&intermediate), Some(AnomalyKind::G1b));
    }

    #[test]
    fn elle_detects_malformed_reads() {
        let lost: History = [
            Op::ok(0, vec![Mop::append(1, 1)]),
            Op::ok(1, vec![Mop::append(1, 2)]),
            Op::ok(2, vec![Mop::append(1, 3)]),
            Op::ok(3, vec![Mop::read(1, [1, 2])]),
            // Lost the append of 2.
            Op::ok(4, vec![Mop::read(1, [1, 3])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&lost), Some(AnomalyKind::IncompatibleOrder));

        let internal: History = [Op::ok(0, vec![Mop::append(1, 1), Mop::read(1, [])])]
            .into_iter()
            .collect();
        assert_eq!(kind(&internal), Some(AnomalyKind::Internal));

        let garbage: History = [Op::ok(0, vec![Mop::read(1, [7])])].into_iter().collect();
        assert_eq!(kind(&garbage), Some(AnomalyKind::GarbageRead));

        let twice: History = [
            Op::ok(0, vec![Mop::append(1, 1)]),
            Op::ok(1, vec![Mop::read(1, [1, 1])]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&twice), Some(AnomalyKind::DuplicateElements));

        let appended_twice: History = [
            Op::ok(0, vec![Mop::append(1, 1)]),
            Op::ok(1, vec![Mop::append(1, 1)]),
        ]
        .into_iter()
        .collect();
        assert_eq!(kind(&appended_twice), Some(AnomalyKind::DuplicateAppend));
    }
}
