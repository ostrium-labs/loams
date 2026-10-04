// The module surface: one Go method per generated facade call.
//
// # Provenance
//
// Design §44 §7.3 (D606) says the module and method surface is **generated**
// from the `loams.options.v1` annotations, so thirteen languages cannot drift,
// and it allows a hand-written facade for a language where the generator proves
// costly (Q604). The Go renderer is not written yet, so this is that fallback
// (D730) and it is a transcription of the same annotations
// `sdks/typescript/packages/client/src/gen/facade.ts` was generated from.
//
// Every method below resolves its binding by module and call name and hands the
// generated stub's method value to the shared invoker. It holds no RPC path, no
// retry class and no message name of its own, so a wrong retry class would have
// to be written here — and there is nowhere here to write one, because they all
// come from `facade`.
//
// When `crates/loams-facade-gen/src/go.rs` lands, `go generate` writes this file
// and the hand-written one is deleted. **Do not add a method without the
// annotation that generates it.**

package loams

import (
	"context"

	"loams.dev/go/gen/facade"
	instancev1connect "loams.dev/go/gen/loams/instance/v1/instancev1connect"
	livev1connect "loams.dev/go/gen/loams/live/v1/livev1connect"
)

// clientOf is the generated Connect client behind a service, or a clear internal
// error. It is a free function rather than a method because Go has no generic
// methods, and a type assertion that panics would turn a wiring mistake into a
// crash in an application.
func clientOf[T any](invoker *CallInvoker, service string) (T, error) {
	var zero T
	found, ok := invoker.clients[service]
	if !ok {
		return zero, newInternalError("", "no generated client for %s", service)
	}
	typed, ok := found.(T)
	if !ok {
		return zero, newInternalError("", "the generated client for %s is a %T, not the one the binding names", service, found)
	}
	return typed, nil
}

// InstanceModule is `loams.instance`: what this instance is, and who the caller
// is on it.
//
// GetInstance is the first call any client makes: it needs no credentials, and
// its `Services` field is the module catalogue an SDK feature-detects from
// (design §44 §4, D600).
type InstanceModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// GetInstance is `loams.instance.v1.InstanceService/GetInstance`, retried:
	// safe. No auth: what this instance is and how to sign in to it.
	GetInstance(ctx context.Context, request *GetInstanceRequest, options ...CallOption) (*GetInstanceResponse, error)
	// WhoAmI is `loams.instance.v1.InstanceService/WhoAmI`, retried: safe. The
	// calling principal, its org, and the environments it can reach.
	WhoAmI(ctx context.Context, request *WhoAmIRequest, options ...CallOption) (*WhoAmIResponse, error)
}

// LiveModule is `loams.live`: the live sync session half.
//
// Its wire contract may still change, so `buf breaking` skips the package and an
// SDK marks the module experimental (§44 §10.3).
type LiveModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// ModifyQuerySet is `loams.live.v1.LiveService/ModifyQuerySet`, retried:
	// manual. Adds and removes queries in an open session; the next transition
	// reflects the change.
	ModifyQuerySet(ctx context.Context, request *ModifyQuerySetRequest, options ...CallOption) (*ModifyQuerySetResponse, error)
	// Watch is `loams.live.v1.LiveService/Watch`, retried: manual, and a server
	// stream. Pass `WithStreamResume` to reconnect from the last cursor rather
	// than silently miss the changes in between (R7):
	//
	//	stream, err := client.Live().Watch(ctx, request, loams.WithStreamResume(
	//	    loams.StreamResume[*loams.WatchRequest, *loams.Transition]{
	//	        CursorOf: func(t *loams.Transition) string { return cursorOf(t) },
	//	        Resume:   func(c string, r *loams.WatchRequest) *loams.WatchRequest { ... },
	//	    }))
	Watch(ctx context.Context, request *WatchRequest, options ...CallOption) (*Stream[Transition], error)
}

