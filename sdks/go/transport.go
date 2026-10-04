// Transports (design §44 §4, D612; runtime contract R10).
//
// One port serves the Connect protocol, gRPC and gRPC-Web (D600), so the only
// question a Go client has is which of those it would rather speak, and Go has
// one honest answer for each:
//
//   - **HTTP/1.1 with connect-go's default `http.Client`.** Connect's server
//     streaming is a plain chunked HTTP response, so it works on HTTP/1.1, and
//     so does everything a TLS terminator, an ingress or a corporate proxy in
//     front of the instance understands. This is the default, because it is the
//     one that works everywhere.
//   - **HTTP/2 with `golang.org/x/net/http2`.** gRPC and the Connect protocol
//     both ride it, and it is what an in-cluster caller wants: one connection,
//     multiplexing, and no per-call handshake. It goes through a real TLS ALPN
//     or h2c, so it needs a listener that speaks it.
//
// R10 ("the browser is a first-class target") has no Go analogue and is not
// restated here: Go is not shipped to a browser, so there is no default entry
// point that must avoid a Node built-in, and no bundler to keep an HTTP/2
// transport away from. The clause's substance — one port, several protocols,
// one client — holds, and the split above is the Go way of saying it.

package loams

import (
	"context"
	"crypto/tls"
	"net"
	"net/http"

	connect "connectrpc.com/connect"
	"golang.org/x/net/http2"
)

// TransportConfig is how a caller configures the transport.
type TransportConfig struct {
	// Endpoint is the instance's base URL, for example
	// `https://acme.loams.dev`. A loopback stack is `http://127.0.0.1:8080`.
	Endpoint string
	// UseHTTP2 asks for the HTTP/2 transport, for an in-cluster caller.
	//
	// It is off by default. HTTP/1.1 works through every proxy an instance is
	// likely to sit behind, and Connect streaming does not need HTTP/2 — so
	// making the caller opt in is the right way round: the failure of guessing
	// wrong is a connection that hangs, and the fix is one field.
	UseHTTP2 bool
	// HTTPClient is the client the transport uses. It wins over everything
	// else, so a caller with its own TLS config, proxy or timeout supplies it.
	HTTPClient *http.Client
	// InsecureSkipVerify is refused in a release build. It is here for the
	// loopback stack with a self-signed certificate, which is a development
	// case and must be visible in a diff.
	InsecureSkipVerify bool
	// Protocol overrides the wire protocol. Unset means Connect, which is what
	// the design asks every language to default to (D612).
	Protocol Protocol
	// ExtraHeaders go on every request, for a gateway or a proxy.
	ExtraHeaders http.Header
	// ConnectOptions are passed through to connect-go: compression, codec,
	// interceptors. `loams.New` supplies its own auth interceptor only if the
	// caller supplies none.
	ConnectOptions []connect.ClientOption
}

// Protocol is the wire protocol a client speaks. All three are served on one
// port (D600).
type Protocol string

const (
	// ProtocolConnect is the Connect protocol. The default: it is the only one
	// that works identically over HTTP/1.1 and HTTP/2, and its unary form is
	// an HTTP POST with a JSON body, which is what `curl` sends (design §44
	// §4).
	ProtocolConnect Protocol = "connect"
	// ProtocolGRPC is gRPC over HTTP/2.
	ProtocolGRPC Protocol = "grpc"
	// ProtocolGRPCWeb is gRPC-Web. A Go client rarely wants it — a browser
	// cannot do gRPC and a Go program can — but it is served on the same port,
	// and a caller behind a proxy that only speaks gRPC-Web needs it.
	ProtocolGRPCWeb Protocol = "grpc-web"
)

// NewHTTPClient builds the `connect.HTTPClient` a transport uses.
func NewHTTPClient(config TransportConfig) (*http.Client, error) {
	if config.HTTPClient != nil {
		return config.HTTPClient, nil
	}
	if !config.UseHTTP2 {
		return &http.Client{Timeout: defaultClientTimeout}, nil
	}
	transport := &http2.Transport{}
	if isPlaintext(config.Endpoint) {
		// h2c: HTTP/2 with no TLS, which is what a loopback stack speaks. The
		// instance's plaintext listener means there is no ALPN to negotiate, so
		// the client asks for HTTP/2 with prior knowledge.
		transport.AllowHTTP = true
		transport.DialTLSContext = func(ctx context.Context, network, addr string, _ *tls.Config) (net.Conn, error) {
			var dialer net.Dialer
			return dialer.DialContext(ctx, network, addr)
		}
	}
	if config.InsecureSkipVerify {
		transport.TLSClientConfig = &tls.Config{InsecureSkipVerify: true} //nolint:gosec // loopback development only
	}
	return &http.Client{Transport: transport, Timeout: defaultClientTimeout}, nil
}

func isPlaintext(endpoint string) bool {
	return len(endpoint) >= 7 && endpoint[:7] == "http://"
}

// newTransport builds the connect-go client options for a call. It is a function
// rather than a struct field so the auth header is computed per call, from the
// caller's context, rather than once at construction.
func transportOptions(config TransportConfig) ([]connect.ClientOption, error) {
	if len(config.ConnectOptions) > 0 {
		return config.ConnectOptions, nil
	}
	// No request compression. connect-go's `WithSendGzip` compresses the **request**
	// body, and the fixture corpus records uncompressed ones — so a compressed
	// request is a request whose bytes the server has never seen, and the mismatch
	// is invisible until a test fails on it. Response compression is still on, which
	// is connect-go's default and costs the caller nothing.
	var options []connect.ClientOption
	switch config.Protocol {
	case "", ProtocolConnect:
		// connect-go's default: the Connect protocol, binary protobuf.
	case ProtocolGRPC:
		options = append(options, connect.WithGRPC())
	case ProtocolGRPCWeb:
		options = append(options, connect.WithGRPCWeb())
	default:
		return nil, newInternalError("", "loams: unknown protocol %q", config.Protocol)
	}
	return options, nil
}

// defaultClientTimeout is how long a client waits for a response body when the
// caller set no deadline on their context.
//
// It is zero, meaning no client-side timeout, and that is the deliberate
// choice: **the context is the deadline**. A default that is too short turns a
// slow query into a spurious `deadline_exceeded` that looks like a server fault,
// and the fix for a caller who wants a bound is `context.WithTimeout`, which
// covers the retry loop as well as the request.
const defaultClientTimeout = 0
