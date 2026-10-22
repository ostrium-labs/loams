// `Documents.WriteBatch` (runtime contract R13, SDKP §2.3; design §44 §7.5).
//
// A bulk load as a loop of ordinary `WriteDocuments` calls, chunked by
// operations and by bytes:
//
//   - **One chunk in flight at a time**, so order is kept and a failure stops
//     the load at a known point.
//   - **One idempotency key per chunk**, minted before the chunk's first attempt
//     and reused on every retry of that chunk (D610), so a retried chunk is the
//     same write and two chunks are never mistaken for one.
//   - **The tokens merge**: the result's token is the merge of every chunk's,
//     so a read held to it sees the whole load.
//   - **The first failed chunk stops the load** with a `*BatchError` that says
//     how many chunks and operations were applied before it, and carries their
//     merged token. Those chunks stay applied; nothing is rolled back.

package loams

import (
	"context"
	"encoding/json"
	"errors"
	"fmt"
	"iter"
	"strconv"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
)

// The default chunk bounds (§44 §7.5).
const (
	DefaultChunkRows  = 10_000
	DefaultChunkBytes = 4 << 20
)

// BatchOptions bound the chunks. Zero means the default.
type BatchOptions struct {
	// ChunkRows is the most operations in one chunk.
	ChunkRows int
	// ChunkBytes is the most encoded bytes in one chunk. A single operation
	// larger than this goes in a chunk of its own.
	ChunkBytes int
}

// BatchResult is what a finished batch reports.
type BatchResult struct {
	// Token is the merge of every chunk's consistency token.
	Token string
	// Chunks is how many chunks were written; Rows how many operations.
	Chunks, Rows int
}

// BatchError is a batch that stopped at a failed chunk. The chunks before it
// are applied; Token is their merged token; the failed chunk's own error is
// the cause, so `errors.As` still finds its class.
type BatchError struct {
	LoamsError
	// ChunksApplied and RowsApplied are what landed before the failure.
	ChunksApplied, RowsApplied int
	// Token is the merged token of the applied chunks, or "".
	Token string
}

// ChunkWriter sends one chunk under its own idempotency key and answers the
// chunk's consistency token.
type ChunkWriter[Op any] func(ctx context.Context, ops []Op, idempotencyKey string) (token string, err error)

// sizeOf is an operation's encoded size: the wire size of a proto message, the
// JSON size of anything else.
func sizeOf(op any) int {
	if message, ok := op.(proto.Message); ok {
		return proto.Size(message)
	}
	raw, err := json.Marshal(op)
	if err != nil {
		return 0
	}
	return len(raw)
}

// WriteBatch splits ops into chunks and writes them one after another.
func WriteBatch[Op any](ctx context.Context, write ChunkWriter[Op], ops iter.Seq[Op], options BatchOptions) (BatchResult, error) {
	rowsLimit, bytesLimit := options.ChunkRows, options.ChunkBytes
	if rowsLimit <= 0 {
		rowsLimit = DefaultChunkRows
	}
	if bytesLimit <= 0 {
		bytesLimit = DefaultChunkBytes
	}
	var (
		token        ConsistencyToken
		hasToken     bool
		result       BatchResult
		pending      []Op
		pendingBytes int
	)
	flush := func() error {
		if len(pending) == 0 {
			return nil
		}
		chunk := pending
		pending, pendingBytes = nil, 0
		// The key is minted here, once per chunk, and the invoker reuses it on
		// every retry of this chunk.
		answer, err := write(ctx, chunk, UUIDv7())
		if err != nil {
			cause := ToLoamsError(err, "")
			var base *LoamsError
			errors.As(cause, &base)
			code := connect.CodeAborted
			rpc := ""
			if base != nil {
				code, rpc = base.Code, base.RPC
				if code == connect.CodeUnknown {
					code = connect.CodeAborted
				}
			}
			applied := ""
			if hasToken {
				applied = token.String()
			}
			metadata := map[string]string{
				"chunks_applied": strconv.Itoa(result.Chunks),
				"rows_applied":   strconv.Itoa(result.Rows),
			}
			if reason := ReasonOf(cause); reason != "" {
				metadata["cause_reason"] = string(reason)
			}
			return &BatchError{
				LoamsError: LoamsError{
					Code:     code,
					Reason:   ReasonBatchPartiallyApplied,
					RPC:      rpc,
					Metadata: metadata,
					Cause: fmt.Errorf("WriteBatch stopped at chunk %d: %d chunks (%d operations) were applied before it: %w",
						result.Chunks+1, result.Chunks, result.Rows, cause),
				},
				ChunksApplied: result.Chunks,
				RowsApplied:   result.Rows,
				Token:         applied,
			}
		}
		if parsed, parseErr := ParseConsistencyToken(answer); answer != "" && parseErr == nil {
			if hasToken {
				token = token.Merge(parsed)
			} else {
				token, hasToken = parsed.Normalized(), true
			}
		}
		result.Chunks++
		result.Rows += len(chunk)
		return nil
	}
	for op := range ops {
		size := sizeOf(op)
		if len(pending) > 0 && (len(pending) >= rowsLimit || pendingBytes+size > bytesLimit) {
			if err := flush(); err != nil {
				return result, err
			}
		}
		pending = append(pending, op)
		pendingBytes += size
	}
	if err := flush(); err != nil {
		return result, err
	}
	if hasToken {
		result.Token = token.String()
	}
	return result, nil
}
