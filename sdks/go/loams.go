// The `Loams` object: one client with namespaced modules (design §44 §7.1).
//
//	client := loams.New(loams.Options{
//	    Endpoint: os.Getenv("LOAMS_ENDPOINT"),
//	    Auth:     loams.APIKey(os.Getenv("LOAMS_API_KEY")),
//	})
//	info, err := client.Instance().GetInstance(ctx, &loams.GetInstanceRequest{})
//
// What is generated and what is hand-written, once more, because it decides
// where a change goes. The **module surface** is generated: `gen/facade` holds
// the module catalogue and the binding table, read from the
// `loams.options.v1` annotations on the protos, and `modules.go` is one method
// per binding that hands it to the same invoker. The **runtime** behind those
// methods is hand-written, once, in this package: transport, credentials,
// retry, errors, tokens, pagination, streams. This file is the thin join — it
// builds the clients, wires the runtime, and holds the client-wide defaults. It
// contains no RPC path and no retry class, which is why annotating a proto is
// enough to add an SDK method.

package loams

import (
	"context"
	"errors"
	"net/http"
	"os"
	"strings"
	"time"

	"loams.dev/go/gen/facade"
	instancev1 "loams.dev/go/gen/loams/instance/v1"
	livev1 "loams.dev/go/gen/loams/live/v1"
)

// Options is how to build a Client.
type Options struct {
	// Endpoint is the instance's base URL, for example
	// `https://acme.loams.dev`. A loopback stack is `http://127.0.0.1:8080`.
	Endpoint string
	// Auth is the bearer source: `APIKey(key)` for a script or a CI job,
	// `StaticToken`, `EnvToken`, `OIDC`, or any `TokenSource` of your own.
	// Omitted means an unauthenticated client, which is what
	// `client.Instance().GetInstance` needs anyway.
	Auth TokenSource
	// Transport configures the wire. The zero value is Connect over HTTP/1.1,
	// which works through every proxy an instance is likely to sit behind.
	Transport TransportConfig
	// HTTPClient is the client the transport uses. It wins over Transport, so a
	// caller with its own TLS config, proxy or timeout supplies it here.
	HTTPClient *http.Client
	// MaxRetries is the retries after the first attempt, for every call.
	//
	// **Zero means `DefaultMaxRetries` (3), not none.** The design's number is the
	// client's number: an SDK that silently did not retry unless it was configured
	// to would be an SDK whose safe-by-default behaviour is a sharp edge, and a
	// caller writing `Options{Endpoint: ...}` would get something other than what
	// every other SDK does. Go's zero value is the problem, and it is solved by an
	// explicit opt-out:
	//
	//	client := loams.New(loams.Options{Endpoint: "…", NoRetries: true})
	//
	// A negative value is treated as the default. `WithMaxRetries(0)` on a **call**
	// still means none for that call, because writing the number there is an
	// unambiguous choice; here it is indistinguishable from not asking.
	MaxRetries int
	// NoRetries turns off automatic retries for the whole client.
	NoRetries bool
	// SessionConsistency holds a session consistency token across calls (D609,
	// R4).
	//
	// **Off by default**: every read is then `STRONG` on its own, which is
	// correct but does not give read-your-writes across processes. On, every
	// token a response carries merges into the session, and every read whose
	// request has a `consistency` field is sent `at_least` the session unless the
	// call set `consistency` itself or passed `WithoutSessionConsistency()`.
	SessionConsistency bool
	// Namespace is the default namespace (R11): it fills the `namespace` field
	// of any request that has one and leaves it empty. An explicit namespace
	// always wins; with neither, the call fails with `*InvalidArgumentError`
	// before any network call.
	Namespace string
	// Timeout is the budget of every unary call, retries included (R12).
	// **Zero means `DefaultTimeout` (30 s)**, for the reason `MaxRetries` gives;
	// set NoTimeout to turn it off. `WithTimeout` overrides it per call. Streams
	// have no default deadline.
	Timeout time.Duration
	// NoTimeout turns off the default unary deadline for the whole client.
	NoTimeout bool
	// Protocol overrides the wire protocol. Unset means Connect.
	Protocol Protocol
}

