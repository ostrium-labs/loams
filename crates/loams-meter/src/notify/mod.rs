//! Threshold and lifecycle events and the notifier (design §63 §15, D1129;
//! ME1 Task 14).
//!
//! - [`events`]: the counter owner's `io.loams.dev.meter.*` events, with
//!   deterministic ids, and the tracker that emits each threshold once.
//! - [`webhook`]: Standard Webhooks signing, URL rules, the HTTP transport.
//! - [`sealing`]: webhook secrets sealed at rest (age, as `pg-control`'s).
//! - [`notifier`]: delivery to webhooks (signed, retried, once per endpoint
//!   and event), email and the console; the `NotificationService` domain.

pub mod events;
pub mod notifier;
pub mod sealing;
pub mod webhook;

pub use events::{MeterEvent, ThresholdTracker};
pub use notifier::{Channel, Delivery, DeliveryStatus, Endpoint, Handled, Notifier, Rule};
