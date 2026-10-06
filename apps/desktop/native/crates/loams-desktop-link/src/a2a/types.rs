//! The A2A message and task types the stub needs.

use serde::{Deserialize, Deserializer, Serialize, Serializer};

/// Who wrote a message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Role {
    /// The person (or Loams Bot on their behalf).
    User,
    /// An agent.
    Agent,
}

impl Serialize for Role {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(match self {
            Self::User => "ROLE_USER",
            Self::Agent => "ROLE_AGENT",
        })
    }
}

impl<'de> Deserialize<'de> for Role {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        match normalize(&raw, "ROLE_").as_str() {
            "user" => Ok(Self::User),
            "agent" => Ok(Self::Agent),
            other => Err(serde::de::Error::custom(format!("unknown role {other:?}"))),
        }
    }
}

/// `ROLE_USER` / `TASK_STATE_WORKING` / `working` / `input-required` all
/// become one lowercase, separator-free token.
fn normalize(raw: &str, prefix: &str) -> String {
    raw.strip_prefix(prefix)
        .unwrap_or(raw)
        .chars()
        .filter(|c| !matches!(c, '_' | '-'))
        .collect::<String>()
        .to_ascii_lowercase()
}

/// A part of a message or artifact. Only text is modelled; other part kinds
/// deserialize with `text: None` and are skipped by [`Reply`].
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Part {
    /// Plain text.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub text: Option<String>,
}

impl Part {
    /// A text part.
    #[must_use]
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: Some(text.into()),
        }
    }
}

/// An A2A message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Message {
    /// Deterministic per durable step, so a replay deduplicates (D467).
    pub message_id: String,
    /// The chat thread or factory run.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context_id: Option<String>,
    /// The task this message continues, if any.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub task_id: Option<String>,
    /// The author.
    pub role: Role,
    /// The content.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// Task lifecycle states (design 39 section 5.3).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaskState {
    /// Accepted, not started.
    Submitted,
    /// Running.
    Working,
    /// Waiting for a question or an approval; Loams Bot relays, never answers.
    InputRequired,
    /// The agent's app account is not linked.
    AuthRequired,
    /// Done.
    Completed,
    /// Failed.
    Failed,
    /// Canceled.
    Canceled,
    /// Refused by policy.
    Rejected,
    /// A state this stub does not know.
    Unknown,
}

impl TaskState {
    /// No further updates will arrive.
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(
            self,
            Self::Completed | Self::Failed | Self::Canceled | Self::Rejected
        )
    }

    fn wire(self) -> &'static str {
        match self {
            Self::Submitted => "TASK_STATE_SUBMITTED",
            Self::Working => "TASK_STATE_WORKING",
            Self::InputRequired => "TASK_STATE_INPUT_REQUIRED",
            Self::AuthRequired => "TASK_STATE_AUTH_REQUIRED",
            Self::Completed => "TASK_STATE_COMPLETED",
            Self::Failed => "TASK_STATE_FAILED",
            Self::Canceled => "TASK_STATE_CANCELED",
            Self::Rejected => "TASK_STATE_REJECTED",
            Self::Unknown => "TASK_STATE_UNSPECIFIED",
        }
    }
}

impl Serialize for TaskState {
    fn serialize<S: Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.wire())
    }
}

impl<'de> Deserialize<'de> for TaskState {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let raw = String::deserialize(d)?;
        Ok(match normalize(&raw, "TASK_STATE_").as_str() {
            "submitted" => Self::Submitted,
            "working" => Self::Working,
            "inputrequired" => Self::InputRequired,
            "authrequired" => Self::AuthRequired,
            "completed" => Self::Completed,
            "failed" => Self::Failed,
            "canceled" | "cancelled" => Self::Canceled,
            "rejected" => Self::Rejected,
            _ => Self::Unknown,
        })
    }
}

/// A task's status.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TaskStatus {
    /// The state.
    pub state: TaskState,
    /// An optional status message (a question, a failure reason).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub message: Option<Message>,
}

