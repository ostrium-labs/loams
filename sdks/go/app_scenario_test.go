// SDKP Parts B and C for Go: the app scenario through the public facade.
//
// go_app_collections, go_app_search, go_app_graph, and (Part C) go_app_sql and
// go_app_streams replay
// sdks/fixtures/recorded/app/ against the shared fixture server
// (sdks/fixtures/app-scenario.md says how): each step's facade call is made on
// a Client with the recorded request — built with the builders when the step
// names a builder case — and the fixture server compares the body it receives
// with the recording. The answer is compared with the recorded one, or, for a
// refusal, the typed error's code and reason. They always use the fixture
// server: a live `loams dev` cannot replay a recording, and replaying against
// one would create the `app` namespace a `--drift` re-record needs fresh.

package loams

import (
	"context"
	"encoding/json"
	"fmt"
	"os"
	"path/filepath"
	"strconv"
	"testing"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
	graphv1 "loams.dev/go/gen/loams/graph/v1"
	operationsv1 "loams.dev/go/gen/loams/operations/v1"
	sqlv1 "loams.dev/go/gen/loams/sql/v1"
	streamv1 "loams.dev/go/gen/loams/stream/v1"
)

// Canonical test names, for check-languages --suite app.
const (
	AppCollections = "go_app_collections"
	AppSearch      = "go_app_search"
	AppGraph       = "go_app_graph"
	AppSql         = "go_app_sql"
	AppStreams     = "go_app_streams"
)

type appStep struct {
	Call    string `json:"call"`
	Builder string `json:"builder"`
	Request struct {
		Body string `json:"body"`
	} `json:"request"`
	Response struct {
		Status int             `json:"status"`
		Body   json.RawMessage `json:"body"`
	} `json:"response"`
	Expect struct {
		Status int    `json:"status"`
		Reason string `json:"reason"`
	} `json:"expect"`
}

type appFixture struct {
	Name  string    `json:"name"`
	Steps []appStep `json:"steps"`
}

func readAppFixture(t *testing.T, name string) appFixture {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(fixturesDir, "recorded", "app", name+".json"))
	if err != nil {
		t.Fatal(err)
	}
	var fixture appFixture
	if err := json.Unmarshal(raw, &fixture); err != nil {
		t.Fatal(err)
	}
	return fixture
}

// appCall is one facade call over a decoded request.
type appCall struct {
	request func() proto.Message
	answer  func() proto.Message
	call    func(ctx context.Context, c *Client, request proto.Message, options []CallOption) (proto.Message, error)
}

func unaryCall[Req, Res any, PReq interface {
	*Req
	proto.Message
}, PRes interface {
	*Res
	proto.Message
}](method func(c *Client) func(context.Context, *Req, ...CallOption) (*Res, error)) appCall {
	return appCall{
		request: func() proto.Message { return PReq(new(Req)) },
		answer:  func() proto.Message { return PRes(new(Res)) },
		call: func(ctx context.Context, c *Client, request proto.Message, options []CallOption) (proto.Message, error) {
			answer, err := method(c)(ctx, request.(PReq), options...)
			if err != nil {
				return nil, err
			}
			return PRes(answer), nil
		},
	}
}

