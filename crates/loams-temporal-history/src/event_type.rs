//! The event type each attribute variant implies. Exhaustive: a new
//! variant in a proto update fails to compile here until it is mapped.

use loams_temporal_proto::temporal::api::enums::v1::EventType;
use loams_temporal_proto::temporal::api::history::v1::history_event::Attributes;

/// The `event_type` an event with these attributes must carry.
pub fn event_type(attributes: &Attributes) -> EventType {
    match attributes {
        Attributes::WorkflowExecutionStartedEventAttributes(_) => {
            EventType::WorkflowExecutionStarted
        }
        Attributes::WorkflowExecutionCompletedEventAttributes(_) => {
            EventType::WorkflowExecutionCompleted
        }
        Attributes::WorkflowExecutionFailedEventAttributes(_) => EventType::WorkflowExecutionFailed,
        Attributes::WorkflowExecutionTimedOutEventAttributes(_) => {
            EventType::WorkflowExecutionTimedOut
        }
        Attributes::WorkflowTaskScheduledEventAttributes(_) => EventType::WorkflowTaskScheduled,
        Attributes::WorkflowTaskStartedEventAttributes(_) => EventType::WorkflowTaskStarted,
        Attributes::WorkflowTaskCompletedEventAttributes(_) => EventType::WorkflowTaskCompleted,
        Attributes::WorkflowTaskTimedOutEventAttributes(_) => EventType::WorkflowTaskTimedOut,
        Attributes::WorkflowTaskFailedEventAttributes(_) => EventType::WorkflowTaskFailed,
        Attributes::ActivityTaskScheduledEventAttributes(_) => EventType::ActivityTaskScheduled,
        Attributes::ActivityTaskStartedEventAttributes(_) => EventType::ActivityTaskStarted,
        Attributes::ActivityTaskCompletedEventAttributes(_) => EventType::ActivityTaskCompleted,
        Attributes::ActivityTaskFailedEventAttributes(_) => EventType::ActivityTaskFailed,
        Attributes::ActivityTaskTimedOutEventAttributes(_) => EventType::ActivityTaskTimedOut,
        Attributes::TimerStartedEventAttributes(_) => EventType::TimerStarted,
        Attributes::TimerFiredEventAttributes(_) => EventType::TimerFired,
        Attributes::ActivityTaskCancelRequestedEventAttributes(_) => {
            EventType::ActivityTaskCancelRequested
        }
        Attributes::ActivityTaskCanceledEventAttributes(_) => EventType::ActivityTaskCanceled,
        Attributes::TimerCanceledEventAttributes(_) => EventType::TimerCanceled,
        Attributes::MarkerRecordedEventAttributes(_) => EventType::MarkerRecorded,
        Attributes::WorkflowExecutionSignaledEventAttributes(_) => {
            EventType::WorkflowExecutionSignaled
        }
        Attributes::WorkflowExecutionTerminatedEventAttributes(_) => {
            EventType::WorkflowExecutionTerminated
        }
        Attributes::WorkflowExecutionCancelRequestedEventAttributes(_) => {
            EventType::WorkflowExecutionCancelRequested
        }
        Attributes::WorkflowExecutionCanceledEventAttributes(_) => {
            EventType::WorkflowExecutionCanceled
        }
        Attributes::RequestCancelExternalWorkflowExecutionInitiatedEventAttributes(_) => {
            EventType::RequestCancelExternalWorkflowExecutionInitiated
        }
        Attributes::RequestCancelExternalWorkflowExecutionFailedEventAttributes(_) => {
            EventType::RequestCancelExternalWorkflowExecutionFailed
        }
        Attributes::ExternalWorkflowExecutionCancelRequestedEventAttributes(_) => {
            EventType::ExternalWorkflowExecutionCancelRequested
        }
        Attributes::WorkflowExecutionContinuedAsNewEventAttributes(_) => {
            EventType::WorkflowExecutionContinuedAsNew
        }
        Attributes::StartChildWorkflowExecutionInitiatedEventAttributes(_) => {
            EventType::StartChildWorkflowExecutionInitiated
        }
        Attributes::StartChildWorkflowExecutionFailedEventAttributes(_) => {
            EventType::StartChildWorkflowExecutionFailed
        }
        Attributes::ChildWorkflowExecutionStartedEventAttributes(_) => {
            EventType::ChildWorkflowExecutionStarted
        }
        Attributes::ChildWorkflowExecutionCompletedEventAttributes(_) => {
            EventType::ChildWorkflowExecutionCompleted
        }
        Attributes::ChildWorkflowExecutionFailedEventAttributes(_) => {
            EventType::ChildWorkflowExecutionFailed
        }
        Attributes::ChildWorkflowExecutionCanceledEventAttributes(_) => {
            EventType::ChildWorkflowExecutionCanceled
        }
        Attributes::ChildWorkflowExecutionTimedOutEventAttributes(_) => {
            EventType::ChildWorkflowExecutionTimedOut
        }
        Attributes::ChildWorkflowExecutionTerminatedEventAttributes(_) => {
            EventType::ChildWorkflowExecutionTerminated
        }
        Attributes::SignalExternalWorkflowExecutionInitiatedEventAttributes(_) => {
            EventType::SignalExternalWorkflowExecutionInitiated
        }
        Attributes::SignalExternalWorkflowExecutionFailedEventAttributes(_) => {
            EventType::SignalExternalWorkflowExecutionFailed
        }
        Attributes::ExternalWorkflowExecutionSignaledEventAttributes(_) => {
            EventType::ExternalWorkflowExecutionSignaled
        }
        Attributes::UpsertWorkflowSearchAttributesEventAttributes(_) => {
            EventType::UpsertWorkflowSearchAttributes
        }
        Attributes::WorkflowExecutionUpdateAcceptedEventAttributes(_) => {
            EventType::WorkflowExecutionUpdateAccepted
        }
        Attributes::WorkflowExecutionUpdateRejectedEventAttributes(_) => {
            EventType::WorkflowExecutionUpdateRejected
        }
        Attributes::WorkflowExecutionUpdateCompletedEventAttributes(_) => {
            EventType::WorkflowExecutionUpdateCompleted
        }
        Attributes::WorkflowPropertiesModifiedExternallyEventAttributes(_) => {
            EventType::WorkflowPropertiesModifiedExternally
        }
        Attributes::ActivityPropertiesModifiedExternallyEventAttributes(_) => {
            EventType::ActivityPropertiesModifiedExternally
        }
        Attributes::WorkflowPropertiesModifiedEventAttributes(_) => {
            EventType::WorkflowPropertiesModified
        }
        Attributes::WorkflowExecutionUpdateAdmittedEventAttributes(_) => {
            EventType::WorkflowExecutionUpdateAdmitted
        }
        Attributes::NexusOperationScheduledEventAttributes(_) => EventType::NexusOperationScheduled,
        Attributes::NexusOperationStartedEventAttributes(_) => EventType::NexusOperationStarted,
        Attributes::NexusOperationCompletedEventAttributes(_) => EventType::NexusOperationCompleted,
        Attributes::NexusOperationFailedEventAttributes(_) => EventType::NexusOperationFailed,
        Attributes::NexusOperationCanceledEventAttributes(_) => EventType::NexusOperationCanceled,
        Attributes::NexusOperationTimedOutEventAttributes(_) => EventType::NexusOperationTimedOut,
        Attributes::NexusOperationCancelRequestedEventAttributes(_) => {
            EventType::NexusOperationCancelRequested
        }
        Attributes::WorkflowExecutionOptionsUpdatedEventAttributes(_) => {
            EventType::WorkflowExecutionOptionsUpdated
        }
        Attributes::NexusOperationCancelRequestCompletedEventAttributes(_) => {
            EventType::NexusOperationCancelRequestCompleted
        }
        Attributes::NexusOperationCancelRequestFailedEventAttributes(_) => {
            EventType::NexusOperationCancelRequestFailed
        }
        Attributes::WorkflowExecutionPausedEventAttributes(_) => EventType::WorkflowExecutionPaused,
        Attributes::WorkflowExecutionUnpausedEventAttributes(_) => {
            EventType::WorkflowExecutionUnpaused
        }
        Attributes::WorkflowExecutionTimeSkippingTransitionedEventAttributes(_) => {
            EventType::WorkflowExecutionTimeSkippingTransitioned
        }
    }
}
