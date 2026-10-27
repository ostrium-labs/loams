// The app builders (SDKP §2.4 and §2.6): `Schema`, `Filter` and `Search`.
//
//	request, err := loams.NewSearch("notes").
//	    Text("refund", loams.TextOptions{Fields: []string{"body"}}).
//	    Vector("embedding", []float32{0.1, 0.2, 0.3, 0.4}, loams.VectorOptions{K: 10}).
//	    Fuse(loams.RRF(60)).
//	    Filter(loams.Filter.And(loams.Filter.Eq("tags", "policy"), loams.Filter.Range("n", loams.Bounds{Gte: 2}))).
//	    Highlight("body").
//	    Select(loams.SelectOptions{Include: []string{"title"}}).
//	    Limit(5).
//	    Build()
//
// Every verb is pure: it returns a new builder and leaves the receiver as it
// was, so a base query can be shared and specialised. Each builder's output is
// **proto3 JSON** — exactly how the builder corpus (`sdks/fixtures/builders`)
// spells its expectations, and the same JSON the TypeScript builders produce —
// and `Build` decodes it with protojson into the generated message, refusing
// the requests the builder can tell are wrong before any network call.
//
// Zero means unset throughout the option structs: no `k`, no `limit` of 0. A
// caller who needs an explicit zero sets the field on the built message.

package loams

import (
	"encoding/json"
	"errors"
	"fmt"
	"strconv"
	"strings"

	connect "connectrpc.com/connect"
	"google.golang.org/protobuf/encoding/protojson"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/structpb"

	collectionv1 "loams.dev/go/gen/loams/collection/v1"
)

// JSONObject is a proto3 JSON object: a filter, a fusion, a sort key or a
// whole request, before it is decoded into the generated message.
type JSONObject = map[string]any

// FilterJSON is one filter, the proto3 JSON of a `loams.collection.v1.Query`.
type FilterJSON = JSONObject

// ---------------------------------------------------------------------------
// Document ids
// ---------------------------------------------------------------------------

// DocID is a document id: one of the three `DocumentId` arms.
type DocID struct {
	kind string // "uint", "string" or "uuid"
	uint uint64
	text string
}

// UintID is a numeric id, exact over the whole uint64 range.
func UintID(id uint64) DocID { return DocID{kind: "uint", uint: id} }

// StringID is a string id.
func StringID(id string) DocID { return DocID{kind: "string", text: id} }

// UUIDID is a UUID id, `8-4-4-4-12` in any case.
func UUIDID(id string) DocID { return DocID{kind: "uuid", text: id} }

// filterValue is the id as `Query.ids` spells it: `{"uint": "1"}`,
// `{"uuid": "…"}` or a bare string.
func (id DocID) filterValue() any {
	switch id.kind {
	case "uint":
		return JSONObject{"uint": strconv.FormatUint(id.uint, 10)}
	case "uuid":
		return JSONObject{"uuid": id.text}
	default:
		return id.text
	}
}

// documentIDJSON is the id as `DocumentId`'s proto3 JSON.
func (id DocID) documentIDJSON() JSONObject {
	switch id.kind {
	case "uint":
		return JSONObject{"uint": strconv.FormatUint(id.uint, 10)}
	case "uuid":
		return JSONObject{"uuid": id.text}
	default:
		return JSONObject{"string": id.text}
	}
}

// Proto is the id as the generated `DocumentId`.
func (id DocID) Proto() *collectionv1.DocumentId {
	switch id.kind {
	case "uint":
		return &collectionv1.DocumentId{Id: &collectionv1.DocumentId_Uint{Uint: id.uint}}
	case "uuid":
		return &collectionv1.DocumentId{Id: &collectionv1.DocumentId_Uuid{Uuid: id.text}}
	default:
		return &collectionv1.DocumentId{Id: &collectionv1.DocumentId_String_{String_: id.text}}
	}
}

// ---------------------------------------------------------------------------
// Filter
// ---------------------------------------------------------------------------

