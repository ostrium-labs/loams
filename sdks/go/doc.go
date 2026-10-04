// loams.dev/go is the Loams SDK for Go.
//
// It is one client object with namespaced modules over the unified Connect API
// on one port (design §44 §7, D600–D612):
//
//	client := loams.New(loams.Options{
//	    Endpoint: os.Getenv("LOAMS_ENDPOINT"),
//	    Auth:     loams.APIKey(os.Getenv("LOAMS_API_KEY")),
//	})
//
//	info, err := client.Instance().GetInstance(ctx, &loams.GetInstanceRequest{})
//	if err != nil {
//	    return err
//	}
//	if err := client.System().Guard(ctx, "live"); err != nil {
//	    return err
//	}
//	stream, err := client.Live().Watch(ctx, &loams.WatchRequest{})
//	if err != nil {
//	    return err
//	}
//	defer stream.Close()
//	for stream.Receive() {
//	    apply(stream.Msg())
//	}
//	return stream.Err()
//
// # Context first
//
// Every call takes a `context.Context` as its first argument and every one of
// the runtime contract's clauses composes with it rather than against it:
//
//   - a cancelled or expired context aborts the attempt and no further retry is
//     spent on it, because the caller has already said they do not want the
//     answer;
//   - `context.WithTimeout` is the deadline. There is no `TimeoutMs` option on
//     a call, because in Go a deadline is not an option, it is the context;
//   - the retry backoff sleeps on `ctx`, so a shutdown does not have to wait
//     out a 2 s backoff;
//   - a server stream is driven by the caller's loop, so cancellation is a
//     `select`/`ctx.Done()` rather than a subscription to unsubscribe.
//
// A caller who wants the whole client to share a deadline puts it on the
// context at the top: `ctx, cancel := context.WithTimeout(ctx, 5*time.Second)`
// and every call below takes it.
//
// # What the modules are, and what is not there
//
// The module and method surface is generated from the `loams.options.v1`
// annotations on the protos (design §44 §7.3, D606); only the runtime you are
// reading is hand-written. A module the generator has not seen is absent, so
// `client.Tables()` exists and `client.Collections()` does not — it arrives with
// `loams.collection.v1` (API1 Task 2).
//
// Three things design §44 §7.5 and §7.6 ask for are deliberately **not** here,
// because the protos to hang them on do not exist yet:
//
//   - the typed hybrid-query builder — no hybrid query RPC exists
//     (`loams.collection.v1`, `loams.query.v1`, API1 Tasks 2 and 4), and Q604
//     (one builder or thirteen) is unanswered;
//   - `client.Bulk()` over Arrow Flight — the write RPCs land with the write
//     paths (API1 Tasks 3 and 4), there is no `QueryArrow`, and this repository
//     carries no Flight SQL proto;
//   - a per-call `ListAll` alias — the pagination *iterator* is
//     `client.Paginate(...)` and is tested; a generated alias needs a generated
//     signature, which arrives with `ListCollections`.
//
// Writing any of them now would be a hand-written API with no proto behind it,
// which is exactly what §7.3 says not to do.
//
// # The runtime contract
//
// `docs/sdk/runtime-contract.md` states the behaviour as clauses R1–R10 that a
// conformance suite can check, so the thirteen SDKs are comparable. Each clause
// names the test that pins it. The six Go tests are in this package.
package loams
