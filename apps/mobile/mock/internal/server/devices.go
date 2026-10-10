package server

import (
	"context"
	"encoding/base64"
	"encoding/json"
	"fmt"
	"time"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"

	devicesv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/devices/v1"
	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
	"github.com/ostrium-labs/loams-mobile/mock/internal/seal"
)

// PairingTTL is how long a pairing code lives (§37 §7.2.1).
const PairingTTL = 5 * time.Minute

type deviceService struct{ s *Server }

// QRPayload is the v1 pairing payload of §37 §7.2.1.
type QRPayload struct {
	V          int      `json:"v"`
	Kind       string   `json:"kind"`
	Issuer     string   `json:"issuer"`
	InstanceID string   `json:"instance_id"`
	SPKI       []string `json:"spki"`
	JKT        string   `json:"jkt"`
	Code       string   `json:"code"`
	UserCode   string   `json:"user_code"`
	Exp        int64    `json:"exp"`
}

// NewPairing creates a pairing for a user and returns its QR payload.
func (s *Server) NewPairing(userID, issuer string) (QRPayload, time.Time) {
	exp := s.now().Add(PairingTTL)
	p := &pairing{code: randomCode(), userCode: randomDigits(), userID: userID, expiresAt: exp}
	s.mu.Lock()
	s.pairings[p.code] = p
	s.userCodes[p.userCode] = p.code
	s.mu.Unlock()
	return QRPayload{
		V: 1, Kind: "loams-pair", Issuer: issuer, InstanceID: InstanceID,
		SPKI: nil, // plain HTTP mock: no TLS pins. A TLS mock would list its SPKI hash here.
		JKT:  s.jkt, Code: p.code, UserCode: p.userCode, Exp: exp.Unix(),
	}, exp
}

func (d deviceService) CreatePairing(ctx context.Context, req *connect.Request[devicesv1.CreatePairingRequest]) (*connect.Response[devicesv1.CreatePairingResponse], error) {
	c := callerFrom(ctx)
	payload, exp := d.s.NewPairing(c.user.Id, issuerFrom(ctx, d.s.cfg.PublicURL))
	b, _ := json.Marshal(payload)
	return connect.NewResponse(&devicesv1.CreatePairingResponse{
		PairingId: "pair_" + payload.Code[:8], QrPayload: string(b), UserCode: payload.UserCode, ExpiresAt: timestamppb.New(exp),
	}), nil
}

func (d deviceService) ListDevices(ctx context.Context, _ *connect.Request[devicesv1.ListDevicesRequest]) (*connect.Response[devicesv1.ListDevicesResponse], error) {
	c := callerFrom(ctx)
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	out := &devicesv1.ListDevicesResponse{}
	for _, dev := range d.s.devices {
		if dev.userID == c.user.Id {
			out.Devices = append(out.Devices, proto.Clone(dev.proto).(*devicesv1.Device))
		}
	}
	return connect.NewResponse(out), nil
}

func (d deviceService) ownDevice(ctx context.Context, id string) (*device, error) {
	c := callerFrom(ctx)
	dev := d.s.devices[id]
	if dev == nil || dev.userID != c.user.Id {
		return nil, fail(connect.CodeNotFound, "", "no such device")
	}
	return dev, nil
}

func (d deviceService) RenameDevice(ctx context.Context, req *connect.Request[devicesv1.RenameDeviceRequest]) (*connect.Response[devicesv1.RenameDeviceResponse], error) {
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	dev, err := d.ownDevice(ctx, req.Msg.DeviceId)
	if err != nil {
		return nil, err
	}
	if req.Msg.Name == "" {
		return nil, fail(connect.CodeInvalidArgument, "", "name is required")
	}
	dev.proto.Name = req.Msg.Name
	return connect.NewResponse(&devicesv1.RenameDeviceResponse{Device: proto.Clone(dev.proto).(*devicesv1.Device)}), nil
}

func (d deviceService) RevokeDevice(ctx context.Context, req *connect.Request[devicesv1.RevokeDeviceRequest]) (*connect.Response[devicesv1.RevokeDeviceResponse], error) {
	d.s.mu.Lock()
	dev, err := d.ownDevice(ctx, req.Msg.DeviceId)
	if err != nil {
		d.s.mu.Unlock()
		return nil, err
	}
	d.s.revokeLocked(dev)
	d.s.mu.Unlock()
	d.s.DropStreams()
	return connect.NewResponse(&devicesv1.RevokeDeviceResponse{}), nil
}

// revokeLocked ends a device's tokens, push targets and decision key.
func (s *Server) revokeLocked(dev *device) {
	if dev.proto.RevokedAt == nil {
		dev.proto.RevokedAt = timestamppb.New(s.now())
	}
	dev.decisionKey = nil
	for tok, who := range s.access {
		if who == dev.proto.Id {
			delete(s.access, tok)
		}
	}
	for tok, who := range s.refresh {
		if who == dev.proto.Id {
			delete(s.refresh, tok)
		}
	}
	for id, t := range s.pushTargets {
		if t.deviceID == dev.proto.Id {
			delete(s.pushTargets, id)
		}
	}
	dev.proto.PushTargets = nil
}