// Bounds are a range's bounds; a nil bound is open.
type Bounds struct {
	Gt, Gte, Lt, Lte any
}

// MatchOptions are a `match`'s options.
type MatchOptions struct {
	// Operator is "or" (the default) or "and".
	Operator           string
	MinimumShouldMatch string
	// Fuzziness is "auto", "0", "1" or "2"; empty is none.
	Fuzziness string
	Analyzer  string
}

// MultiMatchOptions are a `multi_match`'s options.
type MultiMatchOptions struct {
	// Kind is best_fields (the default), most_fields, cross_fields, phrase or
	// phrase_prefix.
	Kind       string
	Operator   string
	TieBreaker float32
}

// QueryStringOptions are a `query_string`'s options.
type QueryStringOptions struct {
	Fields   []string
	Operator string
}

// filterBuilders is the type of Filter.
type filterBuilders struct{}

// Filter is the filter builder: one verb per arm of `loams.collection.v1.Query`.
//
//	loams.Filter.And(loams.Filter.Eq("tags", "policy"), loams.Filter.Not(loams.Filter.Exists("archived")))
var Filter filterBuilders

func fuzzinessJSON(value string) JSONObject {
	if value == "auto" {
		return JSONObject{"auto": JSONObject{}}
	}
	edits, _ := strconv.Atoi(value)
	return JSONObject{"edits": edits}
}

func boolOperator(operator string) string {
	if operator == "and" {
		return "BOOL_OPERATOR_AND"
	}
	return "BOOL_OPERATOR_OR"
}

func filters(queries []FilterJSON) []any {
	out := make([]any, len(queries))
	for i, q := range queries {
		out[i] = q
	}
	return out
}

// All is every document.
func (filterBuilders) All() FilterJSON { return FilterJSON{"matchAll": JSONObject{}} }

// None is no document.
func (filterBuilders) None() FilterJSON { return FilterJSON{"matchNone": JSONObject{}} }

// Eq is a term query: the field equals value.
func (filterBuilders) Eq(field string, value any) FilterJSON {
	return FilterJSON{"term": JSONObject{"field": field, "value": value}}
}

// In is a terms query: the field equals any of values.
func (filterBuilders) In(field string, values ...any) FilterJSON {
	return FilterJSON{"terms": JSONObject{"field": field, "values": values}}
}

// Range is the field within the bounds.
func (filterBuilders) Range(field string, bounds Bounds) FilterJSON {
	query := JSONObject{"field": field}
	for key, value := range map[string]any{"gt": bounds.Gt, "gte": bounds.Gte, "lt": bounds.Lt, "lte": bounds.Lte} {
		if value != nil {
			query[key] = value
		}
	}
	return FilterJSON{"range": query}
}

// Exists is the field present, whatever its value.
func (filterBuilders) Exists(field string) FilterJSON {
	return FilterJSON{"exists": JSONObject{"field": field}}
}

// IsNull is the field null or absent.
func (filterBuilders) IsNull(field string) FilterJSON {
	return FilterJSON{"isNull": JSONObject{"field": field}}
}

// IsEmpty is the field an empty string, list or object.
func (filterBuilders) IsEmpty(field string) FilterJSON {
	return FilterJSON{"isEmpty": JSONObject{"field": field}}
}

// Prefix is the field starting with value.
func (filterBuilders) Prefix(field, value string) FilterJSON {
	return FilterJSON{"prefix": JSONObject{"field": field, "value": value}}
}

// Wildcard is the field matching a `*`/`?` pattern.
func (filterBuilders) Wildcard(field, pattern string) FilterJSON {
	return FilterJSON{"wildcard": JSONObject{"field": field, "pattern": pattern}}
}

// Fuzzy is the field within `fuzziness` edits ("auto", "0"–"2"; empty is the
// engine's default) of value.
func (filterBuilders) Fuzzy(field, value, fuzziness string) FilterJSON {
	query := JSONObject{"field": field, "value": value}
	if fuzziness != "" {
		query["fuzziness"] = fuzzinessJSON(fuzziness)
	}
	return FilterJSON{"fuzzy": query}
}

