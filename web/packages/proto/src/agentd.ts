// `loams.agentd.v1`: loams-agentd's Connect surface for Loams Studio (design
// §53 §6.1, SD1 Task 4). The messages and service descriptors of every file
// of the package, from one import path (`@loams/proto/agentd`). A page reads
// them with connect-query over the agentd transport (SD1 Task 6), never over
// the Loams server's.
//
// The package is unstable until SD1 Task 32 and is not in any SDK (Q816).

export * from './gen/loams/agentd/v1/approvals_pb.js';
export * from './gen/loams/agentd/v1/attachments_pb.js';
export * from './gen/loams/agentd/v1/diff_pb.js';
export * from './gen/loams/agentd/v1/factory_pb.js';
export * from './gen/loams/agentd/v1/files_pb.js';
export * from './gen/loams/agentd/v1/git_pb.js';
export * from './gen/loams/agentd/v1/groups_pb.js';
export * from './gen/loams/agentd/v1/harness_pb.js';
export * from './gen/loams/agentd/v1/marketplace_pb.js';
export * from './gen/loams/agentd/v1/notifications_pb.js';
export * from './gen/loams/agentd/v1/preview_pb.js';
export * from './gen/loams/agentd/v1/projects_pb.js';
export * from './gen/loams/agentd/v1/queue_pb.js';
export * from './gen/loams/agentd/v1/review_pb.js';
export * from './gen/loams/agentd/v1/studio_pb.js';
export * from './gen/loams/agentd/v1/terminals_pb.js';
export * from './gen/loams/agentd/v1/threads_pb.js';
