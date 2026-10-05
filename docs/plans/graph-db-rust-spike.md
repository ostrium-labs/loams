# Graph databases in Rust — what Loams can actually buy

A spike, 2026-10-04, on `research/graph-db-rust`. It answers one question: **§33's catalog has exactly one graph row (Neo4j, P2). Is a hand-written Rust graph connector the right answer, or is there something to buy?** Everything below is measured, not inferred, and the commands are in `§ Measurements`.

## Summary

**Loams cannot read from Neo4j through Camel at all.** `camel-neo4j` is `producerOnly: true` — a sink with no consumer — while §33 A.3's row says `Source Y | Sink Y`. So the row's source direction has no runtime behind it today, and the choice is not "which Rust driver" but "which surface". That reframes the question, and the answer is that **GQL (ISO/IEC 39075) is the surface worth targeting**, with Grafeo as the one implementation found that is Rust-native, Apache-2.0, and speaks it.

## What §33 says today

```
| Neo4j | Y | Y | · | Y | · | · | basic | `neo4j` | `plugin-neo4j` | P2 |
```

One row, category `graph`, P2 (CN2), both directions, batch only, `basic` auth. Under D354 the plan prefers a runtime over native Rust, and the Camel column claims `neo4j`.

## Finding 1: `camel-neo4j` is a sink and nothing else

From `camel-catalog` 4.22.1's own component JSON:

```
neo4j     consumerOnly=False producerOnly=True  supportLevel=Stable  syntax=neo4j:name
arangodb  consumerOnly=False producerOnly=True  supportLevel=Stable  syntax=arangodb:database
```

Both graph components Camel ships are **producer-only**. There is no consumer, so `SELECT … FROM neo4j:name` cannot exist and no route can read a Neo4j graph through Camel. §33's `Source Y` has no runtime behind it. `camel-arangodb` has the same shape, so this is not a Neo4j quirk.

**This is a fourth Appendix A error of the same family as the three CN1's drift check found**, and it needs the same treatment: either the row becomes `Source ·` with a note that reading needs a native connector, or CN1 gains a native Neo4j source and the row stays honest.

## Finding 2: the catalog and the module tree disagree about `camel-neo4j`

`camel-catalog` 4.22.1 lists `neo4j` as a Stable component with `artifactId: camel-neo4j`, `groupId: org.apache.camel`, `firstVersion: 4.10.0`, `javaType: org.apache.camel.component.neo4j.Neo4jComponent`. But `apache/camel` at tag `camel-4.22.1` has **no `components/camel-neo4j` directory** (404), while `components/camel-arangodb` exists.

This is the mirror image of the seven cases CN1's drift check found, where a module existed with no catalog JSON. Here the catalog advertises a component whose module is absent from the tree. Two possibilities: the component moved to a path outside `components/`, or the catalog entry is stale. **Either way `camel_catalog.py`'s severity model should flag it** — "in `camel-catalog` but not in `apache/camel`" is the error branch — and it did not, so the check has a false negative worth fixing. The other graph component, `arangodb`, *is* in the tree, so the check is not simply blind to `components/`.

## Finding 3: the Rust driver landscape, measured

| Crate | Version | Licence | Downloads | Updated | Verdict |
|---|---|---|---|---|---|
| `neo4rs` | 0.8.0 | MIT | 1 225 567 | 2026-06-11 | **Viable.** The healthy option, and licence-clean for D359 |
| `grafeo` | 0.5.43 | **Apache-2.0** | 23 701 | 2026-09-27 | **See below** |
| `falkordb` | 0.10.4 | MIT | 45 117 | 2026-09-23 | Good, but Redis-backed — a different product |
| `kuzu` | 0.11.3 | MIT | 282 927 | 2025-10-10 | Stale by a year; Kuzu was acquired and folded elsewhere |
| `bolt-client` | 0.11.0 | MIT | 49 661 | **2022-12-28** | **Abandoned.** The Bolt wire-protocol crate everyone reaches for is four years cold |
| `surrealdb` | 3.3.0 | **`non-standard`** | 1 600 118 | 2026-09-24 | **Blocked by D359.** crates.io reports `license = "non-standard"`, which is not an SPDX id and cannot be allowlisted |
| `neo4j` | 0.2.0 | Apache-2.0 | 8 454 | 2025-02-21 | Unrelated namesake, 8 k downloads |

The headline is the **Bolt protocol problem**. `bolt-client` — the crate a Rust developer reaches for to speak Bolt to Neo4j, Memgraph and anything else Bolt-compatible — has had no release since December 2022. Its ecosystem (`bolt-proto`, `bolt-client-macros`, `mobc-bolt`) is all in the same freeze. The one maintained Bolt client, `neo4rs`, is **Neo4j-specific**: it is a driver for one server, not a portable Bolt implementation. So the "write one graph connector" instinct lands on a protocol with no maintained portable Rust implementation, and the "write a portable one" instinct means owning Bolt yourself.

`neo4j` 0.2.0 and `grapha` 0.4.1 are name collisions, not drivers.