// Match is a full-text match on one field.
func (filterBuilders) Match(field, text string, options ...MatchOptions) FilterJSON {
	query := JSONObject{"field": field, "text": text}
	if len(options) > 0 {
		o := options[0]
		if o.Operator != "" {
			query["operator"] = o.Operator
		}
		if o.MinimumShouldMatch != "" {
			query["minimumShouldMatch"] = o.MinimumShouldMatch
		}
		if o.Fuzziness != "" {
			query["fuzziness"] = fuzzinessJSON(o.Fuzziness)
		}
		if o.Analyzer != "" {
			query["analyzer"] = o.Analyzer
		}
	}
	return FilterJSON{"match": query}
}

// Phrase is the words of text, in order, at most slop positions apart.
func (filterBuilders) Phrase(field, text string, slop uint32) FilterJSON {
	query := JSONObject{"field": field, "text": text}
	if slop > 0 {
		query["slop"] = slop
	}
	return FilterJSON{"matchPhrase": query}
}

// MultiMatch is a full-text match over several fields; "title^2" boosts one.
func (filterBuilders) MultiMatch(fields []string, text string, options ...MultiMatchOptions) FilterJSON {
	specs := make([]any, len(fields))
	for i, spec := range fields {
		entry := JSONObject{"field": spec}
		if at := strings.LastIndex(spec, "^"); at > 0 {
			if boost, err := strconv.ParseFloat(spec[at+1:], 64); err == nil {
				entry = JSONObject{"field": spec[:at], "boost": boost}
			}
		}
		specs[i] = entry
	}
	query := JSONObject{"fields": specs, "text": text}
	if len(options) > 0 {
		o := options[0]
		if o.Kind != "" {
			query["kind"] = "MULTI_MATCH_KIND_" + strings.ToUpper(o.Kind)
		}
		if o.Operator != "" {
			query["operator"] = boolOperator(o.Operator)
		}
		if o.TieBreaker != 0 {
			query["tieBreaker"] = o.TieBreaker
		}
	}
	return FilterJSON{"multiMatch": query}
}

// QueryString is the query-string syntax, over fields or the collection's
// defaults.
func (filterBuilders) QueryString(query string, options ...QueryStringOptions) FilterJSON {
	out := JSONObject{"query": query}
	if len(options) > 0 {
		if len(options[0].Fields) > 0 {
			out["defaultFields"] = options[0].Fields
		}
		if options[0].Operator != "" {
			out["defaultOperator"] = boolOperator(options[0].Operator)
		}
	}
	return FilterJSON{"queryString": out}
}

// IDs is the documents with these ids, and no others.
func (filterBuilders) IDs(ids ...DocID) FilterJSON {
	values := make([]any, len(ids))
	for i, id := range ids {
		values[i] = id.filterValue()
	}
	return FilterJSON{"ids": values}
}

// And is every query matching (a bool `must`).
func (filterBuilders) And(queries ...FilterJSON) FilterJSON {
	return FilterJSON{"bool": JSONObject{"must": filters(queries)}}
}

// Or is at least one query matching (a bool `should`).
func (filterBuilders) Or(queries ...FilterJSON) FilterJSON {
	return FilterJSON{"bool": JSONObject{"should": filters(queries)}}
}

// AnyOf is at least minimumShouldMatch of the queries matching.
func (filterBuilders) AnyOf(minimumShouldMatch string, queries ...FilterJSON) FilterJSON {
	return FilterJSON{"bool": JSONObject{"should": filters(queries), "minimumShouldMatch": minimumShouldMatch}}
}

// Not is none of the queries matching (a bool `must_not`).
func (filterBuilders) Not(queries ...FilterJSON) FilterJSON {
	return FilterJSON{"bool": JSONObject{"mustNot": filters(queries)}}
}

// Filter is every query matching, without scoring (a bool `filter`).
func (filterBuilders) Filter(queries ...FilterJSON) FilterJSON {
	return FilterJSON{"bool": JSONObject{"filter": filters(queries)}}
}

