------------------------------ MODULE Lifecycle ------------------------------
(***************************************************************************)
(* The serverless lifecycle of one Loams SQL branch (design §47 §14, D733; *)
(* plan SQ1 Task 5): scale to zero when idle, wake on connect.             *)
(*                                                                         *)
(*   RUNNING -> SUSPENDING -> SUSPENDED -> RESUMING -> RUNNING             *)
(*                                                                         *)
(* The suspend saga closes idle sessions (1053), waits up to 30 s for the  *)
(* others (then kills them) and scales the pool to 0. The resume saga      *)
(* scales it to `Replicas`, probes it (`SELECT 1`) and releases the        *)
(* connections the gate held. A connection that arrives while the suspend  *)
(* is still closing sessions aborts it (the connection wins); once the     *)
(* scale-down has started, the suspend finishes and a resume follows (the  *)
(* suspend wins). Either way no session runs on a stopped pool.            *)
(*                                                                         *)
(* `state` and `step` are the saga's durable record; `pool` is the         *)
(* runtime's member count; `held`, `sessions` and `gone` are the gate's    *)
(* view of client connections. Every saga step is idempotent and its       *)
(* effect is persisted with it, so a restart of the saga process from its  *)
(* record changes nothing here (`Restart` is a stutter). The runtime       *)
(* effects (`ScaleDown`, `ScaleUp`) are separate from the steps that       *)
(* record them (`Suspended`, `ScaledUp`), because a crash can land between *)
(* the two: the restarted saga repeats the effect.                         *)
(*                                                                         *)
(* Action to code (crates/loams-sqlrouter/src/machines/lifecycle.rs emits  *)
(* each as a `SpecEvent` {conn, state, step}; tests/it/lifecycle.rs        *)
(* validates its traces with LifecycleTrace.tla):                          *)
(*   Admit(c), Hold(c)    Input::Connect: running, or held up to 30 s      *)
(*   Close(c)             Input::Closed of a session                       *)
(*   Timeout(c)           Input::Tick past a held deadline (1040), or      *)
(*                        Input::Closed of a held connection               *)
(*   StartSuspend         Input::Idle (the idle detector, suspend_after)   *)
(*   CloseIdle            Output::CloseIdle (the gate closes idle sessions)*)
(*   Kill                 Input::Tick past the 30 s quiesce deadline,      *)
(*                        Output::CloseAll                                 *)
(*   Quiesced             the last session closed during the suspend       *)
(*   AbortSuspend         Input::Connect before the scale-down             *)
(*   ScaleDown, Suspended Input::Scaled { replicas: 0 }                    *)
(*   StartResume          Input::Connect while suspended                   *)
(*   ScaleUp, ScaledUp    Input::Scaled { replicas: n }                    *)
(*   Resumed              Input::Probed { ok: true }                       *)
(*   Restart              Lifecycle::recover, then Input::Start            *)
(***************************************************************************)
EXTENDS Naturals, FiniteSets

CONSTANTS
    \* Client connections that may arrive.
    \* @type: Set(Str);
    Conns,
    \* The pool size a resume scales to.
    \* @type: Int;
    Replicas,
    \* Unsafe variant: a connection may still abort the suspend once the
    \* scale-down has started (MCLifecycle_LateAbort shows why it must not).
    \* @type: Bool;
    LateAbort

ASSUME Replicas \in Nat \ {0}

States == {"RUNNING", "SUSPENDING", "SUSPENDED", "RESUMING"}
Steps == {"none", "close_idle", "quiesce", "kill", "scale_down", "scale_up", "probe"}

VARIABLES
    \* The saga's durable record.
    \* @type: Str;
    state,
    \* @type: Str;
    step,
    \* Members the runtime runs.
    \* @type: Int;
    pool,
    \* Connections the gate holds until the branch runs (at most 30 s).
    \* @type: Set(Str);
    held,
    \* Connections relayed to a pool member.
    \* @type: Set(Str);
    sessions,
    \* Connections that ended: closed, refused (1040) or given up.
    \* @type: Set(Str);
    gone

vars == <<state, step, pool, held, sessions, gone>>

TypeOK ==
    /\ state \in States
    /\ step \in Steps
    /\ pool \in 0..Replicas
    /\ held \subseteq Conns
    /\ sessions \subseteq Conns
    /\ gone \subseteq Conns

\* Connections that have not arrived yet.
Fresh == Conns \ (held \cup sessions \cup gone)

\* The suspend steps a connection may still abort.
Abortable == IF LateAbort THEN {"close_idle", "quiesce", "kill", "scale_down"}
                          ELSE {"close_idle", "quiesce", "kill"}

-----------------------------------------------------------------------------
Init ==
    /\ \/ state = "RUNNING" /\ pool = Replicas
       \/ state = "SUSPENDED" /\ pool = 0
    /\ step = "none"
    /\ held = {}
    /\ sessions = {}
    /\ gone = {}

\* EnsureRunning answers at once while the branch runs.
Admit(c) ==
    /\ c \in Fresh
    /\ state = "RUNNING"
    /\ sessions' = sessions \cup {c}
    /\ UNCHANGED <<state, step, pool, held, gone>>

\* Otherwise the gate holds the connection.
Hold(c) ==
    /\ c \in Fresh
    /\ state # "RUNNING"
    /\ held' = held \cup {c}
    /\ UNCHANGED <<state, step, pool, sessions, gone>>

\* A session ends: the client quits, or the gate closes it (idle during a
\* suspend, or killed).
Close(c) ==
    /\ c \in sessions
    /\ sessions' = sessions \ {c}
    /\ gone' = gone \cup {c}
    /\ UNCHANGED <<state, step, pool, held>>

\* A held connection's 30 s pass (1040 "database is resuming, retry"), or
\* its client goes away.
Timeout(c) ==
    /\ c \in held
    /\ held' = held \ {c}
    /\ gone' = gone \cup {c}
    /\ UNCHANGED <<state, step, pool, sessions>>

\* The idle detector: no command for suspend_after.
StartSuspend ==
    /\ state = "RUNNING"
    /\ state' = "SUSPENDING"
    /\ step' = "close_idle"
    /\ UNCHANGED <<pool, held, sessions, gone>>

\* The gate closes idle sessions (1053) and, from now on, every session as
\* soon as it is idle.
CloseIdle ==
    /\ step = "close_idle"
    /\ step' = "quiesce"
    /\ UNCHANGED <<state, pool, held, sessions, gone>>

\* 30 s later the remaining sessions are killed.
Kill ==
    /\ step = "quiesce"
    /\ step' = "kill"
    /\ UNCHANGED <<state, pool, held, sessions, gone>>

Quiesced ==
    /\ step \in {"quiesce", "kill"}
    /\ sessions = {}
    /\ step' = "scale_down"
    /\ UNCHANGED <<state, pool, held, sessions, gone>>

\* A held connection wins over a suspend that has not started scaling down:
\* the branch runs again and the held connections are admitted.
AbortSuspend ==
    /\ state = "SUSPENDING"
    /\ step \in Abortable
    /\ held # {}
    /\ state' = "RUNNING"
    /\ step' = "none"
    /\ sessions' = sessions \cup held
    /\ held' = {}
    /\ UNCHANGED <<pool, gone>>

\* The runtime's effect; repeated if the saga restarts before recording it.
ScaleDown ==
    /\ step = "scale_down"
    /\ pool' = 0
    /\ UNCHANGED <<state, step, held, sessions, gone>>

Suspended ==
    /\ step = "scale_down"
    /\ pool = 0
    /\ state' = "SUSPENDED"
    /\ step' = "none"
    /\ UNCHANGED <<pool, held, sessions, gone>>

\* A held connection wakes a suspended branch.
StartResume ==
    /\ state = "SUSPENDED"
    /\ held # {}
    /\ state' = "RESUMING"
    /\ step' = "scale_up"
    /\ UNCHANGED <<pool, held, sessions, gone>>

ScaleUp ==
    /\ step = "scale_up"
    /\ pool' = Replicas
    /\ UNCHANGED <<state, step, held, sessions, gone>>

ScaledUp ==
    /\ step = "scale_up"
    /\ pool = Replicas
    /\ step' = "probe"
    /\ UNCHANGED <<state, pool, held, sessions, gone>>

\* A member's port answers and `SELECT 1` succeeds: release the held
\* connections. A failed probe is retried (a stutter here).
Resumed ==
    /\ step = "probe"
    /\ pool > 0
    /\ state' = "RUNNING"
    /\ step' = "none"
    /\ sessions' = sessions \cup held
    /\ held' = {}
    /\ UNCHANGED <<pool, gone>>

\* The saga process restarts from its record.
Restart == UNCHANGED vars

\* Under Kill, the gate closes the remaining sessions.
KillClose == step = "kill" /\ \E c \in sessions : Close(c)

Next ==
    \/ \E c \in Conns : Admit(c) \/ Hold(c) \/ Close(c) \/ Timeout(c)
    \/ StartSuspend
    \/ CloseIdle
    \/ Kill
    \/ Quiesced
    \/ AbortSuspend
    \/ ScaleDown
    \/ Suspended
    \/ StartResume
    \/ ScaleUp
    \/ ScaledUp
    \/ Resumed
    \/ Restart

\* The saga's steps and the kill deadline always fire eventually; clients,
\* timeouts and the idle detector do not have to.
Fairness ==
    /\ WF_vars(CloseIdle)
    /\ WF_vars(Kill)
    /\ WF_vars(KillClose)
    /\ WF_vars(Quiesced)
    /\ WF_vars(AbortSuspend)
    /\ WF_vars(ScaleDown)
    /\ WF_vars(Suspended)
    /\ WF_vars(StartResume)
    /\ WF_vars(ScaleUp)
    /\ WF_vars(ScaledUp)
    /\ WF_vars(Resumed)

Spec == Init /\ [][Next]_vars /\ Fairness

-----------------------------------------------------------------------------
(* Safety *)

\* No session is relayed to a pool that has been scaled to zero: a
\* connection is never lost or misrouted by a suspend (Review Focus 4).
NoSessionOnStoppedPool == sessions # {} => pool > 0

\* A connection is in at most one place.
ConnsDisjoint ==
    /\ held \cap sessions = {}
    /\ held \cap gone = {}
    /\ sessions \cap gone = {}

\* A running branch holds nobody and has its members.
RunningIsWarm == state = "RUNNING" => held = {} /\ pool = Replicas

\* A suspended branch has no members and no sessions.
SuspendedIsCold == state = "SUSPENDED" => pool = 0 /\ sessions = {}

\* Steps belong to their saga.
StepMatchesState ==
    /\ state \in {"RUNNING", "SUSPENDED"} <=> step = "none"
    /\ state = "SUSPENDING" => step \in {"close_idle", "quiesce", "kill", "scale_down"}
    /\ state = "RESUMING" => step \in {"scale_up", "probe"}

(* Liveness *)

\* Every held connection is admitted or ends: the saga never strands it,
\* even when no deadline fires.
HeldIsAnswered == \A c \in Conns : (c \in held) ~> (c \notin held)

\* A held connection is admitted unless it gives up.
HeldIsServed == \A c \in Conns : (c \in held) ~> (c \in sessions \cup gone)

\* A suspend always ends, suspended or running again.
SuspendEnds == (state = "SUSPENDING") ~> (state # "SUSPENDING")
=============================================================================