// appCalls are the facade calls the scenario makes, by `module.call`.
var appCalls = map[string]appCall{
	"instance.get_instance": unaryCall[GetInstanceRequest, GetInstanceResponse](func(c *Client) func(context.Context, *GetInstanceRequest, ...CallOption) (*GetInstanceResponse, error) {
		return c.Instance().GetInstance
	}),
	"namespaces.create": unaryCall[collectionv1.CreateNamespaceRequest, collectionv1.CreateNamespaceResponse](func(c *Client) func(context.Context, *collectionv1.CreateNamespaceRequest, ...CallOption) (*collectionv1.CreateNamespaceResponse, error) {
		return c.Namespaces().Create
	}),
	"collections.create": unaryCall[collectionv1.CreateCollectionRequest, collectionv1.CollectionInfo](func(c *Client) func(context.Context, *collectionv1.CreateCollectionRequest, ...CallOption) (*collectionv1.CollectionInfo, error) {
		return c.Collections().Create
	}),
	"collections.get": unaryCall[collectionv1.GetCollectionRequest, collectionv1.CollectionInfo](func(c *Client) func(context.Context, *collectionv1.GetCollectionRequest, ...CallOption) (*collectionv1.CollectionInfo, error) {
		return c.Collections().Get
	}),
	"collections.list": unaryCall[collectionv1.ListCollectionsRequest, collectionv1.ListCollectionsResponse](func(c *Client) func(context.Context, *collectionv1.ListCollectionsRequest, ...CallOption) (*collectionv1.ListCollectionsResponse, error) {
		return c.Collections().List
	}),
	"collections.drop": unaryCall[collectionv1.DropCollectionRequest, collectionv1.DropCollectionResponse](func(c *Client) func(context.Context, *collectionv1.DropCollectionRequest, ...CallOption) (*collectionv1.DropCollectionResponse, error) {
		return c.Collections().Drop
	}),
	"documents.write": unaryCall[collectionv1.WriteDocumentsRequest, collectionv1.WriteDocumentsResponse](func(c *Client) func(context.Context, *collectionv1.WriteDocumentsRequest, ...CallOption) (*collectionv1.WriteDocumentsResponse, error) {
		return c.Documents().Write
	}),
	"documents.get": unaryCall[collectionv1.GetDocumentsRequest, collectionv1.GetDocumentsResponse](func(c *Client) func(context.Context, *collectionv1.GetDocumentsRequest, ...CallOption) (*collectionv1.GetDocumentsResponse, error) {
		return c.Documents().Get
	}),
	"documents.count": unaryCall[collectionv1.CountDocumentsRequest, collectionv1.CountDocumentsResponse](func(c *Client) func(context.Context, *collectionv1.CountDocumentsRequest, ...CallOption) (*collectionv1.CountDocumentsResponse, error) {
		return c.Documents().Count
	}),
	"documents.scroll": unaryCall[collectionv1.ScrollDocumentsRequest, collectionv1.ScrollDocumentsResponse](func(c *Client) func(context.Context, *collectionv1.ScrollDocumentsRequest, ...CallOption) (*collectionv1.ScrollDocumentsResponse, error) {
		return c.Documents().Scroll
	}),
	"search.query": unaryCall[collectionv1.SearchRequest, collectionv1.SearchResponse](func(c *Client) func(context.Context, *collectionv1.SearchRequest, ...CallOption) (*collectionv1.SearchResponse, error) {
		return c.Search().Query
	}),
	"vector.search": unaryCall[collectionv1.SearchRequest, collectionv1.SearchResponse](func(c *Client) func(context.Context, *collectionv1.SearchRequest, ...CallOption) (*collectionv1.SearchResponse, error) {
		return c.Vector().Search
	}),
	"graph.create": unaryCall[graphv1.CreateGraphRequest, graphv1.Graph](func(c *Client) func(context.Context, *graphv1.CreateGraphRequest, ...CallOption) (*graphv1.Graph, error) {
		return c.Graph().Create
	}),
	"graph.query": unaryCall[graphv1.ExecuteRequest, graphv1.ExecuteResponse](func(c *Client) func(context.Context, *graphv1.ExecuteRequest, ...CallOption) (*graphv1.ExecuteResponse, error) {
		return c.Graph().Query
	}),
	"graph.delete": unaryCall[graphv1.DeleteGraphRequest, operationsv1.Operation](func(c *Client) func(context.Context, *graphv1.DeleteGraphRequest, ...CallOption) (*operationsv1.Operation, error) {
		return c.Graph().Delete
	}),
	"sql.query": unaryCall[sqlv1.QueryRequest, sqlv1.QueryResponse](func(c *Client) func(context.Context, *sqlv1.QueryRequest, ...CallOption) (*sqlv1.QueryResponse, error) {
		return c.Sql().Query
	}),
	"sql.query_arrow": unaryCall[sqlv1.QueryArrowRequest, sqlv1.QueryArrowResponse](func(c *Client) func(context.Context, *sqlv1.QueryArrowRequest, ...CallOption) (*sqlv1.QueryArrowResponse, error) {
		return c.Sql().QueryArrow
	}),
	"streams.create": unaryCall[streamv1.CreateStreamRequest, streamv1.CreateStreamResponse](func(c *Client) func(context.Context, *streamv1.CreateStreamRequest, ...CallOption) (*streamv1.CreateStreamResponse, error) {
		return c.Streams().Create
	}),
	"streams.get": unaryCall[streamv1.DescribeStreamRequest, streamv1.DescribeStreamResponse](func(c *Client) func(context.Context, *streamv1.DescribeStreamRequest, ...CallOption) (*streamv1.DescribeStreamResponse, error) {
		return c.Streams().Get
	}),
	"streams.list": unaryCall[streamv1.ListStreamsRequest, streamv1.ListStreamsResponse](func(c *Client) func(context.Context, *streamv1.ListStreamsRequest, ...CallOption) (*streamv1.ListStreamsResponse, error) {
		return c.Streams().List
	}),
	"streams.produce": unaryCall[streamv1.ProduceRequest, streamv1.ProduceResponse](func(c *Client) func(context.Context, *streamv1.ProduceRequest, ...CallOption) (*streamv1.ProduceResponse, error) {
		return c.Streams().Produce
	}),
	"streams.fetch": unaryCall[streamv1.FetchRequest, streamv1.FetchResponse](func(c *Client) func(context.Context, *streamv1.FetchRequest, ...CallOption) (*streamv1.FetchResponse, error) {
		return c.Streams().Fetch
	}),
	"streams.produce_events": unaryCall[streamv1.ProduceCloudEventsRequest, streamv1.ProduceCloudEventsResponse](func(c *Client) func(context.Context, *streamv1.ProduceCloudEventsRequest, ...CallOption) (*streamv1.ProduceCloudEventsResponse, error) {
		return c.Streams().ProduceEvents
	}),
	"streams.fetch_events": unaryCall[streamv1.FetchCloudEventsRequest, streamv1.FetchCloudEventsResponse](func(c *Client) func(context.Context, *streamv1.FetchCloudEventsRequest, ...CallOption) (*streamv1.FetchCloudEventsResponse, error) {
		return c.Streams().FetchEvents
	}),
}

