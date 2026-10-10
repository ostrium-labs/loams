--------------------------- MODULE LifecycleTrace ---------------------------
(***************************************************************************)
(* Trace validation of Lifecycle (§31 §11.3, D311; plan SQ1 Task 5).       *)
(*                                                                         *)
(* `Trace` is a sequence of the `SpecEvent`s the Rust `Lifecycle` machine  *)
(* emitted, as records [action, conn, state, step]: `conn` is "" for an    *)
(* action without one, and `state` and `step` are the machine's durable    *)
(* record after the event. The driver puts an "Init" record with the       *)
(* starting record first. Each step of TraceNext explains the event at     *)
(* position `l` by the Lifecycle action of that name and checks the       *)
(* record; `pool`, `held`, `sessions` and `gone` follow from the spec.     *)
(* TraceMatched fails if some event cannot be explained, at the first one. *)
(*                                                                         *)
(* TLC 1.7.4 has no Json module (spec/tla/router/README.md), so the Rust   *)
(* test writes each trace as a TLA+ definition (MCLifecycleTrace*.tla).    *)
(***************************************************************************)
EXTENDS Lifecycle, Sequences

CONSTANT
    \* @type: Seq({ action: Str, conn: Str, state: Str, step: Str });
    Trace

VARIABLE
    \* The position of the next event to explain.
    \* @type: Int;
    l

traceVars == <<vars, l>>

\* The Lifecycle action that explains event `e`.
Explains(e) ==
    \/ e.action = "Admit" /\ Admit(e.conn)
    \/ e.action = "Hold" /\ Hold(e.conn)
    \/ e.action = "Close" /\ Close(e.conn)
    \/ e.action = "Timeout" /\ Timeout(e.conn)
    \/ e.action = "StartSuspend" /\ StartSuspend
    \/ e.action = "CloseIdle" /\ CloseIdle
    \/ e.action = "Kill" /\ Kill
    \/ e.action = "Quiesced" /\ Quiesced
    \/ e.action = "AbortSuspend" /\ AbortSuspend
    \/ e.action = "ScaleDown" /\ ScaleDown
    \/ e.action = "Suspended" /\ Suspended
    \/ e.action = "StartResume" /\ StartResume
    \/ e.action = "ScaleUp" /\ ScaleUp
    \/ e.action = "ScaledUp" /\ ScaledUp
    \/ e.action = "Resumed" /\ Resumed
    \/ e.action = "Restart" /\ Restart

TraceInit ==
    /\ Trace[1].action = "Init"
    /\ Init
    /\ state = Trace[1].state
    /\ step = Trace[1].step
    /\ l = 2

TraceNext ==
    /\ l <= Len(Trace)
    /\ Explains(Trace[l])
    /\ state' = Trace[l].state
    /\ step' = Trace[l].step
    /\ l' = l + 1

TraceSpec == TraceInit /\ [][TraceNext]_traceVars /\ WF_traceVars(TraceNext)

\* Every event is explained.
TraceMatched == <>(l = Len(Trace) + 1)
=============================================================================