// Client is one SDK over one instance.
//
// It is safe for concurrent use: every field is either immutable or guarded,
// and the token source and session are, too. The one thing a caller must not do
// is close the `http.Client` it supplied while calls are in flight.
type Client struct {
	invoker *CallInvoker
	// session is nil unless SessionConsistency was on.
	session *ConsistencySession

	instance  InstanceModule
	live      LiveModule
	liveAdmin LiveAdminModule
	tables    TablesModule
	system    *System

	// The app modules (SDKP).
	namespaces  NamespacesModule
	collections CollectionsModule
	documents   DocumentsModule
	search      SearchModule
	vector      VectorModule
	graph       GraphModule
	sql         SqlModule
	streams     StreamsModule

	// modules is every generated module, by name: the catalogue a caller
	// iterates when it wants to feature-detect without hard-coding a field.
	modules map[string]any

	transport TransportConfig
	// namespace is the default namespace (R11), or "".
	namespace string
	// handleModules are the calls a collection handle delegates to.
	handleModules HandleModules
}

// New builds a Client.
//
// An empty Endpoint is a usage error rather than a request to localhost: a
// client that quietly talks to the wrong instance is worse than one that does
// not start.
func New(options Options) (*Client, error) {
	endpoint := strings.TrimRight(options.Endpoint, "/")
	if endpoint == "" {
		return nil, newInternalError("", "loams: Options.Endpoint is empty; it is the instance's base URL, for example https://acme.loams.dev")
	}
	config := options.Transport
	config.Endpoint = endpoint
	if options.Protocol != "" {
		config.Protocol = options.Protocol
	}
	// `Options.HTTPClient` wins over `Transport.HTTPClient`, but only when it is
	// set: assigning it unconditionally threw away a client the caller had put on
	// the transport.
	if options.HTTPClient != nil {
		config.HTTPClient = options.HTTPClient
	}
	if config.HTTPClient == nil {
		built, err := NewHTTPClient(config)
		if err != nil {
			return nil, err
		}
		config.HTTPClient = built
	}
	clientOptions, err := transportOptions(config)
	if err != nil {
		return nil, err
	}

	client := &Client{
		modules:   map[string]any{},
		transport: config,
	}
	if options.SessionConsistency {
		client.session = NewConsistencySession()
	}
	maxRetries := options.MaxRetries
	if maxRetries < 0 {
		maxRetries = DefaultMaxRetries
	}
	if maxRetries == 0 {
		maxRetries = DefaultMaxRetries
	}
	if options.NoRetries {
		maxRetries = 0
	}
	clients := facade.NewServiceClients(config.HTTPClient, endpoint, clientOptions...)
	// A nil *ConsistencySession must not reach the invoker as a non-nil
	// interface: "no session" has to stay nil.
	var store ConsistencyTokenStore
	if client.session != nil {
		store = client.session
	}
	client.invoker = NewCallInvoker(clients, options.Auth, maxRetries, store)
	timeout := options.Timeout
	if timeout <= 0 {
		timeout = DefaultTimeout
	}
	if options.NoTimeout {
		timeout = 0
	}
	client.invoker.SetDefaults(InvokerDefaults{Namespace: options.Namespace, Timeout: timeout})
	client.namespace = options.Namespace
	// ExtraHeaders were accepted and never sent (B17), so a client configured to
	// address one Live app silently addressed the server's default app instead.
	// They go through the invoker, which is the one place every unary call and
	// every stream builds its request headers.
	client.invoker.headers = config.ExtraHeaders.Clone()

	client.instance = &instanceModule{moduleInfo{invoker: client.invoker, registry: client.modules, name: "instance"}}
	client.live = &liveModule{moduleInfo{invoker: client.invoker, registry: client.modules, name: "live"}}
	client.liveAdmin = &liveAdminModule{moduleInfo{invoker: client.invoker, registry: client.modules, name: "live.admin"}}
	client.tables = &tablesModule{moduleInfo{invoker: client.invoker, registry: client.modules, name: "tables"}}
	client.modules["instance"] = client.instance
	client.modules["live"] = client.live
	client.modules["live.admin"] = client.liveAdmin
	client.modules["tables"] = client.tables
	app := func(name string) moduleInfo {
		return moduleInfo{invoker: client.invoker, registry: client.modules, name: name}
	}
	client.namespaces = &namespacesModule{app("namespaces")}
	client.collections = &collectionsModule{app("collections")}
	client.documents = &documentsModule{app("documents")}
	client.search = &searchModule{app("search")}
	client.vector = &vectorModule{app("vector")}
	client.graph = &graphModule{app("graph")}
	client.sql = &sqlModule{app("sql")}
	client.streams = &streamsModule{app("streams")}
	client.modules["namespaces"] = client.namespaces
	client.modules["collections"] = client.collections
	client.modules["documents"] = client.documents
	client.modules["search"] = client.search
	client.modules["vector"] = client.vector
	client.modules["graph"] = client.graph
	client.modules["sql"] = client.sql
	client.modules["streams"] = client.streams
	client.handleModules = HandleModules{
		Documents:      DocumentCalls{Write: client.documents.Write, Scroll: client.documents.Scroll},
		GetDocuments:   client.documents.Get,
		CountDocuments: client.documents.Count,
		Search:         SearchCalls{Query: client.search.Query},
		GetCollection:  client.collections.Get,
		DropCollection: client.collections.Drop,
	}
	client.system = &System{
		client:   client,
		endpoint: endpoint,
		speaks:   packagesSpoken(),
	}
	return client, nil
}

