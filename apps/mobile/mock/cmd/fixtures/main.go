// Command fixtures writes conformance/fixtures/push/sealed.json: a
// notification sealed by the mock's HPKE code to a fixed test key, which the
// Android (Tink) and iOS (CryptoKit) unsealers must open.
//
// HPKE encapsulation is randomized, so `sealed` (and only `sealed`) changes on
// every run; the key, ids and plaintext are fixed. Commit a regenerated file
// only when the plaintext or the scheme changes. Run from mock/:
//
//	go run ./cmd/fixtures
package main

import (
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"log"
	"os"
	"time"

	"github.com/cloudflare/circl/hpke"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"

	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	"github.com/ostrium-labs/loams-mobile/mock/internal/seal"
)

func main() {
	// A fixed, test-only X25519 key derived from a label.
	seed := sha256.Sum256([]byte("loams-mobile push fixture key (test only)"))
	pk, sk := hpke.KEM_X25519_HKDF_SHA256.Scheme().DeriveKeyPair(seed[:])
	pub, _ := pk.MarshalBinary()
	priv, _ := sk.MarshalBinary()

	const instanceID, notificationID = "01J9Z3MOCK0000000000000000", "ntf_fixture_0001"
	n := &notificationsv1.Notification{
		Id: notificationID, Category: notificationsv1.Category_CATEGORY_APPROVALS,
		CloudeventType: "io.loams.dev.approval.requested.v1", Subject: "apr_drop_logs",
		Title: "Approval needed", Body: "Bob Example wants to drop logs-2026 in production",
		Ref:           &notificationsv1.Notification_ApprovalId{ApprovalId: "apr_drop_logs"},
		EnvironmentId: "env_prod", CreatedAt: timestamppb.New(time.Unix(1790899200, 0)),
	}
	plain, err := proto.MarshalOptions{Deterministic: true}.Marshal(n)
	if err != nil {
		log.Fatal(err)
	}
	sealed, err := seal.Seal(pub, instanceID, notificationID, plain)
	if err != nil {
		log.Fatal(err)
	}
	b64 := base64.StdEncoding.EncodeToString
	out := map[string]any{
		"about": "HPKE base mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305 (design §37 §7.4). " +
			"info = \"loams-push-v1\" 0x00 instance_id 0x00 notification_id, empty AAD; sealed = enc (32 bytes) || ciphertext. " +
			"The plaintext is a serialized loams.notifications.v1.Notification. Keys are test-only. Regenerate with `go run ./cmd/fixtures` in mock/; `sealed` differs on every run because HPKE encapsulation is randomized.",
		"recipient_private_key": b64(priv),
		"recipient_public_key":  b64(pub),
		"instance_id":           instanceID,
		"notification_id":       notificationID,
		"info":                  b64(seal.Info(instanceID, notificationID)),
		"plaintext":             b64(plain),
		"sealed":                b64(sealed),
		"message":               map[string]string{"v": "1", "i": instanceID, "n": notificationID, "s": b64(sealed)},
		"expect":                map[string]string{"title": n.Title, "body": n.Body, "approval_id": "apr_drop_logs"},
	}
	f, err := os.Create("../conformance/fixtures/push/sealed.json")
	if err != nil {
		log.Fatal(err)
	}
	enc := json.NewEncoder(f)
	enc.SetIndent("", "  ")
	if err := enc.Encode(out); err != nil {
		log.Fatal(err)
	}
	_ = f.Close()
}
