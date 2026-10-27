// The app modules (SDKP §2.2–§2.5): `namespaces`, `collections`,
// `documents`, `search`, `vector` and `graph`, one Go method per facade call.
//
// Like modules.go this is the Q604 fallback (D730): a transcription of the
// `loams.options.v1` annotations, here written mechanically from
// `sdks/fixtures/facade-index.json` and the protos' rpc lines, and checked
// against the index by `go_facade_matches_index`. Every method resolves its
// binding by name and hands the generated stub's method value to the shared
// invoker, so the RPC path, the retry class and the idempotency key come from
// `facade`, not from here.

package loams

import (
	"context"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
	collectionv1connect "loams.dev/go/gen/loams/collection/v1/collectionv1connect"
	graphv1 "loams.dev/go/gen/loams/graph/v1"
	graphv1connect "loams.dev/go/gen/loams/graph/v1/graphv1connect"
	operationsv1 "loams.dev/go/gen/loams/operations/v1"
)

// CollectionsModule is `loams.collections`.
type CollectionsModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// AddFields is `loams.collection.v1.CollectionService/AddFields`, retried: safe.
	AddFields(ctx context.Context, request *collectionv1.AddFieldsRequest, options ...CallOption) (*collectionv1.AddFieldsResponse, error)
	// Create is `loams.collection.v1.CollectionService/CreateCollection`, retried: safe.
	Create(ctx context.Context, request *collectionv1.CreateCollectionRequest, options ...CallOption) (*collectionv1.CollectionInfo, error)
	// Drop is `loams.collection.v1.CollectionService/DropCollection`, retried: safe.
	Drop(ctx context.Context, request *collectionv1.DropCollectionRequest, options ...CallOption) (*collectionv1.DropCollectionResponse, error)
	// Get is `loams.collection.v1.CollectionService/GetCollection`, retried: safe.
	Get(ctx context.Context, request *collectionv1.GetCollectionRequest, options ...CallOption) (*collectionv1.CollectionInfo, error)
	// List is `loams.collection.v1.CollectionService/ListCollections`, retried: safe.
	List(ctx context.Context, request *collectionv1.ListCollectionsRequest, options ...CallOption) (*collectionv1.ListCollectionsResponse, error)
	// ListVersions is `loams.collection.v1.CollectionService/ListVersions`, retried: safe.
	ListVersions(ctx context.Context, request *collectionv1.ListVersionsRequest, options ...CallOption) (*collectionv1.ListVersionsResponse, error)
	// Scan is `loams.collection.v1.CollectionService/Scan`, retried: manual.
	Scan(ctx context.Context, request *collectionv1.ScanRequest, options ...CallOption) (*collectionv1.ScanPlan, error)
	// SetHot is `loams.collection.v1.CollectionService/SetHot`, retried: safe.
	SetHot(ctx context.Context, request *collectionv1.SetHotRequest, options ...CallOption) (*collectionv1.SetHotResponse, error)
	// UpdateAliases is `loams.collection.v1.CollectionService/UpdateAliases`, retried: safe.
	UpdateAliases(ctx context.Context, request *collectionv1.UpdateAliasesRequest, options ...CallOption) (*collectionv1.UpdateAliasesResponse, error)
	// Warm is `loams.collection.v1.CollectionService/WarmCollection`, retried: safe.
	Warm(ctx context.Context, request *collectionv1.WarmCollectionRequest, options ...CallOption) (*collectionv1.WarmCollectionResponse, error)
}

type collectionsModule struct{ moduleInfo }

func (m *collectionsModule) AddFields(ctx context.Context, request *collectionv1.AddFieldsRequest, options ...CallOption) (*collectionv1.AddFieldsResponse, error) {
	binding, err := m.binding("AddFields")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.AddFields, request, options...)
}

func (m *collectionsModule) Create(ctx context.Context, request *collectionv1.CreateCollectionRequest, options ...CallOption) (*collectionv1.CollectionInfo, error) {
	binding, err := m.binding("Create")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.CreateCollection, request, options...)
}

