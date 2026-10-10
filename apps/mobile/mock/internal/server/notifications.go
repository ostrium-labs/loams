package server

import (
	"context"
	"slices"

	"connectrpc.com/connect"
	"google.golang.org/protobuf/proto"
	"google.golang.org/protobuf/types/known/timestamppb"

	notificationsv1 "github.com/ostrium-labs/loams-mobile/mock/gen/loams/notifications/v1"
)

type notificationService struct{ s *Server }

func cloneN(n *notificationsv1.Notification) *notificationsv1.Notification {
	return proto.Clone(n).(*notificationsv1.Notification)
}

func (n notificationService) ListNotifications(_ context.Context, req *connect.Request[notificationsv1.ListNotificationsRequest]) (*connect.Response[notificationsv1.ListNotificationsResponse], error) {
	out := &notificationsv1.ListNotificationsResponse{}
	all := n.s.notifications.List()
	slices.Reverse(all) // newest first
	for _, x := range all {
		if req.Msg.UnreadOnly && x.ReadAt != nil {
			continue
		}
		out.Notifications = append(out.Notifications, cloneN(x))
	}
	return connect.NewResponse(out), nil
}

func (n notificationService) WatchNotifications(ctx context.Context, req *connect.Request[notificationsv1.WatchNotificationsRequest], stream *connect.ServerStream[notificationsv1.WatchNotificationsResponse]) error {
	return runWatch(ctx, n.s, n.s.notifications, req.Msg.ResumeCursor, sink[*notificationsv1.Notification]{
		id:    func(x *notificationsv1.Notification) string { return x.Id },
		match: func(*notificationsv1.Notification) bool { return true },
		snapshot: func(items []*notificationsv1.Notification, cur string, reset bool) error {
			return stream.Send(&notificationsv1.WatchNotificationsResponse{Event: &notificationsv1.WatchNotificationsResponse_Snapshot{Snapshot: &notificationsv1.NotificationSnapshot{Notifications: items}}, Cursor: cur, SnapshotReset: reset})
		},
		upsert: func(x *notificationsv1.Notification, cur string) error {
			return stream.Send(&notificationsv1.WatchNotificationsResponse{Event: &notificationsv1.WatchNotificationsResponse_Upsert{Upsert: x}, Cursor: cur})
		},
		remove: func(id, cur string) error {
			return stream.Send(&notificationsv1.WatchNotificationsResponse{Event: &notificationsv1.WatchNotificationsResponse_Remove{Remove: id}, Cursor: cur})
		},
		heartbeat: func(cur string) error {
			return stream.Send(&notificationsv1.WatchNotificationsResponse{Event: &notificationsv1.WatchNotificationsResponse_Heartbeat{Heartbeat: &notificationsv1.Heartbeat{}}, Cursor: cur})
		},
	})
}

func (n notificationService) MarkRead(_ context.Context, req *connect.Request[notificationsv1.MarkReadRequest]) (*connect.Response[notificationsv1.MarkReadResponse], error) {
	for _, id := range req.Msg.NotificationIds {
		_, _ = n.s.notifications.Update(id, func(cur *notificationsv1.Notification) (*notificationsv1.Notification, error) {
			if cur.ReadAt != nil {
				return cur, nil
			}
			next := cloneN(cur)
			next.ReadAt = timestamppb.New(n.s.now())
			return next, nil
		})
	}
	return connect.NewResponse(&notificationsv1.MarkReadResponse{}), nil
}
