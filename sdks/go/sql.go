// `loams.sql` (SDKP §2.5, Part C): one read-only SQL statement over a
// namespace's collections, with `$1…$n` parameters. The two calls are the
// facade's (`gen/facade`, checked against the index by
// `go_facade_matches_index`); `Records` and `SqlRecords` are the hand-written
// conveniences.

package loams

import (
	"context"
	"fmt"

	"google.golang.org/protobuf/types/known/structpb"

	sqlv1 "loams.dev/go/gen/loams/sql/v1"
	sqlv1connect "loams.dev/go/gen/loams/sql/v1/sqlv1connect"
)

// SqlModule is `loams.sql`.
type SqlModule interface {
	// Module is the SDK module, as it appears on the client.
	Module() string
	// Service is the service behind the module.
	Service() string
	// Unstable is whether the package's wire contract may still change.
	Unstable() bool
	// Query is `loams.sql.v1.SqlService/Query`, retried: safe.
	Query(ctx context.Context, request *sqlv1.QueryRequest, options ...CallOption) (*sqlv1.QueryResponse, error)
	// QueryArrow is `loams.sql.v1.SqlService/QueryArrow`, retried: safe. The
	// answer's ArrowIpc is one Arrow IPC stream (apache/arrow-go's
	// ipc.NewReader reads it); the SDK does not depend on Arrow.
	QueryArrow(ctx context.Context, request *sqlv1.QueryArrowRequest, options ...CallOption) (*sqlv1.QueryArrowResponse, error)
	// Records runs one statement with its `$1…$n` values (Go scalars, which
	// the server casts to each placeholder's inferred type) and answers its
	// rows as records (SqlRecords).
	Records(ctx context.Context, statement string, params []any, options ...CallOption) ([]map[string]any, error)
}

type sqlModule struct{ moduleInfo }

func (m *sqlModule) Query(ctx context.Context, request *sqlv1.QueryRequest, options ...CallOption) (*sqlv1.QueryResponse, error) {
	binding, err := m.binding("Query")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[sqlv1connect.SqlServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.Query, request, options...)
}

func (m *sqlModule) QueryArrow(ctx context.Context, request *sqlv1.QueryArrowRequest, options ...CallOption) (*sqlv1.QueryArrowResponse, error) {
	binding, err := m.binding("QueryArrow")
	if err != nil {
		return nil, err
	}
	client, err := clientOf[sqlv1connect.SqlServiceClient](m.invoker, binding.Service)
	if err != nil {
		return nil, err
	}
	return Unary(m.invoker, ctx, binding, client.QueryArrow, request, options...)
}

func (m *sqlModule) Records(ctx context.Context, statement string, params []any, options ...CallOption) ([]map[string]any, error) {
	values := make([]*structpb.Value, 0, len(params))
	for at, param := range params {
		value, err := structpb.NewValue(param)
		if err != nil {
			return nil, invalidBuild("params", fmt.Errorf("sql parameter $%d: %w", at+1, err))
		}
		values = append(values, value)
	}
	answer, err := m.Query(ctx, &sqlv1.QueryRequest{Sql: statement, Params: values}, options...)
	if err != nil {
		return nil, err
	}
	return SqlRecords(answer), nil
}

// SqlRecords is a `sql.query` answer's rows as one map per row, keyed by
// column name. A value is its JSON form as structpb gives it (float64,
// string, bool, nil, []any, map[string]any): a 64-bit integer past 2^53 is the
// decimal string the server sent, so it stays exact. A column name that
// repeats keeps its last value.
func SqlRecords(answer *sqlv1.QueryResponse) []map[string]any {
	columns := answer.GetColumns()
	records := make([]map[string]any, 0, len(answer.GetRows()))
	for _, row := range answer.GetRows() {
		values := row.GetValues()
		record := make(map[string]any, len(columns))
		for at, column := range columns {
			if at < len(values) {
				record[column.GetName()] = values[at].AsInterface()
			} else {
				record[column.GetName()] = nil
			}
		}
		records = append(records, record)
	}
	return records
}