func (m *collectionsModule) Drop(ctx context.Context, request *collectionv1.DropCollectionRequest, options ...CallOption) (*collectionv1.DropCollectionResponse, error) {
	binding, err := m.binding("Drop")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.DropCollection, request, options...)
}

func (m *collectionsModule) Get(ctx context.Context, request *collectionv1.GetCollectionRequest, options ...CallOption) (*collectionv1.CollectionInfo, error) {
	binding, err := m.binding("Get")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.GetCollection, request, options...)
}

func (m *collectionsModule) List(ctx context.Context, request *collectionv1.ListCollectionsRequest, options ...CallOption) (*collectionv1.ListCollectionsResponse, error) {
	binding, err := m.binding("List")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ListCollections, request, options...)
}

func (m *collectionsModule) ListVersions(ctx context.Context, request *collectionv1.ListVersionsRequest, options ...CallOption) (*collectionv1.ListVersionsResponse, error) {
	binding, err := m.binding("ListVersions")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ListVersions, request, options...)
}

func (m *collectionsModule) Scan(ctx context.Context, request *collectionv1.ScanRequest, options ...CallOption) (*collectionv1.ScanPlan, error) {
	binding, err := m.binding("Scan")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Scan, request, options...)
}

func (m *collectionsModule) SetHot(ctx context.Context, request *collectionv1.SetHotRequest, options ...CallOption) (*collectionv1.SetHotResponse, error) {
	binding, err := m.binding("SetHot")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.SetHot, request, options...)
}

func (m *collectionsModule) UpdateAliases(ctx context.Context, request *collectionv1.UpdateAliasesRequest, options ...CallOption) (*collectionv1.UpdateAliasesResponse, error) {
	binding, err := m.binding("UpdateAliases")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.UpdateAliases, request, options...)
}

func (m *collectionsModule) Warm(ctx context.Context, request *collectionv1.WarmCollectionRequest, options ...CallOption) (*collectionv1.WarmCollectionResponse, error) {
	binding, err := m.binding("Warm")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.CollectionServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.WarmCollection, request, options...)
}

// DocumentsModule is `loams.documents`.
type DocumentsModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Count is `loams.collection.v1.DocumentService/CountDocuments`, retried: safe.
	Count(ctx context.Context, request *collectionv1.CountDocumentsRequest, options ...CallOption) (*collectionv1.CountDocumentsResponse, error)
	// DeleteByFilter is `loams.collection.v1.DocumentService/DeleteByFilter`, retried: manual.
	DeleteByFilter(ctx context.Context, request *collectionv1.DeleteByFilterRequest, options ...CallOption) (*collectionv1.FilterWriteResponse, error)
	// Get is `loams.collection.v1.DocumentService/GetDocuments`, retried: safe.
	Get(ctx context.Context, request *collectionv1.GetDocumentsRequest, options ...CallOption) (*collectionv1.GetDocumentsResponse, error)
	// PatchByFilter is `loams.collection.v1.DocumentService/PatchByFilter`, retried: manual.
	PatchByFilter(ctx context.Context, request *collectionv1.PatchByFilterRequest, options ...CallOption) (*collectionv1.FilterWriteResponse, error)
	// Scroll is `loams.collection.v1.DocumentService/ScrollDocuments`, retried: safe.
	Scroll(ctx context.Context, request *collectionv1.ScrollDocumentsRequest, options ...CallOption) (*collectionv1.ScrollDocumentsResponse, error)
	// Write is `loams.collection.v1.DocumentService/WriteDocuments`, retried: keyed (R3).
	Write(ctx context.Context, request *collectionv1.WriteDocumentsRequest, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error)
}

type documentsModule struct{ moduleInfo }

func (m *documentsModule) Count(ctx context.Context, request *collectionv1.CountDocumentsRequest, options ...CallOption) (*collectionv1.CountDocumentsResponse, error) {
	binding, err := m.binding("Count")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.CountDocuments, request, options...)
}

func (m *documentsModule) DeleteByFilter(ctx context.Context, request *collectionv1.DeleteByFilterRequest, options ...CallOption) (*collectionv1.FilterWriteResponse, error) {
	binding, err := m.binding("DeleteByFilter")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.DeleteByFilter, request, options...)
}