// builtRequest is the step's request, built with the builders where it names
// a builder case.
func builtRequest(t *testing.T, step appStep, request proto.Message) error {
	t.Helper()
	if err := protojson.Unmarshal([]byte(step.Request.Body), request); err != nil {
		return err
	}
	if step.Builder == "" {
		return nil
	}
	var entry builderCase
	for _, c := range loadBuilderCases(t) {
		if c.Name == step.Builder {
			entry = c
		}
	}
	switch entry.Builder {
	case "search":
		var spec struct {
			On    string       `json:"on"`
			Calls []corpusCall `json:"calls"`
		}
		if err := json.Unmarshal(entry.Spec, &spec); err != nil {
			return err
		}
		built, err := buildSearch(spec)
		if err != nil {
			return err
		}
		// The builder writes no namespace: the client's default fills it (R11).
		proto.Reset(request)
		proto.Merge(request, built)
	case "schema":
		var calls []corpusCall
		if err := json.Unmarshal(entry.Spec, &calls); err != nil {
			return err
		}
		schema, err := buildSchema(calls)
		if err != nil {
			return err
		}
		value, err := structpb.NewStruct(roundTrip(schema))
		if err != nil {
			return err
		}
		request.(*collectionv1.CreateCollectionRequest).Schema = value
	case "filter":
		var call corpusCall
		if err := json.Unmarshal(entry.Spec, &call); err != nil {
			return err
		}
		filter, err := evaluateFilter(call)
		if err != nil {
			return err
		}
		value, err := structpb.NewStruct(roundTrip(filter))
		if err != nil {
			return err
		}
		request.(*collectionv1.CountDocumentsRequest).Where = value
	}
	return nil
}

// roundTrip is a builder's JSON as plain JSON values, which is what structpb
// accepts (a builder may hold typed numbers and slices).
func roundTrip(value any) map[string]any {
	raw, _ := json.Marshal(value)
	var out map[string]any
	_ = json.Unmarshal(raw, &out)
	return out
}