/// A task result, for example an issue key and URL.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Artifact {
    /// Stable within the task.
    #[serde(default)]
    pub artifact_id: String,
    /// A display name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// The content.
    #[serde(default)]
    pub parts: Vec<Part>,
}

/// An A2A task; its id is the Loams operation id (D467).
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Task {
    /// The task id.
    pub id: String,
    /// The thread it belongs to.
    #[serde(default)]
    pub context_id: String,
    /// Where it stands.
    pub status: TaskStatus,
    /// What it produced.
    #[serde(default)]
    pub artifacts: Vec<Artifact>,
}

/// `SendMessage` answers with a task or, for a quick reply, a message.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SendMessageResponse {
    /// A task to follow.
    Task(Task),
    /// An immediate message.
    Message(Message),
}

/// What the chat surface needs from a response: the text to show and the
/// ids to continue with.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reply {
    /// The thread id to send the next message under.
    pub context_id: Option<String>,
    /// The task id, when the agent answered with a task.
    pub task_id: Option<String>,
    /// The task state; `Completed` for a plain message.
    pub state: TaskState,
    /// The agent's text: status message, then artifact text, then message text.
    pub text: String,
}

impl Reply {
    /// Flattens a response for display.
    #[must_use]
    pub fn from_response(response: &SendMessageResponse) -> Self {
        match response {
            SendMessageResponse::Message(m) => Self {
                context_id: m.context_id.clone(),
                task_id: m.task_id.clone(),
                state: TaskState::Completed,
                text: join_text(&m.parts),
            },
            SendMessageResponse::Task(t) => {
                let mut chunks = Vec::new();
                if let Some(m) = &t.status.message {
                    chunks.push(join_text(&m.parts));
                }
                chunks.extend(t.artifacts.iter().map(|a| join_text(&a.parts)));
                chunks.retain(|c| !c.is_empty());
                Self {
                    context_id: Some(t.context_id.clone()).filter(|c| !c.is_empty()),
                    task_id: Some(t.id.clone()),
                    state: t.status.state,
                    text: chunks.join("\n\n"),
                }
            }
        }
    }
}

fn join_text(parts: &[Part]) -> String {
    parts
        .iter()
        .filter_map(|p| p.text.as_deref())
        .collect::<Vec<_>>()
        .join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn task_state_accepts_proto_and_lowercase_spellings() {
        for (raw, want) in [
            ("TASK_STATE_WORKING", TaskState::Working),
            ("working", TaskState::Working),
            ("input-required", TaskState::InputRequired),
            ("TASK_STATE_INPUT_REQUIRED", TaskState::InputRequired),
            ("cancelled", TaskState::Canceled),
            ("something-new", TaskState::Unknown),
        ] {
            let got: TaskState = serde_json::from_value(serde_json::json!(raw)).unwrap();
            assert_eq!(got, want, "{raw}");
        }
        assert!(TaskState::Completed.is_terminal() && !TaskState::InputRequired.is_terminal());
    }

    #[test]
    fn message_serializes_as_protojson() {
        let m = Message {
            message_id: "m1".into(),
            context_id: Some("c1".into()),
            task_id: None,
            role: Role::User,
            parts: vec![Part::text("hi")],
        };
        assert_eq!(
            serde_json::to_value(&m).unwrap(),
            serde_json::json!({"messageId":"m1","contextId":"c1","role":"ROLE_USER","parts":[{"text":"hi"}]})
        );
    }

    #[test]
    fn reply_flattens_a_task() {
        let task: SendMessageResponse = serde_json::from_value(serde_json::json!({
            "task": {"id":"op-1","contextId":"c1",
              "status":{"state":"TASK_STATE_COMPLETED"},
              "artifacts":[{"artifactId":"a","parts":[{"text":"PLN-42 created"}]}]}
        }))
        .unwrap();
        let r = Reply::from_response(&task);
        assert_eq!(r.text, "PLN-42 created");
        assert_eq!(r.task_id.as_deref(), Some("op-1"));
        assert_eq!(r.state, TaskState::Completed);
    }
}