// Boost multiplies the query's score by boost.
func (filterBuilders) Boost(query FilterJSON, boost float32) FilterJSON {
	return FilterJSON{"boost": JSONObject{"query": query, "boost": boost}}
}

// ConstantScore gives every match of query the score.
func (filterBuilders) ConstantScore(query FilterJSON, score float32) FilterJSON {
	return FilterJSON{"constantScore": JSONObject{"query": query, "score": score}}
}

// decodeJSON marshals a JSON value and decodes it with protojson into message.
func decodeJSON(value any, message proto.Message) error {
	raw, err := json.Marshal(value)
	if err != nil {
		return err
	}
	return protojson.Unmarshal(raw, message)
}

// ToQuery decodes a filter into the generated `Query`.
func ToQuery(filter FilterJSON) (*collectionv1.Query, error) {
	query := &collectionv1.Query{}
	if err := decodeJSON(filter, query); err != nil {
		return nil, invalidBuild("filter", err)
	}
	return query, nil
}

func invalidBuild(field string, cause error) error {
	return &InvalidArgumentError{LoamsError{
		Code:     connect.CodeInvalidArgument,
		Reason:   ReasonInvalidArgument,
		Metadata: map[string]string{"field": field},
		Cause:    cause,
	}}
}

// ---------------------------------------------------------------------------
// Schema
// ---------------------------------------------------------------------------

// FieldOptions are what every schema field takes.
type FieldOptions struct {
	// Indexed is whether the field is in the inverted index; nil is true.
	Indexed *bool
	// Fast is whether the field has a columnar copy; nil is false for text and
	// true for every other kind.
	Fast *bool
	// SourcePath is where the value comes from in `_source`; empty is the name.
	SourcePath string
}

// SchemaTextOptions are a text field's options.
type SchemaTextOptions struct {
	FieldOptions
	// Analyzer is standard (the default), english, simple, whitespace or keyword.
	Analyzer string
	// NoPositions turns off positions, which phrase queries need.
	NoPositions bool
}

// Schema builds `CreateCollectionRequest.schema` in the REST route's schema
// JSON, in the shape `kb_schema()` sends: every field spelled out, and
// `dynamic` and `max_fields` always present.
type Schema struct {
	fields, vectors, sparse []any
	dynamic                 string
	maxFields               int
}

// NewSchema is an empty schema: dynamic "ignore", max_fields 1000.
func NewSchema() *Schema { return &Schema{dynamic: "ignore", maxFields: 1000} }

func (s *Schema) field(name string, kind any, fastByDefault bool, options FieldOptions) *Schema {
	indexed, fast, source := true, fastByDefault, name
	if options.Indexed != nil {
		indexed = *options.Indexed
	}
	if options.Fast != nil {
		fast = *options.Fast
	}
	if options.SourcePath != "" {
		source = options.SourcePath
	}
	s.fields = append(s.fields, JSONObject{
		"name": name, "source_path": source, "kind": kind, "indexed": indexed, "fast": fast,
	})
	return s
}

func first[T any](options []T) T {
	var zero T
	if len(options) == 0 {
		return zero
	}
	return options[0]
}

// Text is a full-text field.
func (s *Schema) Text(name string, options ...SchemaTextOptions) *Schema {
	o := first(options)
	analyzer := o.Analyzer
	if analyzer == "" {
		analyzer = "standard"
	}
	return s.field(name, JSONObject{"text": JSONObject{"analyzer": analyzer, "positions": !o.NoPositions}}, false, o.FieldOptions)
}

// Keyword is an exact-match string field.
func (s *Schema) Keyword(name string, options ...FieldOptions) *Schema {
	return s.field(name, "keyword", true, first(options))
}

// I64 is a signed 64-bit integer field.
func (s *Schema) I64(name string, options ...FieldOptions) *Schema {
	return s.field(name, "i64", true, first(options))
}