func replayApp(t *testing.T, endpoint, name string) error {
	t.Helper()
	client, err := New(Options{
		Endpoint:  endpoint,
		Namespace: "app",
		NoRetries: true,
		Transport: TransportConfig{ConnectOptions: []connect.ClientOption{connect.WithProtoJSON()}},
	})
	if err != nil {
		return err
	}
	ctx := context.Background()
	for at, step := range readAppFixture(t, name).Steps {
		place := fmt.Sprintf("%s step %d (%s)", name, at, step.Call)
		call, ok := appCalls[step.Call]
		if !ok {
			return fmt.Errorf("%s: no replay for the call", place)
		}
		request := call.request()
		if err := builtRequest(t, step, request); err != nil {
			return fmt.Errorf("%s: %w", place, err)
		}
		options := []CallOption{
			WithHeader("loams-fixture-name", name),
			WithHeader("loams-fixture-step", strconv.Itoa(at)),
		}
		answer, err := call.call(ctx, client, request, options)
		if step.Expect.Status == 200 {
			if err != nil {
				return fmt.Errorf("%s: %w", place, err)
			}
			want := call.answer()
			if err := protojson.Unmarshal(step.Response.Body, want); err != nil {
				return fmt.Errorf("%s: the recording does not decode: %w", place, err)
			}
			if !proto.Equal(answer, want) {
				return fmt.Errorf("%s: answered %v, recorded %v", place, answer, want)
			}
			continue
		}
		if err == nil {
			return fmt.Errorf("%s: expected a refusal, got %v", place, answer)
		}
		if got := string(ReasonOf(err)); got != step.Expect.Reason {
			return fmt.Errorf("%s: reason %q, recorded %q", place, got, step.Expect.Reason)
		}
		var recorded struct {
			Code string `json:"code"`
		}
		_ = json.Unmarshal(step.Response.Body, &recorded)
		if code := connect.CodeOf(err).String(); recorded.Code != "" && code != recorded.Code {
			return fmt.Errorf("%s: code %s, recorded %s", place, code, recorded.Code)
		}
	}
	return nil
}

func replayOnOwnServer(t *testing.T, name string) {
	t.Helper()
	server, err := startNodeFixtureServer(t)
	if err != nil {
		t.Skipf("the shared fixture server needs node: %v", err)
	}
	t.Cleanup(server.stop)
	if err := replayApp(t, server.endpoint, name); err != nil {
		t.Fatal(err)
	}
}

func TestGoAppCollections(t *testing.T) {
	t.Run(AppCollections, func(t *testing.T) { replayOnOwnServer(t, "app_collections") })
}

func TestGoAppSearch(t *testing.T) {
	t.Run(AppSearch, func(t *testing.T) { replayOnOwnServer(t, "app_search") })
}

func TestGoAppGraph(t *testing.T) {
	t.Run(AppGraph, func(t *testing.T) { replayOnOwnServer(t, "app_graph") })
}

func TestGoAppSql(t *testing.T) {
	t.Run(AppSql, func(t *testing.T) { replayOnOwnServer(t, "app_sql") })
}

func TestGoAppStreams(t *testing.T) {
	t.Run(AppStreams, func(t *testing.T) { replayOnOwnServer(t, "app_streams") })
}

// Streams().Subscribe reads the recorded fetch page of app_streams and knows
// where to resume.
func TestGoStreamsSubscribeReadsTheRecordedPage(t *testing.T) {
	server, err := startNodeFixtureServer(t)
	if err != nil {
		t.Skipf("the shared fixture server needs node: %v", err)
	}
	t.Cleanup(server.stop)
	client, err := New(Options{
		Endpoint:  server.endpoint,
		Namespace: "app",
		NoRetries: true,
		Transport: TransportConfig{ConnectOptions: []connect.ClientOption{connect.WithProtoJSON()}},
	})
	if err != nil {
		t.Fatal(err)
	}
	maxRecords := uint32(10)
	sub := client.Streams().Subscribe(context.Background(),
		&streamv1.FetchRequest{Stream: "note-events", MaxRecords: &maxRecords},
		WithHeader("loams-fixture-name", "app_streams"), WithHeader("loams-fixture-step", "5"))
	defer sub.Close()
	var offsets []uint64
	for len(offsets) < 2 && sub.Next() {
		offsets = append(offsets, sub.Record().GetOffset())
	}
	if err := sub.Err(); err != nil {
		t.Fatal(err)
	}
	if fmt.Sprint(offsets) != "[0 1]" || sub.Offset() != 2 {
		t.Fatalf("offsets %v, resume at %d; want [0 1] and 2", offsets, sub.Offset())
	}
}