// The environment FromEnv reads (R1, R11).
const (
	EnvEndpoint  = "LOAMS_ENDPOINT"
	EnvNamespace = "LOAMS_NAMESPACE"
)

// FromEnv builds a Client from the environment: `LOAMS_ENDPOINT`, then
// `LOAMS_API_KEY` or `LOAMS_TOKEN` (through `EnvToken`), then `LOAMS_NAMESPACE`.
// A field already set in `options` wins over the environment.
func FromEnv(options Options) (*Client, error) {
	return fromEnv(options, os.LookupEnv)
}

func fromEnv(options Options, lookup func(string) (string, bool)) (*Client, error) {
	if options.Endpoint == "" {
		endpoint, _ := lookup(EnvEndpoint)
		if endpoint == "" {
			return nil, &InvalidArgumentError{LoamsError{
				Code:     CodeInvalidArgument,
				Reason:   ReasonInvalidArgument,
				Metadata: map[string]string{"field": EnvEndpoint},
				Cause:    errors.New("loams.FromEnv: LOAMS_ENDPOINT is not set"),
			}}
		}
		options.Endpoint = endpoint
	}
	if options.Auth == nil {
		for _, name := range EnvNames {
			if value, ok := lookup(name); ok && value != "" {
				options.Auth = &EnvTokenSource{Lookup: lookup}
				break
			}
		}
	}
	if options.Namespace == "" {
		options.Namespace, _ = lookup(EnvNamespace)
	}
	return New(options)
}

// Namespace is the client's default namespace (R11), or "".
func (c *Client) Namespace() string { return c.namespace }

// MustNew is New for a program that cannot usefully continue without a client —
// a `main`, a `TestMain`, an example. It panics on the error, which is a usage
// error and not a runtime condition.
func MustNew(options Options) *Client {
	client, err := New(options)
	if err != nil {
		panic(err)
	}
	return client
}

// Instance is `loams.instance` — what this instance is, and who the caller is.
func (c *Client) Instance() InstanceModule { return c.instance }

