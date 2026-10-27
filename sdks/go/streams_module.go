// `loams.streams` (SDKP §2.5, Part C): a namespace's logs. The seven calls are
// the facade's (`gen/facade`, checked against the index by
// `go_facade_matches_index`); `Subscribe` (R14) is the hand-written loop over
// `Fetch`.

package loams

import (
	"context"

	streamv1 "loams.dev/go/gen/loams/stream/v1"
	streamv1connect "loams.dev/go/gen/loams/stream/v1/streamv1connect"
)

// StreamsModule is `loams.streams`.
type StreamsModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool
	// Create is `loams.stream.v1.StreamService/CreateStream`, retried: safe
	// (idempotent by name).
	Create(ctx context.Context, request *streamv1.CreateStreamRequest, options ...CallOption) (*streamv1.CreateStreamResponse, error)
	// Fetch is `loams.stream.v1.StreamService/Fetch`, retried: safe.
	Fetch(ctx context.Context, request *streamv1.FetchRequest, options ...CallOption) (*streamv1.FetchResponse, error)
	// FetchEvents is `loams.stream.v1.StreamService/FetchCloudEvents`, retried: safe.
	FetchEvents(ctx context.Context, request *streamv1.FetchCloudEventsRequest, options ...CallOption) (*streamv1.FetchCloudEventsResponse, error)
	// Get is `loams.stream.v1.StreamService/DescribeStream`, retried: safe.
	Get(ctx context.Context, request *streamv1.DescribeStreamRequest, options ...CallOption) (*streamv1.DescribeStreamResponse, error)
	// List is `loams.stream.v1.StreamService/ListStreams`, retried: safe; paged
	// (ListAll follows the tokens).
	List(ctx context.Context, request *streamv1.ListStreamsRequest, options ...CallOption) (*streamv1.ListStreamsResponse, error)
	// ListAll is every stream of List, page after page.
	ListAll(ctx context.Context, request *streamv1.ListStreamsRequest, options ...CallOption) *PageIter[streamv1.ListStreamsRequest, streamv1.ListStreamsResponse, *streamv1.StreamInfo]
	// Produce is `loams.stream.v1.StreamService/Produce`, never retried by the
	// SDK: set ProducerId and ProducerSeq so a retry of yours cannot append twice.
	Produce(ctx context.Context, request *streamv1.ProduceRequest, options ...CallOption) (*streamv1.ProduceResponse, error)
	// ProduceEvents is `loams.stream.v1.StreamService/ProduceCloudEvents`, never
	// retried by the SDK.
	ProduceEvents(ctx context.Context, request *streamv1.ProduceCloudEventsRequest, options ...CallOption) (*streamv1.ProduceCloudEventsResponse, error)
	// Subscribe is every record of one partition from request.Offset on (R14):
	// a loop over Fetch whose Offset is where to resume.
	Subscribe(ctx context.Context, request *streamv1.FetchRequest, options ...CallOption) *Subscription[*streamv1.FetchedRecord]
}

type streamsModule struct{ moduleInfo }

func (m *streamsModule) client() (streamv1connect.StreamServiceClient, error) {
	return clientOf[streamv1connect.StreamServiceClient](m.invoker, streamv1connect.StreamServiceName)
}

func (m *streamsModule) Create(ctx context.Context, request *streamv1.CreateStreamRequest, options ...CallOption) (*streamv1.CreateStreamResponse, error) {
	binding, err := m.binding("Create")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.CreateStream, request, options...)
}

func (m *streamsModule) Fetch(ctx context.Context, request *streamv1.FetchRequest, options ...CallOption) (*streamv1.FetchResponse, error) {
	binding, err := m.binding("Fetch")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Fetch, request, options...)
}

func (m *streamsModule) FetchEvents(ctx context.Context, request *streamv1.FetchCloudEventsRequest, options ...CallOption) (*streamv1.FetchCloudEventsResponse, error) {
	binding, err := m.binding("FetchEvents")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.FetchCloudEvents, request, options...)
}

func (m *streamsModule) Get(ctx context.Context, request *streamv1.DescribeStreamRequest, options ...CallOption) (*streamv1.DescribeStreamResponse, error) {
	binding, err := m.binding("Get")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.DescribeStream, request, options...)
}

func (m *streamsModule) List(ctx context.Context, request *streamv1.ListStreamsRequest, options ...CallOption) (*streamv1.ListStreamsResponse, error) {
	binding, err := m.binding("List")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ListStreams, request, options...)
}

func (m *streamsModule) ListAll(ctx context.Context, request *streamv1.ListStreamsRequest, options ...CallOption) *PageIter[streamv1.ListStreamsRequest, streamv1.ListStreamsResponse, *streamv1.StreamInfo] {
	binding, _ := m.binding("List")
	return Paginate[streamv1.ListStreamsRequest, streamv1.ListStreamsResponse, *streamv1.StreamInfo](binding, m.List, ctx, request, options...)
}

func (m *streamsModule) Produce(ctx context.Context, request *streamv1.ProduceRequest, options ...CallOption) (*streamv1.ProduceResponse, error) {
	binding, err := m.binding("Produce")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Produce, request, options...)
}

func (m *streamsModule) ProduceEvents(ctx context.Context, request *streamv1.ProduceCloudEventsRequest, options ...CallOption) (*streamv1.ProduceCloudEventsResponse, error) {
	binding, err := m.binding("ProduceEvents")
	if err != nil {
		return nil, err
	}
	client, err := m.client()
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.ProduceCloudEvents, request, options...)
}

func (m *streamsModule) Subscribe(ctx context.Context, request *streamv1.FetchRequest, options ...CallOption) *Subscription[*streamv1.FetchedRecord] {
	fetch := func(ctx context.Context, offset uint64) ([]*streamv1.FetchedRecord, error) {
		page := &streamv1.FetchRequest{
			Namespace:  request.GetNamespace(),
			Stream:     request.GetStream(),
			Partition:  request.GetPartition(),
			Offset:     offset,
			MaxBytes:   request.MaxBytes,
			MaxRecords: request.MaxRecords,
			MaxWaitMs:  request.MaxWaitMs,
		}
		answer, err := m.Fetch(ctx, page, options...)
		if err != nil {
			return nil, err
		}
		return answer.GetRecords(), nil
	}
	return Subscribe(ctx, fetch, (*streamv1.FetchedRecord).GetOffset, request.GetOffset())
}