// F64 is a 64-bit float field.
func (s *Schema) F64(name string, options ...FieldOptions) *Schema {
	return s.field(name, "f64", true, first(options))
}

// Bool is a boolean field.
func (s *Schema) Bool(name string, options ...FieldOptions) *Schema {
	return s.field(name, "bool", true, first(options))
}

// Timestamp is an RFC 3339 timestamp field (the schema's `date` kind).
func (s *Schema) Timestamp(name string, options ...FieldOptions) *Schema {
	return s.field(name, "date", true, first(options))
}

// UUID is a UUID field.
func (s *Schema) UUID(name string, options ...FieldOptions) *Schema {
	return s.field(name, "uuid", true, first(options))
}

// Vector is a named dense vector; distance is cosine (the default), dot,
// euclid or manhattan.
func (s *Schema) Vector(name string, dim int, distance ...string) *Schema {
	d := first(distance)
	if d == "" {
		d = "cosine"
	}
	s.vectors = append(s.vectors, JSONObject{"name": name, "dim": dim, "distance": d})
	return s
}

// SparseVector is a named sparse vector; modifier is "", "none" or "idf".
func (s *Schema) SparseVector(name string, modifier ...string) *Schema {
	entry := JSONObject{"name": name}
	if m := first(modifier); m != "" {
		entry["modifier"] = m
	}
	s.sparse = append(s.sparse, entry)
	return s
}

// Dynamic is what a write does with unmapped paths: ignore, strict or map.
func (s *Schema) Dynamic(mode string) *Schema {
	s.dynamic = mode
	return s
}

// MaxFields is the most fields, vectors and sparse vectors together.
func (s *Schema) MaxFields(n int) *Schema {
	s.maxFields = n
	return s
}

// JSON is the schema as JSON.
func (s *Schema) JSON() JSONObject {
	list := func(items []any) []any { return append([]any{}, items...) }
	return JSONObject{
		"fields":         list(s.fields),
		"vectors":        list(s.vectors),
		"sparse_vectors": list(s.sparse),
		"dynamic":        s.dynamic,
		"max_fields":     s.maxFields,
	}
}

// Struct is the schema as `CreateCollectionRequest.schema` takes it.
func (s *Schema) Struct() (*structpb.Struct, error) {
	out := &structpb.Struct{}
	if err := decodeJSON(s.JSON(), out); err != nil {
		return nil, invalidBuild("schema", err)
	}
	return out, nil
}

// ---------------------------------------------------------------------------
// Fusion and sort
// ---------------------------------------------------------------------------

// RRF is reciprocal-rank fusion; with no k, the engine's 60.
func RRF(k ...uint32) JSONObject {
	if len(k) == 0 {
		return JSONObject{"rrf": JSONObject{}}
	}
	return JSONObject{"rrf": JSONObject{"k": k[0]}}
}

// DBSF is distance-based score fusion.
func DBSF() JSONObject { return JSONObject{"dbsf": JSONObject{}} }

// Weighted is a weighted sum, one weight per retriever in order.
func Weighted(weights ...float32) JSONObject {
	return JSONObject{"weightedSum": JSONObject{"weights": weights}}
}

// sortBuilders is the type of Sort.
type sortBuilders struct{}

// Sort is the sort keys. "asc" or "desc"; empty is the key's own default.
var Sort sortBuilders

func sortOrder(order string) string {
	if order == "asc" {
		return "SORT_ORDER_ASC"
	}
	return "SORT_ORDER_DESC"
}

// Score sorts by score; descending unless order says otherwise.
func (sortBuilders) Score(order ...string) JSONObject {
	if o := first(order); o != "" {
		return JSONObject{"score": JSONObject{"order": sortOrder(o)}}
	}
	return JSONObject{"score": JSONObject{}}
}

// PK sorts by primary key; ascending unless order says otherwise.
func (sortBuilders) PK(order ...string) JSONObject {
	if o := first(order); o != "" {
		return JSONObject{"pk": JSONObject{"order": sortOrder(o)}}
	}
	return JSONObject{"pk": JSONObject{}}
}