// Live is `loams.live` — the live sync session half. Its package is unstable, so
// its wire contract may still change (§44 §10.3).
func (c *Client) Live() LiveModule { return c.live }

// LiveAdmin is `loams.live.admin` — the app directory of the Live engine
// (`CreateApp`, `ListApps`, `GetApp`, `UpdateApp`, `DeleteApp`). Address one app
// with `WithLiveApp`.
func (c *Client) LiveAdmin() LiveAdminModule { return c.liveAdmin }

// Tables is `loams.tables` — the table half of the same service (design §44
// §7.2).
func (c *Client) Tables() TablesModule { return c.tables }

// System is the module catalogue, feature detection and the version check.
func (c *Client) System() *System { return c.system }

// Namespaces is `loams.namespaces` (SDKP §2.2).
func (c *Client) Namespaces() NamespacesModule { return c.namespaces }

// Collections is `loams.collections` (SDKP §2.2).
func (c *Client) Collections() CollectionsModule { return c.collections }

// Documents is `loams.documents` (SDKP §2.3): the generated calls. The
// conveniences (Upsert, Delete, Patch, WriteBatch, ScrollAll) are on
// DocumentCalls and on a collection handle (`Collection`).
func (c *Client) Documents() DocumentsModule { return c.documents }

// DocumentCalls are the document conveniences over this client's calls.
func (c *Client) DocumentCalls() DocumentCalls { return c.handleModules.Documents }

// Search is `loams.search` (SDKP §2.4); build a request with NewSearch.
func (c *Client) Search() SearchModule { return c.search }

// SearchCalls are the search conveniences (Text, Hybrid, QueryAll) over this
// client's `search.query`.
func (c *Client) SearchCalls() SearchCalls { return c.handleModules.Search }

// Vector is `loams.vector` (SDKP §2.4, Q613).
func (c *Client) Vector() VectorModule { return c.vector }

// Graph is `loams.graph` (SDKP §2.5). Its package is unstable.
func (c *Client) Graph() GraphModule { return c.graph }

// Sql is `loams.sql` (SDKP §2.5): one read-only SQL statement over a
// namespace's collections.
func (c *Client) Sql() SqlModule { return c.sql }

// Streams is `loams.streams` (SDKP §2.5): a namespace's logs, with
// Subscribe (R14).
func (c *Client) Streams() StreamsModule { return c.streams }

// Module returns a generated module by name, so a caller can feature-detect
// without a hard-coded field:
//
//	module, ok := client.Module("collections") // false until API1 Task 2
func (c *Client) Module(name string) (any, bool) {
	module, ok := c.modules[name]
	return module, ok
}

// Bindings is the generated binding table, as the SDK sees it.
func (c *Client) Bindings() []facade.ModuleBinding { return facade.Modules }

// Binding is the binding a module and call name identify, or a clear error.
// `call` is the Go method name (`GetInstance`), which is what the module methods
// are named after.
func (c *Client) Binding(module, call string) (facade.CallBinding, error) {
	found, ok := facade.Binding(module, call)
	if !ok {
		return facade.CallBinding{}, newInternalError("", "loams.%s has no generated call %s", module, call)
	}
	return found, nil
}

// ProtoRev is the proto revision this SDK declares (§44 §10.3).
func (c *Client) ProtoRev() string { return facade.ProtoRev }

// ProtoPackages is every proto package in the module, as `GetInstance` names
// them.
func (c *Client) ProtoPackages() []string { return facade.ProtoPackages }

// Session is the session consistency token store, or nil when
// `Options.SessionConsistency` was off. Nil rather than an inert store, so
// "not on" and "on but empty" do not look the same.
func (c *Client) Session() ConsistencyTokenStore {
	if c.session == nil {
		return nil
	}
	return c.session
}

// InvalidateCatalogue forgets the cached service catalogue, so the next feature
// check calls again.
func (c *Client) InvalidateCatalogue() { c.system.Invalidate() }

