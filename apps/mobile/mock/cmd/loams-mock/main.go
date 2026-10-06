// Command loams-mock serves the Loams app protos for local app development.
//
//	go run ./cmd/loams-mock                     # 127.0.0.1:8084, demo data
//	go run ./cmd/loams-mock -listen 127.0.0.1:9000 -public-url http://10.0.2.2:9000
//
// See docs/RUNNING.md. Everything here is fake and test-only.
package main

import (
	"context"
	"encoding/json"
	"flag"
	"fmt"
	"log"
	"os"
	"os/signal"
	"time"

	"github.com/mdp/qrterminal/v3"

	"github.com/ostrium-labs/loams-mobile/mock/internal/server"
)

func main() {
	listen := flag.String("listen", "127.0.0.1:8084", "loopback address to listen on")
	public := flag.String("public-url", "http://10.0.2.2:8084", "the issuer the printed pairing QR names (as the phone reaches the mock)")
	heartbeat := flag.Duration("heartbeat", 15*time.Second, "Watch heartbeat interval")
	tick := flag.Duration("tick", 2*time.Second, "how often running operations advance (0 = never)")
	every := flag.Duration("approval-every", 0, "create a demo approval at this interval (0 = never)")
	seed := flag.Bool("seed", true, "load demo data")
	qr := flag.Bool("qr", true, "print the pairing QR code in the terminal")
	flag.Parse()

	logger := log.New(os.Stderr, "loams-mock ", log.LstdFlags)
	s := server.New(server.Config{PublicURL: *public, Heartbeat: *heartbeat, Tick: *tick, Seed: *seed, Log: logger})

	ctx, stop := signal.NotifyContext(context.Background(), os.Interrupt)
	defer stop()

	p, _ := s.NewPairing("usr_alice", *public)
	b, _ := json.Marshal(p)
	fmt.Printf("loams-mock on http://%s (phones: %s)\n\n", *listen, *public)
	fmt.Printf("Pairing payload (valid 5 minutes; GET /mock/pairing for a fresh one):\n%s\n\n", b)
	fmt.Printf("Typed pairing: issuer %s, code %s\n\n", *public, p.UserCode)
	if *qr {
		qrterminal.GenerateHalfBlock(string(b), qrterminal.L, os.Stdout)
	}
	fmt.Println("Controls: POST /mock/approvals (new approval + push), POST /mock/drop-streams, GET /mock/push-log")

	if *every > 0 {
		go func() {
			t := time.NewTicker(*every)
			defer t.Stop()
			for {
				select {
				case <-ctx.Done():
					return
				case <-t.C:
					a := s.NewDemoApproval()
					logger.Printf("new approval %s", a.Id)
				}
			}
		}()
	}
	if err := s.Run(ctx, *listen); err != nil {
		logger.Fatal(err)
	}
}