func (m *documentsModule) Get(ctx context.Context, request *collectionv1.GetDocumentsRequest, options ...CallOption) (*collectionv1.GetDocumentsResponse, error) {
	binding, err := m.binding("Get")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.GetDocuments, request, options...)
}

func (m *documentsModule) PatchByFilter(ctx context.Context, request *collectionv1.PatchByFilterRequest, options ...CallOption) (*collectionv1.FilterWriteResponse, error) {
	binding, err := m.binding("PatchByFilter")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.PatchByFilter, request, options...)
}

func (m *documentsModule) Scroll(ctx context.Context, request *collectionv1.ScrollDocumentsRequest, options ...CallOption) (*collectionv1.ScrollDocumentsResponse, error) {
	binding, err := m.binding("Scroll")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ScrollDocuments, request, options...)
}

func (m *documentsModule) Write(ctx context.Context, request *collectionv1.WriteDocumentsRequest, options ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
	binding, err := m.binding("Write")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.DocumentServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.WriteDocuments, request, options...)
}

// GraphModule is `loams.graph`.
type GraphModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Create is `loams.graph.v1.GraphAdminService/CreateGraph`, retried: keyed (R3).
	Create(ctx context.Context, request *graphv1.CreateGraphRequest, options ...CallOption) (*graphv1.Graph, error)
	// Delete is `loams.graph.v1.GraphAdminService/DeleteGraph`, retried: keyed (R3).
	Delete(ctx context.Context, request *graphv1.DeleteGraphRequest, options ...CallOption) (*operationsv1.Operation, error)
	// Explain is `loams.graph.v1.GraphService/Explain`, retried: safe.
	Explain(ctx context.Context, request *graphv1.ExplainRequest, options ...CallOption) (*graphv1.Plan, error)
	// Get is `loams.graph.v1.GraphAdminService/GetGraph`, retried: safe.
	Get(ctx context.Context, request *graphv1.GetGraphRequest, options ...CallOption) (*graphv1.Graph, error)
	// List is `loams.graph.v1.GraphAdminService/ListGraphs`, retried: safe.
	List(ctx context.Context, request *graphv1.ListGraphsRequest, options ...CallOption) (*graphv1.ListGraphsResponse, error)
	// Query is `loams.graph.v1.GraphService/Execute`, retried: keyed (R3).
	Query(ctx context.Context, request *graphv1.ExecuteRequest, options ...CallOption) (*graphv1.ExecuteResponse, error)
	// QueryBatch is `loams.graph.v1.GraphService/ExecuteBatch`, retried: keyed (R3).
	QueryBatch(ctx context.Context, request *graphv1.ExecuteBatchRequest, options ...CallOption) (*graphv1.ExecuteBatchResponse, error)
	// QueryStream is `loams.graph.v1.GraphService/ExecuteStream`, retried: manual.
	QueryStream(ctx context.Context, request *graphv1.ExecuteStreamRequest, options ...CallOption) (*Stream[graphv1.ResultChunk], error)
	// Schema is `loams.graph.v1.GraphAdminService/GetSchema`, retried: safe.
	Schema(ctx context.Context, request *graphv1.GetSchemaRequest, options ...CallOption) (*graphv1.GraphSchema, error)
	// Update is `loams.graph.v1.GraphAdminService/UpdateGraph`, retried: keyed (R3).
	Update(ctx context.Context, request *graphv1.UpdateGraphRequest, options ...CallOption) (*graphv1.Graph, error)
}

type graphModule struct{ moduleInfo }

func (m *graphModule) Create(ctx context.Context, request *graphv1.CreateGraphRequest, options ...CallOption) (*graphv1.Graph, error) {
	binding, err := m.binding("Create")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.CreateGraph, request, options...)
}

func (m *graphModule) Delete(ctx context.Context, request *graphv1.DeleteGraphRequest, options ...CallOption) (*operationsv1.Operation, error) {
	binding, err := m.binding("Delete")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.DeleteGraph, request, options...)
}

func (m *graphModule) Explain(ctx context.Context, request *graphv1.ExplainRequest, options ...CallOption) (*graphv1.Plan, error) {
	binding, err := m.binding("Explain")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Explain, request, options...)
}

