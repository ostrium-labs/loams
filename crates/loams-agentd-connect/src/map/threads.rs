//! threads.proto: chats, sessions, the sidebar, turns and the transcript.

use std::collections::HashMap;

use loams_agentd_proto as rs;
use loams_agentd_proto::transcript as rs_transcript;

use super::{MapError, Wire, from_msg, from_opt_msg, msg, opt_msg};
use crate::proto::loams::agentd::v1 as pb;
use crate::proto::loams::agentd::v1::__buffa::oneof::{
    message_part::Part, sidebar_pin_change::Change as PinChange,
    sidebar_section_change::Change as SectionChange, tool_call::Call, transcript_update::Frame,
};

wire_struct!(rs::Chat => pb::Chat {
    id => id,
    device_id => device_id,
    title => title,
    archived => archived,
    cwd => cwd,
    branch => branch,
    checkout_id => checkout_id,
    source_context => source_context as opt_msg,
    config => config as opt_msg,
    last_message_preview => last_message_preview,
    last_message_at => last_message_at_ms,
    created_at => created_at_ms,
    harness_session_id => harness_session_id,
    harness_session_cwd => harness_session_cwd,
    space_id => space_id,
    last_seen_at => last_seen_at_ms,
    room_gen => room_gen,
    parent_chat_id => parent_chat_id,
});

wire_struct!(rs::ChatConfig => pb::ChatConfig {
    harness => harness,
    model => model,
    reasoning => reasoning,
    model_options => model_options as map,
    sandbox => sandbox,
});

wire_struct!(rs::ConversationSourceContext => pb::ConversationSourceContext {
    checkout_id => checkout_id,
    repo_root => repo_root,
    cwd => cwd,
    branch => branch,
    head_sha => head_sha,
    observed_at => observed_at_ms,
});

wire_enum!(rs::SessionStatus => pb::SessionStatus {
    Idle => SESSION_STATUS_IDLE,
    Working => SESSION_STATUS_WORKING,
    AwaitingInput => SESSION_STATUS_AWAITING_INPUT,
    Errored => SESSION_STATUS_ERRORED,
});

wire_struct!(rs::Session => pb::Session {
    last_completed_turn => last_completed_turn,
    chat_id => chat_id,
    device_id => device_id,
    status => status,
    started_at => started_at_ms,
    updated_at => updated_at_ms,
});

wire_struct!(rs::SidebarSection => pb::SidebarSection {
    id => id,
    name => name,
    session_ids => session_ids,
    collapsed => collapsed,
});

wire_struct!(rs::SidebarPreferencesState => pb::SidebarPreferencesState {
    revision => revision,
    synced => synced,
    initialized => initialized,
    pinned_session_ids => pinned_session_ids,
    sections => sections,
});

fn position(
    session_id: &str,
    after: &Option<String>,
    before: &Option<String>,
) -> pb::SidebarPinPosition {
    pb::SidebarPinPosition {
        session_id: session_id.to_owned(),
        after: after.clone(),
        before: before.clone(),
        ..Default::default()
    }
}

