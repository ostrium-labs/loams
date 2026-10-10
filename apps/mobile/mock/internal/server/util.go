package server

import (
	"connectrpc.com/connect"
	"crypto/rand"
	"encoding/base32"
	"errors"
	"fmt"
	"math/big"
	"strconv"
)

var (
	errDropped = errors.New("stream dropped by the mock; resume from your cursor")
	errLagging = errors.New("subscriber fell behind; resume from your cursor")
)

func itoa(n uint64) string { return strconv.FormatUint(n, 10) }

// randomCode is 128 random bits as 26 base32 characters (§37 §7.2.1).
func randomCode() string {
	b := make([]byte, 16)
	_, _ = rand.Read(b)
	return base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(b)
}

// randomDigits is an 8-digit user code.
func randomDigits() string {
	n, _ := rand.Int(rand.Reader, big.NewInt(100_000_000))
	return fmt.Sprintf("%08d", n.Int64())
}

func randomToken(prefix string) string {
	b := make([]byte, 12)
	_, _ = rand.Read(b)
	return prefix + base32.StdEncoding.WithPadding(base32.NoPadding).EncodeToString(b)
}

func asConnect(err error, target **connect.Error) bool { return errors.As(err, target) }