func (d deviceService) RegisterPushTarget(ctx context.Context, req *connect.Request[devicesv1.RegisterPushTargetRequest]) (*connect.Response[devicesv1.RegisterPushTargetResponse], error) {
	c := callerFrom(ctx)
	if c.device == nil {
		return nil, fail(connect.CodeFailedPrecondition, "", "push targets belong to a paired device")
	}
	m := req.Msg
	if len(m.HpkePublicKey) != 32 {
		return nil, fail(connect.CodeInvalidArgument, "", "hpke_public_key must be a 32-byte X25519 key")
	}
	if m.TokenOrEndpoint == "" || m.Provider == devicesv1.PushProvider_PUSH_PROVIDER_UNSPECIFIED {
		return nil, fail(connect.CodeInvalidArgument, "", "provider and token_or_endpoint are required")
	}
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	if prev, ok := d.s.idem[m.IdempotencyKey].(*devicesv1.RegisterPushTargetResponse); ok && m.IdempotencyKey != "" {
		return connect.NewResponse(prev), nil
	}
	for id, t := range d.s.pushTargets {
		if t.deviceID == c.device.proto.Id && t.req.TokenOrEndpoint == m.TokenOrEndpoint {
			delete(d.s.pushTargets, id)
			dropRef(c.device.proto, id)
		}
	}
	d.s.nextID++
	id := fmt.Sprintf("pt_%04d", d.s.nextID)
	d.s.pushTargets[id] = &pushTarget{id: id, deviceID: c.device.proto.Id, req: proto.Clone(m).(*devicesv1.RegisterPushTargetRequest)}
	c.device.proto.PushTargets = append(c.device.proto.PushTargets, &devicesv1.PushTargetRef{Id: id, Provider: m.Provider})
	resp := &devicesv1.RegisterPushTargetResponse{PushTargetId: id}
	if m.IdempotencyKey != "" {
		d.s.idem[m.IdempotencyKey] = resp
	}
	d.s.cfg.Log.Printf("push target %s registered (%s)", id, m.Provider)
	return connect.NewResponse(resp), nil
}

func (d deviceService) UnregisterPushTarget(ctx context.Context, req *connect.Request[devicesv1.UnregisterPushTargetRequest]) (*connect.Response[devicesv1.UnregisterPushTargetResponse], error) {
	c := callerFrom(ctx)
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	t := d.s.pushTargets[req.Msg.PushTargetId]
	if t == nil || c.device == nil || t.deviceID != c.device.proto.Id {
		return nil, fail(connect.CodeNotFound, "push_target_unknown", "no such push target")
	}
	delete(d.s.pushTargets, t.id)
	dropRef(c.device.proto, t.id)
	return connect.NewResponse(&devicesv1.UnregisterPushTargetResponse{}), nil
}

// dropRef removes a push target from the device record ListDevices returns.
func dropRef(dev *devicesv1.Device, id string) {
	refs := dev.PushTargets[:0]
	for _, r := range dev.PushTargets {
		if r.Id != id {
			refs = append(refs, r)
		}
	}
	dev.PushTargets = refs
}

func (d deviceService) GetNotificationPreferences(context.Context, *connect.Request[devicesv1.GetNotificationPreferencesRequest]) (*connect.Response[devicesv1.GetNotificationPreferencesResponse], error) {
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	return connect.NewResponse(&devicesv1.GetNotificationPreferencesResponse{Preferences: proto.Clone(d.s.prefs).(*devicesv1.NotificationPreferences)}), nil
}

func (d deviceService) SetNotificationPreferences(_ context.Context, req *connect.Request[devicesv1.SetNotificationPreferencesRequest]) (*connect.Response[devicesv1.SetNotificationPreferencesResponse], error) {
	if req.Msg.Preferences == nil {
		return nil, fail(connect.CodeInvalidArgument, "", "preferences are required")
	}
	d.s.mu.Lock()
	defer d.s.mu.Unlock()
	d.s.prefs = proto.Clone(req.Msg.Preferences).(*devicesv1.NotificationPreferences)
	return connect.NewResponse(&devicesv1.SetNotificationPreferencesResponse{Preferences: proto.Clone(d.s.prefs).(*devicesv1.NotificationPreferences)}), nil
}

func (d deviceService) SendTestNotification(ctx context.Context, _ *connect.Request[devicesv1.SendTestNotificationRequest]) (*connect.Response[devicesv1.SendTestNotificationResponse], error) {
	n := d.s.Notify(&notificationsv1.Notification{
		Category: notificationsv1.Category_CATEGORY_SECURITY, CloudeventType: "io.loams.dev.device.test.v1",
		Title: "Test notification", Body: "Push from your Loams instance works.",
	})
	return connect.NewResponse(&devicesv1.SendTestNotificationResponse{NotificationId: n.Id}), nil
}

// Notify adds a notification to the inbox and pushes it, sealed, to every
// push target. The push log then holds only what a gateway would see.
func (s *Server) Notify(n *notificationsv1.Notification) *notificationsv1.Notification {
	if n.Id == "" {
		n.Id = s.newID("ntf")
	}
	if n.CreatedAt == nil {
		n.CreatedAt = timestamppb.New(s.now())
	}
	s.notifications.Put(n.Id, n)
	plain, _ := proto.Marshal(n)
	s.mu.Lock()
	defer s.mu.Unlock()
	for _, t := range s.pushTargets {
		sealed, err := seal.Seal(t.req.HpkePublicKey, InstanceID, n.Id, plain)
		if err != nil {
			s.cfg.Log.Printf("seal for %s: %v", t.id, err)
			continue
		}
		priority := "normal"
		if n.Category == notificationsv1.Category_CATEGORY_APPROVALS || n.Category == notificationsv1.Category_CATEGORY_SECURITY {
			priority = "high"
		}
		s.pushLog = append(s.pushLog, PushRecord{
			Seq: len(s.pushLog) + 1, TargetID: t.id, Provider: t.req.Provider.String(), Token: t.req.TokenOrEndpoint,
			AppID: t.req.AppId, CollapseID: n.Id, Priority: priority,
			Data: map[string]string{"v": "1", "i": InstanceID, "n": n.Id, "s": base64.StdEncoding.EncodeToString(sealed)},
		})
	}
	return n
}