func (m *graphModule) Get(ctx context.Context, request *graphv1.GetGraphRequest, options ...CallOption) (*graphv1.Graph, error) {
	binding, err := m.binding("Get")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.GetGraph, request, options...)
}

func (m *graphModule) List(ctx context.Context, request *graphv1.ListGraphsRequest, options ...CallOption) (*graphv1.ListGraphsResponse, error) {
	binding, err := m.binding("List")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ListGraphs, request, options...)
}

func (m *graphModule) Query(ctx context.Context, request *graphv1.ExecuteRequest, options ...CallOption) (*graphv1.ExecuteResponse, error) {
	binding, err := m.binding("Query")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Execute, request, options...)
}

func (m *graphModule) QueryBatch(ctx context.Context, request *graphv1.ExecuteBatchRequest, options ...CallOption) (*graphv1.ExecuteBatchResponse, error) {
	binding, err := m.binding("QueryBatch")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ExecuteBatch, request, options...)
}

func (m *graphModule) QueryStream(ctx context.Context, request *graphv1.ExecuteStreamRequest, options ...CallOption) (*Stream[graphv1.ResultChunk], error) {
	binding, err := m.binding("QueryStream")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return ServerStream(m.invoker, ctx, binding, client.ExecuteStream, request, options...)
}

func (m *graphModule) Schema(ctx context.Context, request *graphv1.GetSchemaRequest, options ...CallOption) (*graphv1.GraphSchema, error) {
	binding, err := m.binding("Schema")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.GetSchema, request, options...)
}

func (m *graphModule) Update(ctx context.Context, request *graphv1.UpdateGraphRequest, options ...CallOption) (*graphv1.Graph, error) {
	binding, err := m.binding("Update")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[graphv1connect.GraphAdminServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.UpdateGraph, request, options...)
}

// NamespacesModule is `loams.namespaces`.
type NamespacesModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Create is `loams.collection.v1.NamespaceService/CreateNamespace`, retried: safe.
	Create(ctx context.Context, request *collectionv1.CreateNamespaceRequest, options ...CallOption) (*collectionv1.CreateNamespaceResponse, error)
	// List is `loams.collection.v1.NamespaceService/ListNamespaces`, retried: safe.
	List(ctx context.Context, request *collectionv1.ListNamespacesRequest, options ...CallOption) (*collectionv1.ListNamespacesResponse, error)
}

type namespacesModule struct{ moduleInfo }

func (m *namespacesModule) Create(ctx context.Context, request *collectionv1.CreateNamespaceRequest, options ...CallOption) (*collectionv1.CreateNamespaceResponse, error) {
	binding, err := m.binding("Create")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.NamespaceServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.CreateNamespace, request, options...)
}

func (m *namespacesModule) List(ctx context.Context, request *collectionv1.ListNamespacesRequest, options ...CallOption) (*collectionv1.ListNamespacesResponse, error) {
	binding, err := m.binding("List")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.NamespaceServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ListNamespaces, request, options...)
}

// SearchModule is `loams.search`.
type SearchModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Query is `loams.collection.v1.QueryService/Search`, retried: safe.
	Query(ctx context.Context, request *collectionv1.SearchRequest, options ...CallOption) (*collectionv1.SearchResponse, error)
}

type searchModule struct{ moduleInfo }

func (m *searchModule) Query(ctx context.Context, request *collectionv1.SearchRequest, options ...CallOption) (*collectionv1.SearchResponse, error) {
	binding, err := m.binding("Query")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.QueryServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Search, request, options...)
}

// VectorModule is `loams.vector`.
type VectorModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool

	// Search is `loams.collection.v1.QueryService/Search`, retried: safe.
	Search(ctx context.Context, request *collectionv1.SearchRequest, options ...CallOption) (*collectionv1.SearchResponse, error)
}

type vectorModule struct{ moduleInfo }

func (m *vectorModule) Search(ctx context.Context, request *collectionv1.SearchRequest, options ...CallOption) (*collectionv1.SearchResponse, error) {
	binding, err := m.binding("Search")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[collectionv1connect.QueryServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Search, request, options...)
}
