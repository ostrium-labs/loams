package server

import (
	"context"

	"connectrpc.com/connect"

	instancev1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/instance/v1"
)

type instanceService struct{ s *Server }

func (i instanceService) GetInstance(ctx context.Context, _ *connect.Request[instancev1.GetInstanceRequest]) (*connect.Response[instancev1.GetInstanceResponse], error) {
	issuer := issuerFrom(ctx, i.s.cfg.PublicURL)
	return connect.NewResponse(&instancev1.GetInstanceResponse{
		InstanceId:     InstanceID,
		Edition:        instancev1.Edition_EDITION_OSS,
		ServerVersion:  "0.0.0-mock",
		ApiVersions:    []string{"loams.instance.v1", "loams.devices.v1", "loams.approvals.v1", "loams.operations.v1", "loams.notifications.v1"},
		Features:       map[string]bool{"push": true, "mock": true},
		Issuer:         issuer,
		JwksUri:        issuer + "/.well-known/jwks.json",
		SignInMethods:  []instancev1.SignInMethod{instancev1.SignInMethod_SIGN_IN_METHOD_PAIRING, instancev1.SignInMethod_SIGN_IN_METHOD_BROWSER},
		Push:           &instancev1.PushConfig{GatewayUrl: issuer + "/mock/push", AppIds: map[string]string{"android": "dev.loams.app", "ios": "dev.loams.app"}},
		MinAppVersions: map[string]string{"android": "0.1.0", "ios": "0.1.0"},
		IdentityProvider: &instancev1.IdentityProvider{
			Issuer:          issuer + "/mock/authentik/application/o/loams/",
			AndroidClientId: "loams-android",
			IosClientId:     "loams-ios",
		},
	}), nil
}

func (i instanceService) WhoAmI(ctx context.Context, _ *connect.Request[instancev1.WhoAmIRequest]) (*connect.Response[instancev1.WhoAmIResponse], error) {
	c := callerFrom(ctx)
	resp := &instancev1.WhoAmIResponse{
		Principal:    c.user,
		Org:          &instancev1.Org{Id: "org_acme", Name: "Acme"},
		Environments: i.s.envs,
	}
	if c.device != nil {
		resp.Device = &instancev1.DeviceRef{Id: c.device.proto.Id, Name: c.device.proto.Name}
	}
	return connect.NewResponse(resp), nil
}
