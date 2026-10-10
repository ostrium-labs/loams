// Package seal is the push payload sealing of §37 §7.4: HPKE (RFC 9180) base
// mode, DHKEM(X25519, HKDF-SHA256), HKDF-SHA256, ChaCha20-Poly1305.
//
// Binding: the instance id and notification id go into the HPKE `info`
// ("loams-push-v1" 0x00 instance_id 0x00 notification_id) and the AEAD's
// associated data is empty. §37 says "AAD = instance_id ‖ notification_id";
// info binds them just as firmly (a mismatch fails to open), and it is what
// Tink's public HybridDecrypt exposes on Android. Sealed bytes are
// enc (32 bytes) ‖ ciphertext, which is also Tink's NO_PREFIX output.
package seal

import (
	"crypto/rand"
	"errors"

	"github.com/cloudflare/circl/hpke"
)

var suite = hpke.NewSuite(hpke.KEM_X25519_HKDF_SHA256, hpke.KDF_HKDF_SHA256, hpke.AEAD_ChaCha20Poly1305)

// Info is the HPKE info for one notification.
func Info(instanceID, notificationID string) []byte {
	out := []byte("loams-push-v1")
	out = append(out, 0)
	out = append(out, instanceID...)
	out = append(out, 0)
	out = append(out, notificationID...)
	return out
}

// Seal encrypts plaintext to a 32-byte X25519 public key.
func Seal(recipientPublic []byte, instanceID, notificationID string, plaintext []byte) ([]byte, error) {
	pk, err := hpke.KEM_X25519_HKDF_SHA256.Scheme().UnmarshalBinaryPublicKey(recipientPublic)
	if err != nil {
		return nil, err
	}
	sender, err := suite.NewSender(pk, Info(instanceID, notificationID))
	if err != nil {
		return nil, err
	}
	enc, sealer, err := sender.Setup(rand.Reader)
	if err != nil {
		return nil, err
	}
	ct, err := sealer.Seal(plaintext, nil)
	if err != nil {
		return nil, err
	}
	return append(enc, ct...), nil
}

// Open decrypts what Seal produced, given the 32-byte X25519 private key.
func Open(recipientPrivate []byte, instanceID, notificationID string, sealed []byte) ([]byte, error) {
	if len(sealed) < 32+16 {
		return nil, errors.New("sealed payload too short")
	}
	sk, err := hpke.KEM_X25519_HKDF_SHA256.Scheme().UnmarshalBinaryPrivateKey(recipientPrivate)
	if err != nil {
		return nil, err
	}
	recv, err := suite.NewReceiver(sk, Info(instanceID, notificationID))
	if err != nil {
		return nil, err
	}
	opener, err := recv.Setup(sealed[:32])
	if err != nil {
		return nil, err
	}
	return opener.Open(sealed[32:], nil)
}

// NewKeyPair returns a fresh X25519 key pair as raw 32-byte keys.
func NewKeyPair() (public, private []byte, err error) {
	pk, sk, err := hpke.KEM_X25519_HKDF_SHA256.Scheme().GenerateKeyPair()
	if err != nil {
		return nil, nil, err
	}
	public, err = pk.MarshalBinary()
	if err != nil {
		return nil, nil, err
	}
	private, err = sk.MarshalBinary()
	return public, private, err
}
