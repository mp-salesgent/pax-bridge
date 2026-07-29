//! WebSocket `/ws` — lifecycle event broadcast. Mirrors `attachWebSocket` /
//! `emitPaymentEvent` in `bridge/index.js`: every connecting client is sent a
//! `HELLO` immediately, and every payment lifecycle event is fanned out to
//! all currently-connected clients.

use crate::bridge::AppState;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::State;
use axum::response::Response;
use serde_json::{json, Value};
use tokio::sync::broadcast;

pub struct Broadcaster {
    tx: broadcast::Sender<String>,
}

impl Broadcaster {
    pub fn new() -> Self {
        // Bounded channel: slow/gone clients drop messages rather than backing
        // up the sender; each socket task holds its own receiver.
        let (tx, _rx) = broadcast::channel(256);
        Self { tx }
    }

    fn subscribe(&self) -> broadcast::Receiver<String> {
        self.tx.subscribe()
    }

    /// Broadcast a payment lifecycle event to all connected clients.
    /// `evt` must include at least `{ type, txnId }`; `ts` (epoch ms) is added.
    pub fn emit(&self, mut evt: Value) {
        if self.tx.receiver_count() == 0 {
            return;
        }
        if let Value::Object(map) = &mut evt {
            map.insert("ts".to_string(), json!(chrono::Utc::now().timestamp_millis()));
        }
        let payload = evt.to_string();
        // No receivers is a valid, non-error state (mirrors JS's for-loop over clients).
        let _ = self.tx.send(payload);
    }
}

impl Default for Broadcaster {
    fn default() -> Self {
        Self::new()
    }
}

pub async fn ws_handler(ws: WebSocketUpgrade, State(state): State<AppState>) -> Response {
    ws.on_upgrade(move |socket| handle_socket(socket, state))
}

async fn handle_socket(mut socket: WebSocket, state: AppState) {
    let hello = json!({ "type": "HELLO", "ts": chrono::Utc::now().timestamp_millis() });
    if socket.send(Message::Text(hello.to_string().into())).await.is_err() {
        return;
    }

    let mut rx = state.ws.subscribe();
    loop {
        tokio::select! {
            broadcast_msg = rx.recv() => {
                match broadcast_msg {
                    Ok(payload) => {
                        if socket.send(Message::Text(payload.into())).await.is_err() {
                            break;
                        }
                    }
                    Err(broadcast::error::RecvError::Lagged(_)) => continue,
                    Err(broadcast::error::RecvError::Closed) => break,
                }
            }
            incoming = socket.recv() => {
                match incoming {
                    Some(Ok(_)) => { /* clients don't send anything meaningful; ignore */ }
                    Some(Err(_)) | None => break,
                }
            }
        }
    }
}