// Field sorts by a fast field; missing is "first", "last" or "".
func (sortBuilders) Field(name, order, missing string) JSONObject {
	key := JSONObject{"field": name}
	if order != "" {
		key["order"] = sortOrder(order)
	}
	switch missing {
	case "first":
		key["missing"] = "MISSING_ORDER_FIRST"
	case "last":
		key["missing"] = "MISSING_ORDER_LAST"
	}
	return JSONObject{"field": key}
}

// ---------------------------------------------------------------------------
// Search
// ---------------------------------------------------------------------------

// TextOptions are a text retriever's options.
type TextOptions struct {
	// Fields: one is a match, several a multi_match, none a query_string.
	Fields []string
	K      uint32
}

// VectorOptions are a vector retriever's options and ANN knobs.
type VectorOptions struct {
	K            uint32
	Exact        bool
	Nprobes      uint32
	RefineFactor uint32
	Ef           uint32
	Oversampling float32
	Distance     string
	Filter       FilterJSON
}

// SparseOptions are a sparse retriever's options.
type SparseOptions struct {
	K      uint32
	Filter FilterJSON
}

// SelectOptions are what to return of each hit.
type SelectOptions struct {
	Include, Exclude []string
	NoSource         bool
	Vectors, Fields  []string
}

// HighlightOptions are a highlighted field's options.
type HighlightOptions struct {
	Pre, Post    string
	FragmentSize uint32
	Fragments    uint32
}

// ExpandOptions are a graph expansion's options (design §07 §5.2).
type ExpandOptions struct {
	FromField    string
	SeedLabel    string
	Seeds        uint32
	Hops         uint32
	Direction    string // out, in or both
	EdgeTypes    []string
	LimitPerSeed uint32
	Limit        uint32
	Output       string // nested or flat
}

// Search builds a `loams.collection.v1.SearchRequest`.
type Search struct {
	json JSONObject
}

// NewSearch is a search of collection (a name or an alias).
func NewSearch(collection string) Search {
	return Search{json: JSONObject{"collection": collection}}
}

// SearchFromJSON is a builder over a request's proto3 JSON.
func SearchFromJSON(json JSONObject) Search { return Search{json: deepCopy(json).(JSONObject)} }

func deepCopy(value any) any {
	switch v := value.(type) {
	case map[string]any:
		out := make(map[string]any, len(v))
		for key, item := range v {
			out[key] = deepCopy(item)
		}
		return out
	case []any:
		out := make([]any, len(v))
		for i, item := range v {
			out[i] = deepCopy(item)
		}
		return out
	default:
		return v
	}
}

func (s Search) with(key string, value any) Search {
	out := deepCopy(s.json).(JSONObject)
	out[key] = value
	return Search{json: out}
}

func (s Search) retrievers() []any {
	list, _ := s.json["retrievers"].([]any)
	return list
}

func (s Search) addRetriever(retriever JSONObject) Search {
	list := append(append([]any{}, s.retrievers()...), retriever)
	return s.with("retrievers", list)
}

// Namespace is the namespace, when it is not the client's default.
func (s Search) Namespace(namespace string) Search { return s.with("namespace", namespace) }

// Text is a text retriever over a string query.
func (s Search) Text(text string, options ...TextOptions) Search {
	o := first(options)
	var query FilterJSON
	switch len(o.Fields) {
	case 0:
		query = Filter.QueryString(text)
	case 1:
		query = Filter.Match(o.Fields[0], text)
	default:
		query = Filter.MultiMatch(o.Fields, text)
	}
	return s.TextQuery(query, o.K)
}

// TextQuery is a text retriever over a filter (`Filter.Match`, `Phrase`…).
func (s Search) TextQuery(query FilterJSON, k uint32) Search {
	retriever := JSONObject{"query": query}
	if k > 0 {
		retriever["k"] = k
	}
	return s.addRetriever(JSONObject{"text": retriever})
}