// Close releases the transport's idle connections.
//
// It does not close a caller-supplied `http.Client`: that client belongs to the
// caller and may be shared with something else. There is nothing to close for a
// client that supplied none, beyond the idle connections of the one this package
// built.
func (c *Client) Close() error {
	if transport, ok := c.transport.HTTPClient.Transport.(interface{ CloseIdleConnections() }); ok {
		transport.CloseIdleConnections()
	}
	return nil
}

// PaginateCall returns every item of a paged call (R6), resolving the binding
// from a module and call name first:
//
//	items, err := loams.PaginateCall(client, "collections", "ListCollections",
//	    client.Collections().ListCollections, ctx, req)
//	if err != nil {
//	    return err
//	}
//	for collection := range items.Seq() {
//	    use(collection)
//	}
//	return items.Err()
//
// It is `loams.Paginate` with the binding resolved, so a caller does not have to
// look the call up by hand. It is a free function rather than a method because Go
// does not allow a method to have type parameters, and `Client` cannot be
// generic.
//
// `collections` and `ListCollections` arrive with API1 Task 2; until then no
// generated call is paged and this returns an iterator whose `Err` says so. The
// runtime half is pinned by `TestGoPaginationIterator` against a stub.
func PaginateCall[Req, Res any, Item any](
	client *Client,
	module, call string,
	fetch PageFetcher[Req, Res],
	ctx context.Context,
	request *Req,
	options ...CallOption,
) (*PageIter[Req, Res, Item], error) {
	if client == nil {
		return nil, newInternalError("", "loams: PaginateCall needs a client")
	}
	binding, err := client.Binding(module, call)
	if err != nil {
		return nil, err
	}
	return Paginate[Req, Res, Item](binding, fetch, ctx, request, options...), nil
}

// The message types an application needs, re-exported so the common case is one
// import. The full set is in `loams.dev/go/gen/facade` and
// `loams.dev/go/gen/loams/...`.
type (
	// GetInstanceRequest and its response, `loams.instance.v1`.
	GetInstanceRequest  = instancev1.GetInstanceRequest
	GetInstanceResponse = instancev1.GetInstanceResponse
	// WhoAmIRequest and its response, `loams.instance.v1`.
	WhoAmIRequest  = instancev1.WhoAmIRequest
	WhoAmIResponse = instancev1.WhoAmIResponse

	// The live package, whose wire contract may still change (R1).
	WatchRequest           = livev1.WatchRequest
	Transition             = livev1.Transition
	ModifyQuerySetRequest  = livev1.ModifyQuerySetRequest
	ModifyQuerySetResponse = livev1.ModifyQuerySetResponse
	AuthenticateRequest    = livev1.AuthenticateRequest
	AuthenticateResponse   = livev1.AuthenticateResponse
	QueryRequest           = livev1.QueryRequest
	QueryResponse          = livev1.QueryResponse
	MutateRequest          = livev1.MutateRequest
	MutateResponse         = livev1.MutateResponse
	DeployRequest          = livev1.DeployRequest
	DeployResponse         = livev1.DeployResponse
	// The app directory (LV1 Task 13).
	CreateAppRequest  = livev1.CreateAppRequest
	CreateAppResponse = livev1.CreateAppResponse
	ListAppsRequest   = livev1.ListAppsRequest
	ListAppsResponse  = livev1.ListAppsResponse
	GetAppRequest     = livev1.GetAppRequest
	GetAppResponse    = livev1.GetAppResponse
	UpdateAppRequest  = livev1.UpdateAppRequest
	UpdateAppResponse = livev1.UpdateAppResponse
	DeleteAppRequest  = livev1.DeleteAppRequest
	DeleteAppResponse = livev1.DeleteAppResponse
)

// _ keeps the connect import in this file's import list honest: a caller's
// interceptor, codec or compression option is a `connect.ClientOption`, and the
// Options doc names that type.
