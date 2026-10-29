//! [`Source`] over the Iggy Rust SDK (TCP, a named consumer whose offsets
//! Iggy stores).

use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use iggy::prelude::{
    AutoLogin, Client, Consumer, ConsumerOffsetClient, Credentials, HeaderKind, Identifier,
    IggyClient, IggyClientBuilder, IggyError, IggyMessage, MessageClient, PollingStrategy,
    TopicClient, UserClient,
};
use tokio::sync::Mutex;

use crate::source::{Header, RawMessage, Source, SourceError, TopicInfo};

/// How to reach Iggy.
#[derive(Clone)]
pub struct IggySourceConfig {
    /// `host:port` of Iggy's TCP transport.
    pub address: String,
    pub username: String,
    pub password: String,
    /// The consumer whose offsets Iggy keeps for the sink.
    pub consumer: String,
    /// The longest any one call may take.
    pub request_timeout: Duration,
}

impl std::fmt::Debug for IggySourceConfig {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Never the password.
        f.debug_struct("IggySourceConfig")
            .field("address", &self.address)
            .field("username", &self.username)
            .field("consumer", &self.consumer)
            .field("request_timeout", &self.request_timeout)
            .finish_non_exhaustive()
    }
}

/// A lazily connected Iggy client: every call connects (and signs in) when
/// there is no live client, and drops the client after a transport error so
/// the next call starts a fresh one.
pub struct IggySource {
    config: IggySourceConfig,
    consumer: Consumer,
    client: Mutex<Option<Arc<IggyClient>>>,
}

impl std::fmt::Debug for IggySource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("IggySource")
            .field("config", &self.config)
            .finish_non_exhaustive()
    }
}

fn not_found(err: &IggyError) -> bool {
    matches!(
        err,
        IggyError::ResourceNotFound(_)
            | IggyError::StreamIdNotFound(_)
            | IggyError::StreamNameNotFound(_)
            | IggyError::TopicIdNotFound(_, _)
            | IggyError::TopicNameNotFound(_, _)
            | IggyError::PartitionNotFound(_, _, _)
    )
}

fn identifier(name: &str) -> Result<Identifier, SourceError> {
    Identifier::named(name).map_err(|err| SourceError::Iggy(format!("bad name {name:?}: {err}")))
}

/// The polled message, its headers read the way the gateway reads them.
pub fn raw_message(message: &IggyMessage) -> RawMessage {
    let headers = match message.user_headers_map() {
        Ok(Some(map)) => Ok(map
            .iter()
            .map(|(key, value)| Header {
                name: key.as_bytes().to_vec(),
                value: value.value(),
                int64: (value.kind() == HeaderKind::Int64)
                    .then(|| value.as_int64().ok())
                    .flatten(),
            })
            .collect()),
        Ok(None) if message.header.user_headers_length == 0 => Ok(Vec::new()),
        _ => Err(message.header.user_headers_length),
    };
    RawMessage {
        offset: message.header.offset,
        id: message.header.id,
        timestamp_us: message.header.timestamp,
        origin_timestamp_us: message.header.origin_timestamp,
        payload: message.payload.clone(),
        headers,
    }
}

impl IggySource {
    /// A source that connects on first use.
    pub fn new(config: IggySourceConfig) -> Result<Self, SourceError> {
        let consumer = Consumer::new(identifier(&config.consumer)?);
        Ok(Self {
            config,
            consumer,
            client: Mutex::new(None),
        })
    }

    async fn client(&self) -> Result<Arc<IggyClient>, SourceError> {
        let mut slot = self.client.lock().await;
        if let Some(client) = slot.as_ref() {
            return Ok(client.clone());
        }
        let credentials = Credentials::UsernamePassword(
            self.config.username.clone(),
            secrecy::SecretString::from(self.config.password.clone()),
        );
        let client = IggyClientBuilder::new()
            .with_tcp()
            .with_server_address(self.config.address.clone())
            .with_auto_sign_in(AutoLogin::Enabled(credentials))
            // Reconnection is this type's job (a fresh client per failure),
            // so the SDK gives up quickly instead of retrying inside a call.
            .with_reconnection_max_retries(Some(1))
            .build()
            .map_err(|err| SourceError::Iggy(format!("building the client: {err}")))?;
        tokio::time::timeout(self.config.request_timeout, client.connect())
            .await
            .map_err(|_| {
                SourceError::Iggy(format!("connecting to {} timed out", self.config.address))
            })?
            .map_err(|err| {
                SourceError::Iggy(format!("connecting to {}: {err}", self.config.address))
            })?;
        let client = Arc::new(client);
        *slot = Some(client.clone());
        tracing::info!(address = %self.config.address, "loams_sink connected to Iggy");
        Ok(client)
    }