// Vector is a dense-vector kNN retriever on field.
func (s Search) Vector(field string, values []float32, options ...VectorOptions) Search {
	o := first(options)
	retriever := JSONObject{"field": field, "query": append([]float32(nil), values...)}
	if o.K > 0 {
		retriever["k"] = o.K
	}
	params := JSONObject{}
	if o.Exact {
		params["exact"] = true
	}
	if o.Nprobes > 0 {
		params["nprobes"] = o.Nprobes
	}
	if o.RefineFactor > 0 {
		params["refineFactor"] = o.RefineFactor
	}
	if o.Ef > 0 {
		params["ef"] = o.Ef
	}
	if o.Oversampling != 0 {
		params["oversampling"] = o.Oversampling
	}
	if o.Distance != "" {
		params["distance"] = o.Distance
	}
	if len(params) > 0 {
		retriever["params"] = params
	}
	if o.Filter != nil {
		retriever["filter"] = o.Filter
	}
	return s.addRetriever(JSONObject{"vector": retriever})
}

// Sparse is a sparse-vector retriever.
func (s Search) Sparse(field string, indices []uint32, values []float32, options ...SparseOptions) Search {
	o := first(options)
	retriever := JSONObject{"field": field, "query": JSONObject{"indices": indices, "values": values}}
	if o.K > 0 {
		retriever["k"] = o.K
	}
	if o.Filter != nil {
		retriever["filter"] = o.Filter
	}
	return s.addRetriever(JSONObject{"sparse": retriever})
}

// Fuse is how the ranked lists combine: RRF, DBSF or Weighted.
func (s Search) Fuse(fusion JSONObject) Search { return s.with("fusion", fusion) }

// Filter is the filter every candidate must match.
func (s Search) Filter(filter FilterJSON) Search { return s.with("filter", filter) }

// Sort is the sort, replacing any earlier one.
func (s Search) Sort(keys ...JSONObject) Search {
	list := make([]any, len(keys))
	for i, key := range keys {
		list[i] = key
	}
	return s.with("sort", list)
}

// Limit is the window size.
func (s Search) Limit(limit uint32) Search { return s.with("limit", limit) }

// Offset is how many hits to skip.
func (s Search) Offset(offset uint32) Search { return s.with("offset", offset) }

// After is the page after the hit whose sort values these are: JSON values,
// or the `*structpb.Value`s a hit's `SortValues` holds.
func (s Search) After(sortValues ...any) Search {
	list := make([]any, len(sortValues))
	for i, value := range sortValues {
		if v, ok := value.(*structpb.Value); ok {
			list[i] = v.AsInterface()
		} else {
			list[i] = value
		}
	}
	return s.with("searchAfter", list)
}

// MinScore drops hits scoring below score.
func (s Search) MinScore(score float32) Search { return s.with("scoreThreshold", score) }

// Select is what to return of each hit.
func (s Search) Select(options SelectOptions) Search {
	selection := JSONObject{}
	switch {
	case options.NoSource:
		selection["source"] = "none"
	case options.Include != nil || options.Exclude != nil:
		source := JSONObject{}
		if options.Include != nil {
			source["include"] = options.Include
		}
		if options.Exclude != nil {
			source["exclude"] = options.Exclude
		}
		selection["source"] = source
	}
	if options.Vectors != nil {
		selection["vectors"] = options.Vectors
	}
	if options.Fields != nil {
		selection["fields"] = options.Fields
	}
	return s.with("select", selection)
}

// Highlight highlights field in each hit; call once per field.
func (s Search) Highlight(field string, options ...HighlightOptions) Search {
	o := first(options)
	entry := JSONObject{"field": field}
	if o.Pre != "" {
		entry["preTag"] = o.Pre
	}
	if o.Post != "" {
		entry["postTag"] = o.Post
	}
	if o.FragmentSize > 0 {
		entry["fragmentSize"] = o.FragmentSize
	}
	if o.Fragments > 0 {
		entry["numberOfFragments"] = o.Fragments
	}
	var existing []any
	if highlight, ok := s.json["highlight"].(JSONObject); ok {
		existing, _ = highlight["fields"].([]any)
	}
	return s.with("highlight", JSONObject{"fields": append(append([]any{}, existing...), entry)})
}

