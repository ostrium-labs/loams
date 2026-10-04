// Idempotency keys (design §44 §7.4, D610; runtime contract R3).
//
// A mutating call that carries an `idempotency_key` field is given one **per
// logical call**, before the first attempt, and **the same key goes out on
// every retry**. A key regenerated per attempt turns one write into two, which
// is the exact failure the key exists to prevent.
//
// The decision to key is read from the **generated schema** rather than from
// the object a caller happened to build. That matters for proto3 `optional`:
// `MutateRequest.idempotency_key` is optional, so a caller who leaves it out
// sends no key at all, the mutation is not retryable, and the SDK would never
// know — unless it asks the schema. And it must not confuse `MutateRequest`
// (which has the field) with `DeployRequest` (which does not), or it would
// invent a field the schema does not know.

package loams

import (
	"crypto/rand"
	"encoding/hex"
	"reflect"
	"time"

	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/reflect/protoreflect"
)

// IdempotencyKeyField is the generated Go field name of `idempotency_key`.
const IdempotencyKeyField = "IdempotencyKey"

// UUIDv7 returns a fresh UUIDv7 as the canonical lowercase hyphenated string.
//
// An idempotency key has to be unique across every client that has ever talked
// to an instance **and** sort by creation time, because a key that sorts is one
// an operator can correlate in a log. UUIDv4 is unique but unordered; ULIDs
// would do, but each of the thirteen SDKs would then need its own
// implementation, and Go has had `uuid.NewV7` in no standard library yet — so
// this is thirty lines rather than a dependency.
//
// Layout: 48 bits of Unix milliseconds, 4 bits of version (7), 12 bits of
// counter within the millisecond, 2 bits of variant, 62 random bits.
func UUIDv7() string {
	var bytes [16]byte
	if _, err := rand.Read(bytes[:]); err != nil {
		// crypto/rand does not fail on any platform this SDK targets, and there
		// is nothing sensible to fall back to: a predictable idempotency key
		// is a duplicate key. A key is not worth a panic over a write that has
		// not been sent, so this is the one place that panics.
		panic("loams: crypto/rand failed: " + err.Error())
	}
	millis := uint64(time.Now().UnixMilli())
	for index := 0; index < 6; index++ {
		// The 48-bit timestamp is big-endian, so it is read out with shifts
		// rather than by dividing: dividing keeps the fractional bits of the
		// lower digits and truncates the carry, which puts the wrong byte in.
		bytes[index] = byte((millis >> ((5 - index) * 8)) & 0xff)
	}
	bytes[6] = (bytes[6] & 0x0f) | 0x70 // version 7
	bytes[8] = (bytes[8] & 0x3f) | 0x80 // variant 10
	out := make([]byte, 36)
	hex.Encode(out[0:8], bytes[0:4])
	out[8] = '-'
	hex.Encode(out[9:13], bytes[4:6])
	out[13] = '-'
	hex.Encode(out[14:18], bytes[6:8])
	out[18] = '-'
	hex.Encode(out[19:23], bytes[8:10])
	out[23] = '-'
	hex.Encode(out[24:36], bytes[10:16])
	return string(out)
}

// UUIDv7Time is the Unix milliseconds a UUIDv7 encodes, and false for anything
// else.
//
// The timestamp is the first **twelve** hex digits, not eight: 48 bits, and
// milliseconds since the epoch use 41 of them. Reading eight digits returns a
// number around 2^25, which is January 1970.
func UUIDv7Time(value string) (time.Time, bool) {
	if len(value) != 36 {
		return time.Time{}, false
	}
	// 48 bits of timestamp is **twelve** hex digits: the first eight characters of
	// the string plus the next group of four, with the two hyphens skipped. Reading
	// only the first eight returns a number around 2^25, which is January 1970;
	// reading sixteen silently includes the version and the counter.
	digits := value[0:8] + value[9:13]
	millis, ok := parseHex(digits)
	if !ok {
		return time.Time{}, false
	}
	if value[8] != '-' || value[13] != '-' || value[18] != '-' || value[23] != '-' {
		return time.Time{}, false
	}
	if value[14] != '7' {
		return time.Time{}, false
	}
	switch value[19] {
	case '8', '9', 'a', 'b':
	default:
		return time.Time{}, false
	}
	return time.UnixMilli(int64(millis)).UTC(), true
}

func parseHex(value string) (uint64, bool) {
	var out uint64
	for _, character := range []byte(value) {
		var digit uint64
		switch {
		case character >= '0' && character <= '9':
			digit = uint64(character - '0')
		case character >= 'a' && character <= 'f':
			digit = uint64(character-'a') + 10
		default:
			return 0, false
		}
		out = out<<4 | digit
	}
	return out, true
}

// KeyedRequest is a request the runtime has decided to key, and whether it made
// that decision. A caller that wrote their own key gets `Keyed` true and the
// request unchanged.
type KeyedRequest struct {
	// Request is the message to send: a copy with the key set, or the original
	// when nothing was set.
	Request any
	// Keyed is whether the request carries a key the retry policy may rely on.
	Keyed bool
}

