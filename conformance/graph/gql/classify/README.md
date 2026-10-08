# GQL statement classification corpus

GR1 Task 3 (design §48 §11.2; rulings R0.8, R0.10, R0.11). `crates/loams-graph/tests/graph.rs`
reads every `*.gql` file here in two tests:

- `guard_and_engine_agree_on_corpus`: the keyword guard (`classify`) never says Read when the
  engine (`engine_classify`, Grafeo's own translator) says Write or Admin. A case where the guard
  is stricter than the engine must be tagged `conservative`, and a tagged case must really be one.
- `read_session_refuses_every_corpus_write`: every Write or Admin case is refused by a Grafeo
  session with the `ReadOnly` role, and changes nothing.

Format: cases are separated by blank lines. A case's leading `#!` lines are its metadata, and the
rest is the statement, byte for byte (newlines included):

- `#! engine=read|write|admin`: what `engine_classify` must answer.
- `#! refused=<reason>`: the gate refuses the statement before the engine runs it, with this
  `ErrorInfo.reason`.
- `#! parse_error`: Grafeo's parser rejects it (the engine reports the syntax error).
- `#! conservative`: the guard says more than the engine. Since the security review (I1) a
  `CALL` is Write to both, so no case needs this today; the tag stays for the next one.
- A `CALL` is filed under `engine=write` even in `reads.gql`: a procedure may write, and the
  engine classification says so.