// GroupBy groups the hits by field: size hits per group, limit groups.
func (s Search) GroupBy(field string, size, limit uint32) Search {
	group := JSONObject{"field": field}
	if size > 0 {
		group["groupSize"] = size
	}
	if limit > 0 {
		group["limit"] = limit
	}
	return s.with("groupBy", group)
}

// Expand walks graph from the top hits and returns their neighbours.
func (s Search) Expand(graph string, options ...ExpandOptions) Search {
	o := first(options)
	spec := JSONObject{"graph": graph}
	if o.FromField != "" {
		spec["fromField"] = o.FromField
	}
	if o.SeedLabel != "" {
		spec["seedLabel"] = o.SeedLabel
	}
	if o.Seeds > 0 {
		spec["seeds"] = o.Seeds
	}
	if o.Hops > 0 {
		spec["hops"] = o.Hops
	}
	if o.Direction != "" {
		spec["direction"] = "EXPAND_DIRECTION_" + strings.ToUpper(o.Direction)
	}
	if len(o.EdgeTypes) > 0 {
		spec["edgeTypes"] = o.EdgeTypes
	}
	if o.LimitPerSeed > 0 {
		spec["limitPerSeed"] = o.LimitPerSeed
	}
	if o.Limit > 0 {
		spec["limit"] = o.Limit
	}
	if o.Output != "" {
		spec["output"] = "EXPAND_OUTPUT_" + strings.ToUpper(o.Output)
	}
	return s.with("expandSpec", spec)
}

// Consistency is how fresh the read must be: "strong", "eventual", or a token
// (`at_least`).
func (s Search) Consistency(level string) Search {
	switch level {
	case "strong":
		return s.with("consistency", JSONObject{"freshness": "FRESHNESS_STRONG"})
	case "eventual":
		return s.with("consistency", JSONObject{"freshness": "FRESHNESS_EVENTUAL"})
	default:
		return s.with("consistency", JSONObject{"atLeast": level})
	}
}

// Collection is the collection searched.
func (s Search) Collection() string {
	collection, _ := s.json["collection"].(string)
	return collection
}

// HasTiebreak reports whether the sort ends in a primary-key tiebreaker, which
// QueryAll needs to page without skipping or repeating a hit.
func (s Search) HasTiebreak() bool {
	list, _ := s.json["sort"].([]any)
	if len(list) == 0 {
		return false
	}
	last, ok := list[len(list)-1].(JSONObject)
	if !ok {
		return false
	}
	_, ok = last["pk"]
	return ok
}

// JSON is the request's proto3 JSON.
func (s Search) JSON() JSONObject { return deepCopy(s.json).(JSONObject) }

// Build is the request as the generated message, checked: a consistency token
// must parse, and a weighted fusion needs one weight per retriever.
func (s Search) Build() (*collectionv1.SearchRequest, error) {
	if fusion, ok := s.json["fusion"].(JSONObject); ok {
		if sum, ok := fusion["weightedSum"].(JSONObject); ok {
			weights, _ := sum["weights"].([]float32)
			if retrievers := len(s.retrievers()); len(weights) != retrievers {
				return nil, invalidBuild("fusion.weighted_sum.weights", fmt.Errorf(
					"a weighted fusion needs one weight per retriever: %d weights for %d retrievers",
					len(weights), retrievers))
			}
		}
	}
	if consistency, ok := s.json["consistency"].(JSONObject); ok {
		if token, ok := consistency["atLeast"].(string); ok {
			if _, err := ParseConsistencyToken(token); err != nil {
				return nil, err
			}
		}
	}
	request := &collectionv1.SearchRequest{}
	if err := decodeJSON(s.json, request); err != nil {
		return nil, invalidBuild("search", errors.Join(errors.New("the search does not decode"), err))
	}
	return request, nil
}
