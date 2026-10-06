// Package jose holds the few JOSE pieces the mock needs: base64url, RFC 7638
// thumbprints, and ES256 compact JWS verification.
package jose

import (
	"crypto/ecdsa"
	"crypto/elliptic"
	"crypto/sha256"
	"encoding/base64"
	"encoding/json"
	"errors"
	"fmt"
	"math/big"
	"strings"
)

// B64 is unpadded base64url (RFC 7515 §2).
var B64 = base64.RawURLEncoding

// JWK is the subset of RFC 7517 the apps send and the mock serves.
type JWK struct {
	Kty string `json:"kty"`
	Crv string `json:"crv,omitempty"`
	X   string `json:"x,omitempty"`
	Y   string `json:"y,omitempty"`
	Kid string `json:"kid,omitempty"`
	Alg string `json:"alg,omitempty"`
	Use string `json:"use,omitempty"`
}

// Thumbprint is the RFC 7638 SHA-256 thumbprint, base64url.
func Thumbprint(k JWK) (string, error) {
	var members string
	switch k.Kty {
	case "EC":
		members = fmt.Sprintf(`{"crv":%q,"kty":"EC","x":%q,"y":%q}`, k.Crv, k.X, k.Y)
	case "OKP":
		members = fmt.Sprintf(`{"crv":%q,"kty":"OKP","x":%q}`, k.Crv, k.X)
	default:
		return "", fmt.Errorf("unsupported kty %q", k.Kty)
	}
	sum := sha256.Sum256([]byte(members))
	return B64.EncodeToString(sum[:]), nil
}

// P256 turns an EC P-256 JWK into a public key.
func P256(k JWK) (*ecdsa.PublicKey, error) {
	if k.Kty != "EC" || k.Crv != "P-256" {
		return nil, errors.New("decision key must be an EC P-256 JWK")
	}
	x, err := B64.DecodeString(k.X)
	if err != nil || len(x) != 32 {
		return nil, errors.New("bad x")
	}
	y, err := B64.DecodeString(k.Y)
	if err != nil || len(y) != 32 {
		return nil, errors.New("bad y")
	}
	pub := &ecdsa.PublicKey{Curve: elliptic.P256(), X: new(big.Int).SetBytes(x), Y: new(big.Int).SetBytes(y)}
	if !pub.Curve.IsOnCurve(pub.X, pub.Y) { //nolint:staticcheck // fine for a test mock
		return nil, errors.New("point not on curve")
	}
	return pub, nil
}

// VerifyES256 checks a compact JWS signed with ES256 (r||s, 64 bytes) and
// returns its header and payload bytes.
func VerifyES256(jws string, pub *ecdsa.PublicKey) (header map[string]any, payload []byte, err error) {
	parts := strings.Split(jws, ".")
	if len(parts) != 3 {
		return nil, nil, errors.New("not a compact JWS")
	}
	hb, err := B64.DecodeString(parts[0])
	if err != nil {
		return nil, nil, errors.New("bad header encoding")
	}
	if err := json.Unmarshal(hb, &header); err != nil {
		return nil, nil, errors.New("bad header json")
	}
	if header["alg"] != "ES256" {
		return nil, nil, fmt.Errorf("alg %v, want ES256", header["alg"])
	}
	payload, err = B64.DecodeString(parts[1])
	if err != nil {
		return nil, nil, errors.New("bad payload encoding")
	}
	sig, err := B64.DecodeString(parts[2])
	if err != nil || len(sig) != 64 {
		return nil, nil, errors.New("signature must be 64 bytes r||s")
	}
	digest := sha256.Sum256([]byte(parts[0] + "." + parts[1]))
	r := new(big.Int).SetBytes(sig[:32])
	s := new(big.Int).SetBytes(sig[32:])
	if !ecdsa.Verify(pub, digest[:], r, s) {
		return nil, nil, errors.New("signature does not verify")
	}
	return header, payload, nil
}