// TablesModule is `loams.tables`: the table half of the same service, a second
// facade name for `loams.live.v1`'s unary RPCs (design §44 §7.2).
type TablesModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Query is `loams.live.v1.LiveService/Query`, retried: manual. A one-shot
	// query, at the latest tick or at a given timestamp.
	Query(ctx context.Context, request *QueryRequest, options ...CallOption) (*QueryResponse, error)
	// Mutate is `loams.live.v1.LiveService/Mutate`, retried: manual, and the
	// only keyed mutation in the API today.
	//
	// `MutateRequest.IdempotencyKey` is given a UUIDv7 before the first attempt
	// and reused on every retry, so a retried mutation is one write (R3). Pass
	// `WithIdempotencyKey` to supply your own.
	Mutate(ctx context.Context, request *MutateRequest, options ...CallOption) (*MutateResponse, error)
	// Deploy is `loams.live.v1.LiveService/Deploy`, retried: manual. Admin:
	// deploys a function bundle and a schema.
	Deploy(ctx context.Context, request *DeployRequest, options ...CallOption) (*DeployResponse, error)
}

// moduleInfo is what the three implementations share. `registry` is the map a
// caller reaches every module through, so `Client.Module(name)` and the concrete
// accessors can never disagree about what exists.
type moduleInfo struct {
	invoker  *CallInvoker
	registry map[string]any
	name     string
}

// Module is the SDK module's name, as it appears on the client.
func (m *moduleInfo) Module() string { return m.name }

// Service is the service behind the module, from the generated binding table.
func (m *moduleInfo) Service() string {
	entry, _ := facade.Module(m.name)
	return entry.Service
}

// Unstable is whether the package's wire contract may still change.
func (m *moduleInfo) Unstable() bool {
	entry, _ := facade.Module(m.name)
	return entry.Unstable
}

// binding resolves this module's call. A name that does not resolve is a bug in
// this file rather than a caller's mistake, which is why it is `internal` and
// not `not_found`.
func (m *moduleInfo) binding(call string) (facade.CallBinding, error) {
	found, ok := facade.Binding(m.name, call)
	if !ok {
		return facade.CallBinding{}, newInternalError("", "loams.%s has no generated call %s", m.name, call)
	}
	return found, nil
}

type instanceModule struct{ moduleInfo }

func (m *instanceModule) GetInstance(ctx context.Context, request *GetInstanceRequest, options ...CallOption) (*GetInstanceResponse, error) {
	binding, err := m.binding("GetInstance")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[instancev1connect.InstanceServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.GetInstance, request, options...)
}

func (m *instanceModule) WhoAmI(ctx context.Context, request *WhoAmIRequest, options ...CallOption) (*WhoAmIResponse, error) {
	binding, err := m.binding("WhoAmI")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[instancev1connect.InstanceServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.WhoAmI, request, options...)
}

type liveModule struct{ moduleInfo }

func (m *liveModule) ModifyQuerySet(ctx context.Context, request *ModifyQuerySetRequest, options ...CallOption) (*ModifyQuerySetResponse, error) {
	binding, err := m.binding("ModifyQuerySet")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[livev1connect.LiveServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ModifyQuerySet, request, options...)
}

func (m *liveModule) Watch(ctx context.Context, request *WatchRequest, options ...CallOption) (*Stream[Transition], error) {
	binding, err := m.binding("Watch")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[livev1connect.LiveServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return ServerStream(m.invoker, ctx, binding, client.Watch, request, options...)
}

type tablesModule struct{ moduleInfo }

func (m *tablesModule) Query(ctx context.Context, request *QueryRequest, options ...CallOption) (*QueryResponse, error) {
	binding, err := m.binding("Query")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[livev1connect.LiveServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Query, request, options...)
}

func (m *tablesModule) Mutate(ctx context.Context, request *MutateRequest, options ...CallOption) (*MutateResponse, error) {
	binding, err := m.binding("Mutate")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[livev1connect.LiveServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Mutate, request, options...)
}

func (m *tablesModule) Deploy(ctx context.Context, request *DeployRequest, options ...CallOption) (*DeployResponse, error) {
	binding, err := m.binding("Deploy")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[livev1connect.LiveServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Deploy, request, options...)
}