// ApplyIdempotencyKey decides a mutating call's idempotency key, once per
// logical call.
//
// declared says whether the request's **schema** declares the field, which is
// what `facade.CallBinding.TakesIdempotencyKey` carries. A message without the
// field is left exactly as the caller wrote it.
//
// supplied is the caller's own key, if any: making a retry yours rather than
// the SDK's is sometimes the right call, because the key is what your storage
// dedupes on.
func ApplyIdempotencyKey(request any, supplied string, declared bool) KeyedRequest {
	if request == nil {
		return KeyedRequest{Request: request}
	}
	if !declared && !hasIdempotencyKeyField(request) {
		return KeyedRequest{Request: request}
	}
	if current, present := readString(request, IdempotencyKeyField); present && current != "" {
		return KeyedRequest{Request: request, Keyed: true}
	}
	cloned := cloneRequest(request)
	key := supplied
	if key == "" {
		key = UUIDv7()
	}
	if err := writeString(cloned, IdempotencyKeyField, key); err != nil {
		// The field is there but not writable — a read-only field on a caller-built
		// struct. Returning the request unkeyed is correct: the call is then not
		// retryable, which is the safe direction.
		return KeyedRequest{Request: request}
	}
	return KeyedRequest{Request: cloned, Keyed: true}
}

// hasIdempotencyKeyField whether a request declares the field at all.
//
// Two proofs are available and either is enough: the generated **schema**
// declaring it (`declared`, which is what `facade.CallBinding.TakesIdempotencyKey`
// carries), or the Go type having the field — which is what a caller who
// constructs its own struct, and a future language-specific builder, has instead
// of a descriptor. Refusing a request that demonstrably has the field would be a
// way to silently make a mutation un-retryable.
func hasIdempotencyKeyField(request any) bool {
	if message, ok := request.(proto.Message); ok {
		return DeclaresIdempotencyKey(message)
	}
	holder := reflect.ValueOf(request)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return false
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return false
	}
	_, present := holder.Type().FieldByName(IdempotencyKeyField)
	return present
}

// cloneRequest copies a request so the key can be set without mutating the
// caller's message. A proto message is cloned through `proto.Clone`, which
// copies the internal state a concurrent `Marshal` would race on; anything else
// is a shallow struct copy, which is right for the plain structs a test builds.
func cloneRequest(request any) any {
	if message, ok := request.(proto.Message); ok {
		return proto.Clone(message)
	}
	value := reflect.ValueOf(request)
	for value.Kind() == reflect.Pointer {
		if value.IsNil() {
			return request
		}
		value = value.Elem()
	}
	if !value.IsValid() || value.Kind() != reflect.Struct {
		return request
	}
	clone := reflect.New(value.Type())
	clone.Elem().Set(value)
	return clone.Interface()
}

// readString reads a string field, whether it is a `string` or the `*string` a
// proto3 `optional string` is generated as. `present` is false when the field
// is absent or nil, which is what proto3 `optional` means.
func readString(request any, field string) (value string, present bool) {
	holder := reflect.ValueOf(request)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return "", false
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return "", false
	}
	target := holder.FieldByName(field)
	if !target.IsValid() || !target.CanInterface() {
		return "", false
	}
	switch {
	case target.Kind() == reflect.String:
		return target.String(), true
	case target.Kind() == reflect.Pointer && target.Type().Elem().Kind() == reflect.String:
		if target.IsNil() {
			return "", false
		}
		return target.Elem().String(), true
	default:
		return "", false
	}
}

func writeString(request any, field, value string) error {
	holder := reflect.ValueOf(request)
	for holder.Kind() == reflect.Pointer {
		if holder.IsNil() {
			return newInternalError("", "cannot set %s on a nil request", field)
		}
		holder = holder.Elem()
	}
	if !holder.IsValid() || holder.Kind() != reflect.Struct {
		return newInternalError("", "cannot set %s on a %s", field, holder.Kind())
	}
	target := holder.FieldByName(field)
	if !target.IsValid() || !target.CanSet() {
		return newInternalError("", "no settable field %s", field)
	}
	if target.Kind() == reflect.String {
		target.SetString(value)
		return nil
	}
	// A proto3 `optional string` is generated as `*string`, so setting it
	// means allocating. This is the shape `MutateRequest.IdempotencyKey` has.
	if target.Kind() == reflect.Pointer && target.Type().Elem().Kind() == reflect.String {
		boxed := value
		target.Set(reflect.ValueOf(&boxed))
		return nil
	}
	return newInternalError("", "field %s is a %s, not a string", field, target.Kind())
}

// DeclaresIdempotencyKey reports whether a proto message declares
// `idempotency_key`. It exists so a caller — and the drift test — can ask the
// schema rather than the object, which is the point of R3.
func DeclaresIdempotencyKey(message proto.Message) bool {
	descriptor := message.ProtoReflect().Descriptor()
	field := descriptor.Fields().ByName(protoreflect.Name("idempotency_key"))
	return field != nil && field.Kind() == protoreflect.StringKind
}
