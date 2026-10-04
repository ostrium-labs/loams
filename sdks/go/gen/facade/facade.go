// Package facade is the SDK surface of design §44 §7.3 (D606, D730): the
// module catalogue, the binding table the runtime dispatches through, the
// reason registry, and the Connect clients `protoc-gen-connect-go` wrote.
//
// # Provenance — read this before editing
//
// The Go renderer does not exist yet. `crates/loams-facade-gen` ships
// `typescript.rs` only; `src/go.rs`, `sdks/templates/go/` and the `go` case in
// `scripts/sdk/gen.sh` are the generator agent's (SDK1 Task 3, wave 1) and were
// not in `origin/dev` when this was written. So this file is the **Q604
// hand-written-facade fallback**, which design §44 §7.3 explicitly allows:
//
//	"If the plugin proves too costly for a language, that language falls back
//	to a hand-written facade checked by the same conformance suite (D606,
//	Q604)."
//
// It is a transcription of `sdks/typescript/packages/client/src/gen/facade.ts`
// and of the annotations in `proto/`, field for field, so the runtime behind it
// is surface-agnostic: when the renderer lands this file is deleted and
// `scripts/sdk/gen.sh go` writes a replacement in the same package with the
// same exported names. **Do not grow it.** If a call is missing, the proto is
// missing the `loams.options.v1` annotation — see `docs/design/13-decision-log.md`
// Q604.
//
// The generated stubs this file's clients are built from are real: they come
// from `protoc-gen-go` and `protoc-gen-connect-go` over `proto/` (D604; the
// Go SDK is generated from the protos, not a REST wrapper).
package facade

import (
	connect "connectrpc.com/connect"

	errorsv1 "loams.dev/go/gen/loams/errors/v1"
	instancev1connect "loams.dev/go/gen/loams/instance/v1/instancev1connect"
	livev1connect "loams.dev/go/gen/loams/live/v1/livev1connect"
)

// ProtoRev is the proto revision this SDK was generated from
// (`LOAMS_PROTO_REV`, design §44 §10.3). `loams.System().Version` checks it
// against the server's `GetInstance.ApiVersions`. Pre-1.0, so it is the API
// major rather than a release tag.
const ProtoRev = "v1"

// IdempotencyLevel is `idempotency_level` off the proto method. It is what the
// retry class is derived from (D610), so it is generated rather than guessed.
type IdempotencyLevel string

// The three `idempotency_level` values a Loams method may carry.
const (
	// NoSideEffects is `NO_SIDE_EFFECTS`: a read, retryable on its own.
	NoSideEffects IdempotencyLevel = "no_side_effects"
	// Idempotent is `IDEMPOTENT`: repeating it is the same call.
	Idempotent IdempotencyLevel = "idempotent"
	// None carries no idempotency level: a mutation, which may only be retried
	// when it carries an idempotency key.
	None IdempotencyLevel = "none"
)

// RetryClass says whether the SDK may retry a call on its own.
type RetryClass string

const (
	// RetrySafe is a read or an idempotent RPC: the SDK retries it.
	RetrySafe RetryClass = "safe"
	// RetryManual is a mutation: the SDK retries it only once it carries an
	// idempotency key.
	RetryManual RetryClass = "manual"
)

// Streaming says whether a call answers with one message or with a stream.
type Streaming string

const (
	// Unary is one request, one response.
	Unary Streaming = "unary"
	// Server is a server stream. There is no client streaming and no bidi
	// (D420).
	Server Streaming = "server"
)

// Pagination names the two fields a paged call pages on, as
// `FacadeOptions.pagination` ("<items>:<next page token>") does. The field
// names are the generated Go struct field names, because that is what the
// iterator sets and reads.
type Pagination struct {
	// ItemsField is the response's repeated field, in Go's PascalCase.
	ItemsField string
	// NextPageTokenField is the response's token field, in Go's PascalCase.
	NextPageTokenField string
	// PageSizeField is the request's `page_size` field, in Go's PascalCase.
	PageSizeField string
	// PageTokenField is the request's token field, in Go's PascalCase.
	PageTokenField string
}

// CallBinding is one facade call, as the runtime dispatches it.
type CallBinding struct {
	// Module is the SDK module the call is exposed on, in snake_case.
	Module string
	// Name is the Go method name, in PascalCase (design §44 §7.1).
	Name string
	// ProtoName is the name `FacadeOptions` gave it, verbatim.
	ProtoName string
	// Method is the method that backs the call.
	Method string
	// RPC is `package.Service/Method`, which is the path curl and grpcurl use.
	RPC string
	// Service is the fully qualified service name.
	Service string
	// Package is the proto package, which is the module catalogue's key
	// (design §44 §4, D600).
	Package string
	// Idempotency is the method's `idempotency_level`.
	Idempotency IdempotencyLevel
	// Retry is the class derived from Idempotency, or from
	// `FacadeOptions.retry_safe`.
	Retry RetryClass
	// Streaming is Unary or Server.
	Streaming Streaming
	// Pagination is nil for a call that does not page.
	Pagination *Pagination
	// TakesIdempotencyKey says whether the request *message* declares an
	// `idempotency_key` field. It is a property of the message rather than of
	// the call, and it is what makes a mutation retryable (D610), so it is
	// read from the generated schema rather than from what a caller passed.
	TakesIdempotencyKey bool
}