    /// Runs `call` with a timeout; a transport error drops the client.
    async fn call<T, F>(
        &self,
        what: &str,
        call: impl FnOnce(Arc<IggyClient>) -> F,
    ) -> Result<T, SourceError>
    where
        F: std::future::Future<Output = Result<T, IggyError>>,
    {
        let client = self.client().await?;
        match tokio::time::timeout(self.config.request_timeout, call(client.clone())).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(err)) if not_found(&err) => Err(SourceError::NotFound(format!("{what}: {err}"))),
            outcome => {
                let message = match outcome {
                    Ok(Err(err)) => format!("{what}: {err}"),
                    _ => format!("{what}: timed out"),
                };
                self.reset(&client).await;
                Err(SourceError::Iggy(message))
            }
        }
    }

    async fn reset(&self, failed: &Arc<IggyClient>) {
        let mut slot = self.client.lock().await;
        if slot.as_ref().is_some_and(|c| Arc::ptr_eq(c, failed)) {
            *slot = None;
            let client = failed.clone();
            tokio::spawn(async move {
                let _ = tokio::time::timeout(Duration::from_secs(5), client.logout_user()).await;
                let _ = tokio::time::timeout(Duration::from_secs(5), client.shutdown()).await;
            });
        }
    }

    /// Signs out and closes the connection.
    pub async fn close(&self) {
        if let Some(client) = self.client.lock().await.take() {
            let _ = tokio::time::timeout(Duration::from_secs(5), client.logout_user()).await;
            let _ = tokio::time::timeout(Duration::from_secs(5), client.shutdown()).await;
        }
    }
}

#[async_trait]
impl Source for IggySource {
    async fn topics(&self, stream: &str) -> Result<Vec<TopicInfo>, SourceError> {
        let stream_id = identifier(stream)?;
        let topics = self
            .call("listing topics", |c| async move {
                c.get_topics(&stream_id).await
            })
            .await?;
        Ok(topics
            .into_iter()
            .map(|t| TopicInfo {
                name: t.name,
                partitions: t.partitions_count,
            })
            .collect())
    }

    async fn committed(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
    ) -> Result<Option<u64>, SourceError> {
        let (stream_id, topic_id) = (identifier(stream)?, identifier(topic)?);
        let consumer = self.consumer.clone();
        let info = self
            .call("reading the committed offset", |c| async move {
                match c
                    .get_consumer_offset(&consumer, &stream_id, &topic_id, Some(partition))
                    .await
                {
                    Err(IggyError::ConsumerOffsetNotFound(_)) => Ok(None),
                    other => other,
                }
            })
            .await?;
        Ok(info.map(|i| i.stored_offset))
    }

    async fn poll(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
        count: u32,
    ) -> Result<Vec<RawMessage>, SourceError> {
        let (stream_id, topic_id) = (identifier(stream)?, identifier(topic)?);
        let consumer = self.consumer.clone();
        let polled = self
            .call("polling", |c| async move {
                c.poll_messages(
                    &stream_id,
                    &topic_id,
                    Some(partition),
                    &consumer,
                    &PollingStrategy::offset(offset),
                    count,
                    false,
                )
                .await
            })
            .await?;
        let mut messages: Vec<RawMessage> = polled
            .messages
            .iter()
            .map(raw_message)
            .filter(|m| m.offset >= offset)
            .collect();
        messages.sort_by_key(|m| m.offset);
        Ok(messages)
    }

    async fn commit(
        &self,
        stream: &str,
        topic: &str,
        partition: u32,
        offset: u64,
    ) -> Result<(), SourceError> {
        let (stream_id, topic_id) = (identifier(stream)?, identifier(topic)?);
        let consumer = self.consumer.clone();
        self.call("committing the offset", |c| async move {
            c.store_consumer_offset(&consumer, &stream_id, &topic_id, Some(partition), offset)
                .await
        })
        .await
    }
}