// Sql().Records sends the `$1` value and reads the rows as records, against
// the recorded first step of app_sql.
func TestGoSqlRecords(t *testing.T) {
	server, err := startNodeFixtureServer(t)
	if err != nil {
		t.Skipf("the shared fixture server needs node: %v", err)
	}
	t.Cleanup(server.stop)
	client, err := New(Options{
		Endpoint:  server.endpoint,
		Namespace: "app",
		NoRetries: true,
		Transport: TransportConfig{ConnectOptions: []connect.ClientOption{connect.WithProtoJSON()}},
	})
	if err != nil {
		t.Fatal(err)
	}
	records, err := client.Sql().Records(context.Background(),
		"SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", []any{1},
		WithHeader("loams-fixture-name", "app_sql"), WithHeader("loams-fixture-step", "0"))
	if err != nil {
		t.Fatal(err)
	}
	want := []map[string]any{{"tags": "policy", "c": 2.0}, {"tags": "shipping", "c": 1.0}}
	if fmt.Sprint(records) != fmt.Sprint(want) {
		t.Fatalf("records %v, want %v", records, want)
	}
}

// appFixtureNames are the app fixtures the manifest names.
func appFixtureNames(t *testing.T) []string {
	t.Helper()
	raw, err := os.ReadFile(filepath.Join(fixturesDir, "manifest.json"))
	if err != nil {
		t.Fatal(err)
	}
	var manifest struct {
		Fixtures []struct {
			Name string `json:"name"`
		} `json:"fixtures"`
	}
	if err := json.Unmarshal(raw, &manifest); err != nil {
		t.Fatal(err)
	}
	var names []string
	for _, f := range manifest.Fixtures {
		if len(f.Name) > 4 && f.Name[:4] == "app_" {
			names = append(names, f.Name)
		}
	}
	return names
}

// TestGoAppListCollectionsPaginates is the end-to-end half of
// go_pagination_iterator for the app's paged calls: `collections.List` paged
// one at a time through PaginateCall against the recording of
// `app_collections` steps 6 and 7.
func TestGoAppListCollectionsPaginates(t *testing.T) {
	server, err := startNodeFixtureServer(t)
	if err != nil {
		t.Skipf("the shared fixture server needs node: %v", err)
	}
	t.Cleanup(server.stop)
	client, err := New(Options{
		Endpoint: server.endpoint, Namespace: "app", NoRetries: true,
		Transport: TransportConfig{ConnectOptions: []connect.ClientOption{connect.WithProtoJSON()}},
	})
	if err != nil {
		t.Fatal(err)
	}
	step := 6
	fetch := func(ctx context.Context, request *collectionv1.ListCollectionsRequest, options ...CallOption) (*collectionv1.ListCollectionsResponse, error) {
		options = append(options,
			WithHeader("loams-fixture-name", "app_collections"),
			WithHeader("loams-fixture-step", strconv.Itoa(step)))
		step++
		return client.Collections().List(ctx, request, options...)
	}
	items, err := PaginateCall[collectionv1.ListCollectionsRequest, collectionv1.ListCollectionsResponse, *collectionv1.CollectionInfo](
		client, "collections", "List", fetch, context.Background(),
		&collectionv1.ListCollectionsRequest{Namespace: "app", PageSize: 1})
	if err != nil {
		t.Fatal(err)
	}
	var names []string
	for info := range items.Seq() {
		names = append(names, info.GetName())
	}
	if items.Err() != nil {
		t.Fatal(items.Err())
	}
	if len(names) != 2 || names[0] != "notes" || names[1] != "scratch" {
		t.Errorf("paged %v, want [notes scratch]", names)
	}
}
