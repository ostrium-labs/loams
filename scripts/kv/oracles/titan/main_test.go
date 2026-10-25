// ported-from: https://github.com/distributedio/titan
// revision: 97868f5d24e0c13621658fbcb712064fec59e8be
// path: tools/autotest/main_test.go
// licence: Apache-2.0
// Modified by Ostrium Labs for Loams: the harness side of KV1 plan KV0
// Task 10. Instead of starting an in-process Titan on mocktikv (with an
// embedded etcd), TestMain dials the server named by
// LOAMS_TITAN_AUTOTEST_ADDR, so Titan's autotest can run unmodified
// against another server. Copied over Titan's file at run time by
// scripts/kv/oracles/titan-autotest.sh; never part of a Loams build.

package autotest

import (
	"fmt"
	"os"
	"testing"
)

var (
	at *AutoClient
	an *Abnormal
)

func TestMain(m *testing.M) {
	addr := os.Getenv("LOAMS_TITAN_AUTOTEST_ADDR")
	if addr == "" {
		fmt.Fprintln(os.Stderr, "LOAMS_TITAN_AUTOTEST_ADDR is not set")
		os.Exit(2)
	}
	at = NewAutoClient()
	an = NewAbnormal()
	at.Start(addr)
	an.Start(addr)
	v := m.Run()
	an.Close()
	at.Close()
	os.Exit(v)
}
