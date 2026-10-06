package server

import (
	"context"
	"strings"

	"connectrpc.com/connect"

	instancev1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1"
	"github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1/instancev1connect"
)

// caller is who a request comes from.
type caller struct {
	user   *instancev1.Principal
	device *device // nil for a plain user token
}

type callerKey struct{}

func callerFrom(ctx context.Context) caller {
	c, _ := ctx.Value(callerKey{}).(caller)
	return c
}

// Fake tokens. A paired device gets "mock-access-<device id>" from the token
// endpoint; "mock-access-usr_alice" works without pairing (for curl).
func (s *Server) resolve(header string) (caller, error) {
	scheme, token, ok := strings.Cut(header, " ")
	if !ok || (!strings.EqualFold(scheme, "Bearer") && !strings.EqualFold(scheme, "DPoP")) {
		return caller{}, fail(connect.CodeUnauthenticated, "", "missing bearer token")
	}
	s.mu.Lock()
	defer s.mu.Unlock()
	if who, ok := s.access[token]; ok {
		if uid, isUser := strings.CutPrefix(who, "user:"); isUser {
			return caller{user: s.users[uid]}, nil
		}
		d := s.devices[who]
		if d == nil || d.proto.RevokedAt != nil {
			return caller{}, fail(connect.CodeUnauthenticated, "device_revoked", "this device was revoked")
		}
		return caller{user: s.users[d.userID], device: d}, nil
	}
	if uid, ok := strings.CutPrefix(token, "mock-access-"); ok {
		if u := s.users[uid]; u != nil {
			return caller{user: u}, nil
		}
		if d := s.devices[uid]; d != nil && d.proto.RevokedAt != nil {
			return caller{}, fail(connect.CodeUnauthenticated, "device_revoked", "this device was revoked")
		}
	}
	return caller{}, fail(connect.CodeUnauthenticated, "", "unknown token")
}

type authInterceptor struct{ s *Server }

func (s *Server) authInterceptor() connect.Interceptor { return authInterceptor{s} }

func open(procedure string) bool {
	return procedure == instancev1connect.InstanceServiceGetInstanceProcedure
}

func (a authInterceptor) WrapUnary(next connect.UnaryFunc) connect.UnaryFunc {
	return func(ctx context.Context, req connect.AnyRequest) (connect.AnyResponse, error) {
		if open(req.Spec().Procedure) {
			return next(ctx, req)
		}
		c, err := a.s.resolve(req.Header().Get("Authorization"))
		if err != nil {
			return nil, err
		}
		return next(context.WithValue(ctx, callerKey{}, c), req)
	}
}

func (a authInterceptor) WrapStreamingClient(next connect.StreamingClientFunc) connect.StreamingClientFunc {
	return next
}

func (a authInterceptor) WrapStreamingHandler(next connect.StreamingHandlerFunc) connect.StreamingHandlerFunc {
	return func(ctx context.Context, conn connect.StreamingHandlerConn) error {
		c, err := a.s.resolve(conn.RequestHeader().Get("Authorization"))
		if err != nil {
			return err
		}
		return next(context.WithValue(ctx, callerKey{}, c), conn)
	}
}
