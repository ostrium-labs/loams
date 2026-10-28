//! The real transport: a WebSocket client for a CDP endpoint (`ws://` on
//! loopback or a private network, `wss://` anywhere else).
//!
//! Requests are matched to responses by id in a reader task. Unsolicited
//! events go to an event channel that one consumer may take
//! ([`CdpConnection::take_events`]); until it is taken they are dropped, and
//! they are never mistaken for answers.

use std::collections::HashMap;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

use futures::{SinkExt, StreamExt};
use serde_json::Value;
use tokio::sync::{Mutex, mpsc, oneshot};
use tokio_tungstenite::tungstenite::Message;

use crate::cdp::{CdpConnection, CdpEvent, CdpRequest, CdpResponse, CdpTransport, RequestHeaders};
use crate::error::BridgeError;

/// Opens CDP sockets.
#[derive(Debug, Default, Clone, Copy)]
pub struct WebSocketTransport;

#[async_trait::async_trait]
impl CdpTransport for WebSocketTransport {
    async fn connect(
        &self,
        endpoint: &str,
        headers: &RequestHeaders,
    ) -> Result<Arc<dyn CdpConnection>, BridgeError> {
        let mut request = tokio_tungstenite::tungstenite::http::Request::builder()
            .method("GET")
            .uri(endpoint);
        for (name, value) in headers.as_pairs() {
            request = request.header(name.as_str(), value.as_str());
        }
        let request = request
            .body(())
            .map_err(|error| BridgeError::transport(format!("building the request: {error}")))?;

        let (stream, _response) = tokio_tungstenite::connect_async(request)
            .await
            .map_err(|error| BridgeError::transport(format!("connecting: {error}")))?;

        let (mut writer, mut reader) = stream.split();
        let (sender, mut outbound) = mpsc::unbounded_channel::<Message>();
        let pending: Arc<Mutex<HashMap<u64, oneshot::Sender<CdpResponse>>>> =
            Arc::new(Mutex::new(HashMap::new()));

        let (event_sender, event_receiver) = mpsc::unbounded_channel::<CdpEvent>();
        let reader_pending = Arc::clone(&pending);
        let writer_task = tokio::spawn(async move {
            while let Some(message) = outbound.recv().await {
                if writer.send(message).await.is_err() {
                    break;
                }
            }
            let _ = writer.close().await;
        });

        let reader_task = tokio::spawn(async move {
            while let Some(message) = reader.next().await {
                let Ok(message) = message else { break };
                let text = match message {
                    Message::Text(text) => text,
                    Message::Close(_) => break,
                    _ => continue,
                };
                let Ok(message) = serde_json::from_str::<Value>(&text) else {
                    continue;
                };
                if let Some(event) = CdpEvent::from_message(&message) {
                    // Nobody listening is not an error: the event is dropped.
                    let _ = event_sender.send(event);
                    continue;
                }
                let Ok(response) = serde_json::from_value::<CdpResponse>(message) else {
                    continue;
                };
                let Some(id) = response.id else { continue };
                let waiter = reader_pending.lock().await.remove(&id);
                if let Some(waiter) = waiter {
                    let _ = waiter.send(response);
                }
            }
            // The socket is gone: nothing will answer, so fail every waiter
            // rather than leaving the provider waiting for its timeout.
            let mut waiters = reader_pending.lock().await;
            waiters.clear();
        });

        Ok(Arc::new(WebSocketConnection {
            sender,
            pending,
            next_id: AtomicU64::new(1),
            tasks: Mutex::new(vec![writer_task, reader_task]),
            closed: AtomicBool::new(false),
            events: std::sync::Mutex::new(Some(event_receiver)),
        }))
    }
}

#[derive(Debug)]
struct WebSocketConnection {
    sender: mpsc::UnboundedSender<Message>,
    pending: Arc<Mutex<HashMap<u64, oneshot::Sender<CdpResponse>>>>,
    next_id: AtomicU64,
    tasks: Mutex<Vec<tokio::task::JoinHandle<()>>>,
    closed: AtomicBool,
    events: std::sync::Mutex<Option<mpsc::UnboundedReceiver<CdpEvent>>>,
}

impl WebSocketConnection {
    async fn fail_all(&self, error: BridgeError) {
        let mut waiters = self.pending.lock().await;
        for (_, waiter) in waiters.drain() {
            let _ = waiter.send(CdpResponse {
                id: None,
                result: None,
                error: Some(crate::cdp::CdpErrorBody {
                    code: -1,
                    message: error.to_string(),
                }),
            });
        }
    }
}

#[async_trait::async_trait]
impl CdpConnection for WebSocketConnection {
    async fn send(&self, request: CdpRequest) -> Result<Value, BridgeError> {
        if self.closed.load(Ordering::Relaxed) {
            return Err(BridgeError::transport("the session is closed"));
        }
        let id = if request.id == 0 {
            self.next_id.fetch_add(1, Ordering::Relaxed)
        } else {
            request.id
        };
        let (sender, receiver) = oneshot::channel();
        self.pending.lock().await.insert(id, sender);

        let text = serde_json::to_string(&CdpRequest { id, ..request })
            .map_err(|error| BridgeError::transport(format!("encoding: {error}")))?;
        if self.sender.send(Message::Text(text.into())).is_err() {
            self.pending.lock().await.remove(&id);
            return Err(BridgeError::transport("the session socket is gone"));
        }

        let response = receiver
            .await
            .map_err(|_| BridgeError::transport("the session ended before the answer arrived"))?;
        match (response.result, response.error) {
            (_, Some(error)) => Err(BridgeError::cdp(error.to_string())),
            (Some(result), None) => Ok(result),
            (None, None) => Err(BridgeError::cdp("the browser sent an empty response")),
        }
    }

    fn take_events(&self) -> Option<mpsc::UnboundedReceiver<CdpEvent>> {
        self.events
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()
    }

    async fn close(&self) -> Result<(), BridgeError> {
        if self.closed.swap(true, Ordering::Relaxed) {
            return Ok(());
        }
        for task in self.tasks.lock().await.drain(..) {
            task.abort();
        }
        self.fail_all(BridgeError::transport("the session was closed"))
            .await;
        Ok(())
    }
}

/// The `Value` a CDP call returns when a field is missing.
pub fn field<'a>(value: &'a Value, name: &str) -> Option<&'a Value> {
    value.get(name).filter(|found| !found.is_null())
}