// ModuleBinding is one SDK module and its calls.
type ModuleBinding struct {
	// Name is the module's name in snake_case, as `loams.<name>`.
	Name string
	// Summary is one line for the module's reference docs.
	Summary string
	// Service is the service behind the module.
	Service string
	// Package is the proto package, which is what `GetInstance.Services[]`
	// keys on.
	Package string
	// Unstable says the package's wire contract may still change, so `buf
	// breaking` skips it and an SDK marks the module experimental (§44
	// §10.3).
	Unstable bool
	// Derived is true for a second facade name for the same RPCs, which has no
	// summary of its own.
	Derived bool
	// Calls are the module's facade calls.
	Calls []CallBinding
}

// Modules is every annotated service, as the module catalogue, ordered by
// module name.
var Modules = []ModuleBinding{
	{
		Name:    "instance",
		Summary: "What this instance is, and who the caller is on it.",
		Service: instancev1connect.InstanceServiceName,
		Package: "loams.instance.v1",
		Calls: []CallBinding{
			{
				Module: "instance", Name: "GetInstance", ProtoName: "getInstance",
				Method: "GetInstance", RPC: instancev1connect.InstanceServiceName + "/GetInstance",
				Service: instancev1connect.InstanceServiceName, Package: "loams.instance.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
			{
				Module: "instance", Name: "WhoAmI", ProtoName: "whoAmI",
				Method: "WhoAmI", RPC: instancev1connect.InstanceServiceName + "/WhoAmI",
				Service: instancev1connect.InstanceServiceName, Package: "loams.instance.v1",
				Idempotency: NoSideEffects, Retry: RetrySafe, Streaming: Unary,
			},
		},
	},
	{
		Name:     "live",
		Summary:  "Live sync: watch a query set over a server stream.",
		Service:  livev1connect.LiveServiceName,
		Package:  "loams.live.v1",
		Unstable: true,
		Calls: []CallBinding{
			{
				Module: "live", Name: "ModifyQuerySet", ProtoName: "modifyQuerySet",
				Method: "ModifyQuerySet", RPC: livev1connect.LiveServiceName + "/ModifyQuerySet",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "live", Name: "Watch", ProtoName: "watch",
				Method: "Watch", RPC: livev1connect.LiveServiceName + "/Watch",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Server,
			},
		},
	},
	{
		Name:     "tables",
		Service:  livev1connect.LiveServiceName,
		Package:  "loams.live.v1",
		Unstable: true,
		Derived:  true,
		Calls: []CallBinding{
			{
				Module: "tables", Name: "Deploy", ProtoName: "deploy",
				Method: "Deploy", RPC: livev1connect.LiveServiceName + "/Deploy",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
			{
				Module: "tables", Name: "Mutate", ProtoName: "mutate",
				Method: "Mutate", RPC: livev1connect.LiveServiceName + "/Mutate",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
				// `MutateRequest.idempotency_key` (live.proto:163). Read off
				// the generated message, not guessed: `DeployRequest` has no
				// such field and must not be keyed.
				TakesIdempotencyKey: true,
			},
			{
				Module: "tables", Name: "Query", ProtoName: "query",
				Method: "Query", RPC: livev1connect.LiveServiceName + "/Query",
				Service: livev1connect.LiveServiceName, Package: "loams.live.v1",
				Idempotency: None, Retry: RetryManual, Streaming: Unary,
			},
		},
	},
}

// ProtoPackages is every proto package in the module, as
// `GetInstance.ApiVersions` names them.
var ProtoPackages = []string{
	"google.protobuf",
	"loams.approvals.v1",
	"loams.devices.v1",
	"loams.errors.v1",
	"loams.instance.v1",
	"loams.live.v1",
	"loams.notifications.v1",
	"loams.operations.v1",
	"loams.options.v1",
}

// ErrorInfoTypeURL is the `type` a server puts in an error detail. connect-go
// prepends the default Any prefix to a bare name, so a detail that arrives as
// `loams.errors.v1.ErrorInfo` is looked up here.
const ErrorInfoTypeURL = "loams.errors.v1.ErrorInfo"

// ErrorInfoType is the Go type of that detail.
type ErrorInfoType = errorsv1.ErrorInfo

// NewServiceClients builds one Connect client per annotated service, from the
// stubs `protoc-gen-connect-go` wrote over the same protos. This is what makes
// the facade generated rather than hand-written: the SDK never writes a client,
// it wraps the generated ones.
func NewServiceClients(httpClient connect.HTTPClient, baseURL string, opts ...connect.ClientOption) map[string]any {
	return map[string]any{
		instancev1connect.InstanceServiceName: instancev1connect.NewInstanceServiceClient(httpClient, baseURL, opts...),
		livev1connect.LiveServiceName:         livev1connect.NewLiveServiceClient(httpClient, baseURL, opts...),
	}
}