impl Wire for rs::SidebarPinChange {
    type Pb = pb::SidebarPinChange;
    fn to_pb(&self) -> Self::Pb {
        let change = match self {
            Self::Pin {
                session_id,
                after,
                before,
            } => PinChange::Pin(Box::new(position(session_id, after, before))),
            Self::Move {
                session_id,
                after,
                before,
            } => PinChange::Reorder(Box::new(position(session_id, after, before))),
            Self::Unpin { session_id } => PinChange::from(pb::SidebarUnpin {
                session_id: session_id.clone(),
                ..Default::default()
            }),
            Self::Section { change } => PinChange::from(change.to_pb()),
        };
        pb::SidebarPinChange {
            change: Some(change),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        match pb.change {
            Some(PinChange::Pin(pin)) => Ok(Self::Pin {
                session_id: pin.session_id,
                after: pin.after,
                before: pin.before,
            }),
            Some(PinChange::Reorder(pin)) => Ok(Self::Move {
                session_id: pin.session_id,
                after: pin.after,
                before: pin.before,
            }),
            Some(PinChange::Unpin(unpin)) => Ok(Self::Unpin {
                session_id: unpin.session_id,
            }),
            Some(PinChange::Section(change)) => Ok(Self::Section {
                change: Wire::from_pb(*change)?,
            }),
            None => Err(MapError::empty_oneof("SidebarPinChange.change")),
        }
    }
}

impl Wire for rs::SidebarSectionChange {
    type Pb = pb::SidebarSectionChange;
    fn to_pb(&self) -> Self::Pb {
        let change = match self {
            Self::Create { id, name } => SectionChange::from(pb::SidebarSectionCreate {
                id: id.clone(),
                name: name.clone(),
                ..Default::default()
            }),
            Self::Rename { id, name } => SectionChange::from(pb::SidebarSectionRename {
                id: id.clone(),
                name: name.clone(),
                ..Default::default()
            }),
            Self::Collapse { id, collapsed } => SectionChange::from(pb::SidebarSectionCollapse {
                id: id.clone(),
                collapsed: *collapsed,
                ..Default::default()
            }),
            Self::Delete { id } => SectionChange::from(pb::SidebarSectionRemove {
                id: id.clone(),
                ..Default::default()
            }),
            Self::Assign {
                session_id,
                section_id,
            } => SectionChange::from(pb::SidebarSectionAssign {
                session_id: session_id.clone(),
                section_id: section_id.clone(),
                ..Default::default()
            }),
            Self::Import { sections } => SectionChange::from(pb::SidebarSectionImport {
                sections: sections.to_pb(),
                ..Default::default()
            }),
        };
        pb::SidebarSectionChange {
            change: Some(change),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        match pb.change {
            Some(SectionChange::Create(create)) => Ok(Self::Create {
                id: create.id,
                name: create.name,
            }),
            Some(SectionChange::Rename(rename)) => Ok(Self::Rename {
                id: rename.id,
                name: rename.name,
            }),
            Some(SectionChange::Collapse(collapse)) => Ok(Self::Collapse {
                id: collapse.id,
                collapsed: collapse.collapsed,
            }),
            Some(SectionChange::Remove(remove)) => Ok(Self::Delete { id: remove.id }),
            Some(SectionChange::Assign(assign)) => Ok(Self::Assign {
                session_id: assign.session_id,
                section_id: assign.section_id,
            }),
            Some(SectionChange::ImportSections(import)) => Ok(Self::Import {
                sections: Wire::from_pb(import.sections)?,
            }),
            None => Err(MapError::empty_oneof("SidebarSectionChange.change")),
        }
    }
}

wire_enum!(rs::MessageRole => pb::MessageRole {
    User => MESSAGE_ROLE_USER,
    Assistant => MESSAGE_ROLE_ASSISTANT,
    System => MESSAGE_ROLE_SYSTEM,
});

wire_enum!(rs::MessageStatus => pb::MessageStatus {
    Streaming => MESSAGE_STATUS_STREAMING,
    Complete => MESSAGE_STATUS_COMPLETE,
    Aborted => MESSAGE_STATUS_ABORTED,
});

wire_enum!(rs::SubagentStatus => pb::SubagentStatus {
    Running => SUBAGENT_STATUS_RUNNING,
    Done => SUBAGENT_STATUS_DONE,
    Failed => SUBAGENT_STATUS_FAILED,
});

wire_struct!(rs::SessionMessageEntry => pb::SessionMessageEntry {
    id => id,
    role => role,
    parts => parts,
    created_at => created_at_ms,
    device_id => device_id,
    status => status,
    continuation_of => continuation_of,
    duration_ms => duration_ms,
});

wire_struct!(rs::ToolDiffStat => pb::ToolDiffStat {
    path => path,
    additions => additions,
    deletions => deletions,
});

wire_struct!(rs::ToolDiff => pb::ToolDiff {
    path => path,
    old_text => old_text,
    new_text => new_text,
});

wire_struct!(rs::UserInputQuestion => pb::UserInputQuestion {
    id => id,
    header => header,
    question => question,
    options => options,
    multi_select => multi_select,
    prefill => prefill,
    multiline => multiline,
});

wire_struct!(rs::UserInputAnswer => pb::UserInputAnswer {
    question_id => question_id,
    labels => labels,
});

wire_struct!(rs::TodoItem => pb::TodoItem {
    text => text,
    done => done,
});

/// `Option<Vec<ToolDiffStat>>` on the ndjson wire: a wrapper keeps presence.
struct DiffStats(Vec<rs::ToolDiffStat>);

impl Wire for DiffStats {
    type Pb = pb::ToolDiffStats;
    fn to_pb(&self) -> Self::Pb {
        pb::ToolDiffStats {
            stats: self.0.to_pb(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(Self(Wire::from_pb(pb.stats)?))
    }
}

impl Wire for rs::MessagePart {
    type Pb = pb::MessagePart;
    fn to_pb(&self) -> Self::Pb {
        let part = match self {
            Self::Text { id, text } => Part::from(pb::TextPart {
                id: id.clone(),
                text: text.clone(),
                ..Default::default()
            }),
            Self::Image {
                id,
                path,
                name,
                mime_type,
            } => Part::from(pb::ImagePart {
                id: id.clone(),
                path: path.clone(),
                name: name.clone(),
                mime_type: mime_type.clone(),
                ..Default::default()
            }),
            Self::Reasoning { id, text } => Part::from(pb::ReasoningPart {
                id: id.clone(),
                text: text.clone(),
                ..Default::default()
            }),
            Self::Tool {
                id,
                call,
                is_error,
                resolved,
                output,
                diff,
                output_ref,
                output_bytes,
                diff_ref,
                diff_stats,
                subagent_ref,
                subagent_status,
                subagent_tail,
            } => Part::from(pb::ToolPart {
                id: id.clone(),
                call: msg(call),
                is_error: *is_error,
                resolved: *resolved,
                output: output.clone(),
                diff: opt_msg(diff),
                output_ref: output_ref.clone(),
                output_bytes: *output_bytes,
                diff_ref: diff_ref.clone(),
                diff_stats: opt_msg(&diff_stats.clone().map(DiffStats)),
                subagent_ref: subagent_ref.clone(),
                subagent_status: subagent_status.to_pb(),
                subagent_tail: subagent_tail.clone(),
                ..Default::default()
            }),
            Self::Input {
                id,
                request_id,
                questions,
                resolved,
            } => Part::from(pb::InputPart {
                id: id.clone(),
                request_id: request_id.clone(),
                questions: questions.to_pb(),
                resolved: *resolved,
                ..Default::default()
            }),
            Self::Error { id, message } => Part::from(pb::ErrorPart {
                id: id.clone(),
                message: message.clone(),
                ..Default::default()
            }),
            Self::Fork {
                id,
                source_chat_id,
                source_title,
            } => Part::from(pb::ForkPart {
                id: id.clone(),
                source_chat_id: source_chat_id.clone(),
                source_title: source_title.clone(),
                ..Default::default()
            }),
        };
        pb::MessagePart {
            part: Some(part),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(match pb.part {
            Some(Part::Text(text)) => Self::Text {
                id: text.id,
                text: text.text,
            },
            Some(Part::Image(image)) => {
                let image = *image;
                Self::Image {
                    id: image.id,
                    path: image.path,
                    name: image.name,
                    mime_type: image.mime_type,
                }
            }
            Some(Part::Reasoning(reasoning)) => Self::Reasoning {
                id: reasoning.id,
                text: reasoning.text,
            },
            Some(Part::Tool(tool)) => {
                let tool = *tool;
                Self::Tool {
                    id: tool.id,
                    call: from_msg(tool.call, "call")?,
                    is_error: tool.is_error,
                    resolved: tool.resolved,
                    output: tool.output,
                    diff: from_opt_msg(tool.diff)?,
                    output_ref: tool.output_ref,
                    output_bytes: tool.output_bytes,
                    diff_ref: tool.diff_ref,
                    diff_stats: from_opt_msg::<DiffStats, _>(tool.diff_stats)?.map(|stats| stats.0),
                    subagent_ref: tool.subagent_ref,
                    subagent_status: Wire::from_pb(tool.subagent_status)?,
                    subagent_tail: tool.subagent_tail,
                }
            }
            Some(Part::Input(input)) => {
                let input = *input;
                Self::Input {
                    id: input.id,
                    request_id: input.request_id,
                    questions: Wire::from_pb(input.questions)?,
                    resolved: input.resolved,
                }
            }
            Some(Part::Error(error)) => Self::Error {
                id: error.id,
                message: error.message,
            },
            Some(Part::Fork(fork)) => {
                let fork = *fork;
                Self::Fork {
                    id: fork.id,
                    source_chat_id: fork.source_chat_id,
                    source_title: fork.source_title,
                }
            }
            None => return Err(MapError::empty_oneof("MessagePart.part")),
        })
    }
}

impl Wire for rs::ToolCall {
    type Pb = pb::ToolCall;
    fn to_pb(&self) -> Self::Pb {
        let call = match self {
            Self::Exec { command } => Call::from(pb::ExecCall {
                command: command.clone(),
                ..Default::default()
            }),
            Self::ReadFile { path } => Call::from(pb::ReadFileCall {
                path: path.clone(),
                ..Default::default()
            }),
            Self::WriteFile { path, content } => Call::from(pb::WriteFileCall {
                path: path.clone(),
                content: content.clone(),
                ..Default::default()
            }),
            Self::EditFile {
                path,
                old_string,
                new_string,
            } => Call::from(pb::EditFileCall {
                path: path.clone(),
                old_string: old_string.clone(),
                new_string: new_string.clone(),
                ..Default::default()
            }),
            Self::ApplyPatch { path } => Call::from(pb::ApplyPatchCall {
                path: path.clone(),
                ..Default::default()
            }),
            Self::Search { pattern, path } => Call::from(pb::SearchCall {
                pattern: pattern.clone(),
                path: path.clone(),
                ..Default::default()
            }),
            Self::Glob { pattern } => Call::from(pb::GlobCall {
                pattern: pattern.clone(),
                ..Default::default()
            }),
            Self::WebFetch { url, prompt } => Call::from(pb::WebFetchCall {
                url: url.clone(),
                prompt: prompt.clone(),
                ..Default::default()
            }),
            Self::WebSearch { query } => Call::from(pb::WebSearchCall {
                query: query.clone(),
                ..Default::default()
            }),
            Self::Todo { items } => Call::from(pb::TodoCall {
                items: items.to_pb(),
                ..Default::default()
            }),
            Self::Mcp {
                server,
                tool,
                input,
            } => Call::from(pb::McpCall {
                server: server.clone(),
                tool: tool.clone(),
                input: opt_msg(input),
                ..Default::default()
            }),
            Self::Unknown { name, input } => Call::from(pb::UnknownCall {
                name: name.clone(),
                input: opt_msg(input),
                ..Default::default()
            }),
        };
        pb::ToolCall {
            call: Some(call),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        Ok(match pb.call {
            Some(Call::Exec(exec)) => Self::Exec {
                command: exec.command,
            },
            Some(Call::ReadFile(read)) => Self::ReadFile { path: read.path },
            Some(Call::WriteFile(write)) => Self::WriteFile {
                path: write.path,
                content: write.content,
            },
            Some(Call::EditFile(edit)) => {
                let edit = *edit;
                Self::EditFile {
                    path: edit.path,
                    old_string: edit.old_string,
                    new_string: edit.new_string,
                }
            }
            Some(Call::ApplyPatch(patch)) => Self::ApplyPatch { path: patch.path },
            Some(Call::Search(search)) => Self::Search {
                pattern: search.pattern,
                path: search.path,
            },
            Some(Call::Glob(glob)) => Self::Glob {
                pattern: glob.pattern,
            },
            Some(Call::WebFetch(fetch)) => Self::WebFetch {
                url: fetch.url,
                prompt: fetch.prompt,
            },
            Some(Call::WebSearch(search)) => Self::WebSearch {
                query: search.query,
            },
            Some(Call::Todo(todo)) => Self::Todo {
                items: Wire::from_pb(todo.items)?,
            },
            Some(Call::Mcp(mcp)) => {
                let mcp = *mcp;
                Self::Mcp {
                    server: mcp.server,
                    tool: mcp.tool,
                    input: from_opt_msg(mcp.input)?,
                }
            }
            Some(Call::Unknown(unknown)) => {
                let unknown = *unknown;
                Self::Unknown {
                    name: unknown.name,
                    input: from_opt_msg(unknown.input)?,
                }
            }
            None => return Err(MapError::empty_oneof("ToolCall.call")),
        })
    }
}

wire_struct!(rs::ContextUsage => pb::ContextUsage {
    tokens => tokens,
    window => window,
});

impl Wire for rs_transcript::TranscriptBaseline {
    type Pb = pb::TranscriptBaseline;
    fn to_pb(&self) -> Self::Pb {
        let rs_transcript::TranscriptBaseline { entries } = self;
        pb::TranscriptBaseline {
            entries: entries
                .iter()
                .map(|(entry, parts)| {
                    let lengths = pb::PartLengths {
                        parts: super::map_to(parts),
                        ..Default::default()
                    };
                    (entry.clone(), lengths)
                })
                .collect(),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let entries = pb
            .entries
            .into_iter()
            .map(|(entry, lengths)| {
                let parts: HashMap<String, usize> = super::map_from(lengths.parts)?;
                Ok((entry, parts))
            })
            .collect::<Result<_, MapError>>()?;
        Ok(rs_transcript::TranscriptBaseline { entries })
    }
}

wire_struct!(rs_transcript::TranscriptUpsert => pb::TranscriptUpsert {
    after => after,
    entry => entry as msg,
});

wire_struct!(rs_transcript::TextAppend => pb::TextAppend {
    entry => entry,
    part => part,
    text => text,
    len => len,
});

impl Wire for rs_transcript::TranscriptUpdate {
    type Pb = pb::TranscriptUpdate;
    fn to_pb(&self) -> Self::Pb {
        let rs_transcript::TranscriptUpdate {
            frame,
            context_usage,
            replay_baseline,
            history_pending,
        } = self;
        let frame = match frame {
            rs_transcript::TranscriptFrame::Reset { reset } => Frame::from(pb::TranscriptReset {
                entries: reset.to_pb(),
                ..Default::default()
            }),
            rs_transcript::TranscriptFrame::Delta {
                upsert,
                append,
                remove,
                count,
            } => Frame::from(pb::TranscriptDelta {
                upsert: upsert.to_pb(),
                append: append.to_pb(),
                remove: remove.clone(),
                count: count.to_pb(),
                ..Default::default()
            }),
        };
        pb::TranscriptUpdate {
            context_usage: opt_msg(context_usage),
            replay_baseline: opt_msg(replay_baseline),
            history_pending: *history_pending,
            frame: Some(frame),
            ..Default::default()
        }
    }
    fn from_pb(pb: Self::Pb) -> Result<Self, MapError> {
        let frame = match pb.frame {
            Some(Frame::Reset(reset)) => rs_transcript::TranscriptFrame::Reset {
                reset: Wire::from_pb(reset.entries)?,
            },
            Some(Frame::Delta(delta)) => {
                let delta = *delta;
                rs_transcript::TranscriptFrame::Delta {
                    upsert: Wire::from_pb(delta.upsert)?,
                    append: Wire::from_pb(delta.append)?,
                    remove: delta.remove,
                    count: Wire::from_pb(delta.count)?,
                }
            }
            None => return Err(MapError::empty_oneof("TranscriptUpdate.frame")),
        };
        Ok(rs_transcript::TranscriptUpdate {
            frame,
            context_usage: from_opt_msg(pb.context_usage)?,
            replay_baseline: from_opt_msg(pb.replay_baseline)?,
            history_pending: pb.history_pending,
        })
    }
}

wire_struct!(rs::RunRequest => pb::RunRequest {
    prompt => prompt,
    harness => harness,
    model => model,
    reasoning => reasoning,
    model_options => model_options as map,
    cwd => cwd,
    sandbox => sandbox,
    auto_approve => auto_approve,
    resume => resume,
    attachments => attachments,
    worktree => worktree as opt_msg,
    mcp => mcp as opt_msg,
    mcp_servers => mcp_servers,
});

wire_struct!(rs::WorktreeSpec => pb::WorktreeSpec {
    repo_path => repo_path,
    base => base,
    space_id => space_id,
});

wire_struct!(rs::McpServer => pb::McpServer {
    name => name,
    command => command,
    args => args,
    env => env as map,
});

// Requests whose message is theirs alone.

wire_struct!(rs::ForkSideChatParams => pb::ForkThreadRequest {
    chat_id => chat_id,
    source_chat_id => source_chat_id,
    parent_chat_id => parent_chat_id,
});

wire_struct!(rs::WatchDocMessagesParams => pb::WatchTranscriptRequest {
    chat_id => chat_id,
    opening_tail => opening_tail,
});