## Finding 4: Grafeo is the option worth weighing

`grafeo` 0.5.43, **Apache-2.0**, 23 701 downloads, updated 2026-09-27, `cargo add grafeo`, [grafeo.dev](https://grafeo.dev/).

It is a graph database **written in Rust**, and that is the part that matters for Loams:

- **No C dependency in the core.** "Core database engine written in Rust with no required C dependencies" — optional jemalloc/mimalloc and TLS only. Compare Neo4j, where every Rust path goes through Bolt and `neo4rs`.
- **Embedded or standalone.** Embeds directly, or runs as a server with a REST API. Loams's Fabric already has both modes in mind.
- **Six query languages, one engine:** **GQL** (the ISO standard, and its default), **Cypher** (Neo4j-compatible), Gremlin, GraphQL, SPARQL, and **SQL/PGQ**.
- **Both data models:** LPG *and* RDF/triples.
- MVCC with snapshot isolation, full ACID; cost-based optimizer; columnar storage with type-specific compression; zone maps; HNSW vector search with Scalar/Binary/Product quantization.
- 23 701 downloads is small — roughly two orders of magnitude behind `neo4rs`. **This is a young project and that is the real risk**, not the licence.

### Why GQL matters more than the engine

**GQL is ISO/IEC 39075**, the international standard for graph query languages, and Grafeo's default. That gives Loams the thing D354 wants and the Rust ecosystem otherwise lacks: **one portable surface across graph databases**. A Loams graph connector that targets GQL rather than Cypher is not tied to one vendor's dialect, and the same manifest serves Neo4j, Memgraph and anything GQL-conformant.

The same argument applies to the *sink* side, and it is the stronger half: writing **into** a graph is where dialects differ least, because a sink usually writes nodes and edges rather than running traversals. So a GQL-based sink is portable in a way a Cypher-based one is not.

**SQL/PGQ (SQL:2023 `GRAPH_TABLE`) is the interesting outlier.** It is the SQL standard's graph construct, which makes it the one query language here that could in principle be spoken by Loam House's ClickHouse surface. I am **not** claiming that works — ClickHouse does not implement `GRAPH_TABLE`, and FL2's `chsurface-1` is a declared subset of ClickHouse, not of SQL:2023. It is worth recording as a direction, not a plan.

## Options, ranked

1. **Target GQL; add Grafeo as the reference implementation; leave Neo4j to `neo4rs`.** One native Rust connector against the ISO standard, Grafeo as the engine Loams can actually embed and test, and `neo4rs` (MIT, maintained) for the Neo4j row that §33 already claims. This is the only option that makes Loams's graph story portable rather than vendor-shaped.
2. **Fix the row, defer the engine.** Change §33 A.3's Neo4j row to `Source ·` with a note that Camel cannot read and a native source is deferred, and add Grafeo as a second graph row at P3. Honest today, no new code, and it leaves the portable surface unclaimed.
3. **Write a portable Bolt client in Rust.** Maximum reach (Neo4j, Memgraph, and others), maximum cost: `bolt-client` has been frozen since 2022, so this is owning a wire protocol and its version matrix. Not justified by one P2 row.
4. **Do nothing.** Not viable — §33 claims a `Source Y` that cannot be served, so the catalog is currently wrong regardless.

## What I would rule, and what I cannot

**I would rule:** that the graph surface is **GQL**, not Cypher, and that Appendix A gains a Grafeo row. Both are cheap and both are reversible.

**I cannot rule, and want the owner or a reviewer to:**

- **Whether a 23 701-download project is a serious dependency.** Apache-2.0 and Rust-native are strong signals, but Grafeo is young and its own claims — "fastest graph database in our graph-bench suite" — are self-reported. If Loams depends on GQL, it depends on the *standard*, and Grafeo is a test target rather than a foundation. That framing is what makes option 1 safe despite the download count.
- **Whether Neo4j stays P2 at all**, given that reading it needs `neo4rs` and writing it needs nothing. A sink-only P2 row is a thin commitment.
- **Whether `surrealdb` should be added at all**, since `license = "non-standard"` is a hard D359 stop and would need an SPDX clarification the way `stringmetrics` got one in the root `deny.toml`.

## Measurements

```
# Camel component shape (the decisive finding)
python3 -c "import zipfile,json; z=zipfile.ZipFile('camel-catalog-4.22.1.jar'); \
  [print(k, json.loads(z.read(f'org/apache/camel/catalog/components/{k}.json'))['component']) \
   for k in ('neo4j','arangodb')]"

# Module tree at the same tag
gh api "repos/apache/camel/contents/components/camel-neo4j?ref=camel-4.22.1"   # 404
gh api "repos/apache/camel/contents/components?ref=camel-4.22.1" --jq '.[].name' # camel-arangodb present

# Crate provenance
curl -s "https://crates.io/api/v1/crates/<name>" -H 'User-Agent: loams-research'
```

Reproduce against `camel-catalog` 4.22.1 and `apache/camel` tag `camel-4.22.1`; re-check on any version bump, because Finding 2 is a live inconsistency rather than a historical one.