// Command notes is the notes app (SDKP Part D): the app scenario of
// sdks/fixtures/app-scenario.md as a program, through the public API.
//
//	export LOAMS_ENDPOINT=http://127.0.0.1:8080
//	export LOAMS_NAMESPACE=app
//	go run ./examples/notes
//
// `go vet ./...` builds it, so it cannot drift from the API; running it needs a
// `loams dev`.
package main

import (
	"context"
	"errors"
	"fmt"
	"iter"
	"log"

	loams "loams.dev/go"
	collectionv1 "loams.dev/go/gen/loams/collection/v1"
	streamv1 "loams.dev/go/gen/loams/stream/v1"
)

func main() {
	if err := run(context.Background()); err != nil {
		log.Fatal(err)
	}
}

func run(ctx context.Context) error {
	// LOAMS_ENDPOINT, LOAMS_API_KEY (or LOAMS_TOKEN) and LOAMS_NAMESPACE;
	// reads see this process's own writes.
	client, err := loams.FromEnv(loams.Options{SessionConsistency: true})
	if err != nil {
		return err
	}
	namespace := client.Namespace()
	if namespace == "" {
		namespace = "app"
	}
	if _, err := client.Namespaces().Create(ctx, &collectionv1.CreateNamespaceRequest{Namespace: namespace}); err != nil && !alreadyExists(err) {
		return err
	}
	schema, err := loams.NewSchema().Text("title").Text("body").Keyword("tags").I64("n").Vector("embedding", 4).Struct()
	if err != nil {
		return err
	}
	if _, err := client.Collections().Create(ctx, &collectionv1.CreateCollectionRequest{Namespace: namespace, Name: "notes", Schema: schema}); err != nil && !alreadyExists(err) {
		return err
	}

	notes := client.Collection("notes", namespace)
	if _, err := notes.Upsert(ctx, []loams.DocumentInput{
		{ID: loams.UintID(1), Source: loams.JSONObject{"title": "Refund policy", "body": "refund policy for every order", "tags": "policy", "n": 1}, Vectors: map[string][]float32{"embedding": {1, 0, 0, 0}}},
		{ID: loams.UintID(2), Source: loams.JSONObject{"title": "Refund window", "body": "the refund window is thirty days", "tags": "policy", "n": 2}, Vectors: map[string][]float32{"embedding": {0, 0, 1, 0}}},
	}); err != nil {
		return err
	}

	// Hybrid: text and vector, fused with RRF, filtered, highlighted, projected.
	hits, err := notes.Search(ctx, notes.Query().
		Text("refund", loams.TextOptions{Fields: []string{"body"}}).
		Vector("embedding", []float32{1, 0, 0, 0}, loams.VectorOptions{K: 10}).
		Fuse(loams.RRF(60)).
		Filter(loams.Filter.Eq("tags", "policy")).
		Highlight("body").
		Select(loams.SelectOptions{Include: []string{"title"}}).
		Limit(3))
	if err != nil {
		return err
	}
	fmt.Println(len(hits.GetHits()), "hits")

	// Every policy note, page by page (the sort ends in the primary key).
	all, err := client.SearchCalls().QueryAll(ctx, loams.NewSearch("notes").
		Text("refund", loams.TextOptions{Fields: []string{"body"}}).
		Sort(loams.Sort.Score(), loams.Sort.PK()).
		Limit(2))
	if err != nil {
		return err
	}
	for hit := range all.Seq() {
		fmt.Println("page hit", hit.GetPk())
	}
	if all.Err() != nil {
		return all.Err()
	}

	policy, err := notes.Count(ctx, loams.Filter.Eq("tags", "policy"))
	if err != nil {
		return err
	}
	fmt.Println("policy notes:", policy.GetCount())

	// SQL over the same collection, with a `$1` parameter, as records.
	perTag, err := client.Sql().Records(ctx,
		"SELECT tags, count(*) AS c FROM notes WHERE n > $1 GROUP BY tags ORDER BY tags", []any{1})
	if err != nil {
		return err
	}
	for _, row := range perTag {
		fmt.Println(row["tags"], row["c"])
	}

	// A stream of note events: produce with a producer key, then read it back.
	if _, err := client.Streams().Create(ctx, &streamv1.CreateStreamRequest{Name: "note-events", Partitions: 1}); err != nil {
		return err
	}
	producer, seq := "notes-example", uint64(1)
	if _, err := client.Streams().Produce(ctx, &streamv1.ProduceRequest{
		Stream:      "note-events",
		Records:     []*streamv1.Record{{Value: []byte(`{"note":1,"op":"created"}`)}},
		ProducerId:  &producer,
		ProducerSeq: &seq,
	}); err != nil {
		return err
	}
	events := client.Streams().Subscribe(ctx, &streamv1.FetchRequest{Stream: "note-events"})
	defer events.Close()
	if events.Next() {
		fmt.Println("event at", events.Record().GetOffset())
	}
	if err := events.Err(); err != nil {
		return err
	}

	// A bulk load: chunked, one idempotency key per chunk, the tokens merged.
	loaded, err := notes.WriteBatch(ctx, bulk(100), loams.BatchOptions{ChunkRows: 25})
	if err != nil {
		return err
	}
	fmt.Printf("wrote %d notes in %d chunks\n", loaded.Rows, loaded.Chunks)
	return nil
}

func bulk(n int) iter.Seq[loams.DocumentInput] {
	return func(yield func(loams.DocumentInput) bool) {
		for i := range n {
			if !yield(loams.DocumentInput{ID: loams.UintID(uint64(1000 + i)), Source: loams.JSONObject{"title": fmt.Sprintf("note %d", i), "tags": "bulk", "n": i}}) {
				return
			}
		}
	}
}

// alreadyExists is a create that found the thing already there: done, not
// failed.
func alreadyExists(err error) bool {
	var exists *loams.AlreadyExistsError
	return errors.As(err, &exists)
}
