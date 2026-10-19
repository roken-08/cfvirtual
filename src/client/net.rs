//! Small typed WebSocket transport; rendering, retry policy and input stay in main/TUI.
use crate::server::proto::{ClientEnvelope, ServerEnvelope, MAX_MESSAGE_BYTES, VERSION};
pub use crate::server::proto::{ClientMessage, ServerMessage, Snapshot};
use anyhow::{anyhow, bail, Context, Result};
use futures_util::{SinkExt, StreamExt};
use serde::{Deserialize, Serialize};
use tokio::net::TcpStream;
use tokio_tungstenite::{
    connect_async_with_config,
    tungstenite::{protocol::WebSocketConfig, Message},
    MaybeTlsStream, WebSocketStream,
};

/// Bearer credential: keep in session memory, never log it or include in exports.
#[derive(Clone, Serialize, Deserialize)]
pub struct ReconnectSession {
    pub room: String,
    pub participant_token: String,
}

pub struct Client {
    socket: WebSocketStream<MaybeTlsStream<TcpStream>>,
    session: Option<ReconnectSession>,
    snapshot: Option<Snapshot>,
}

impl Client {
    /// Connect to ws:// or wss://, using `/` unless the URL specifies a path.
    /// Send a hello/spectate/create/reconnect request next, then receive welcome.
    pub async fn connect(url: &str) -> Result<Self> {
        let config = WebSocketConfig::default()
            .max_message_size(Some(MAX_MESSAGE_BYTES))
            .max_frame_size(Some(MAX_MESSAGE_BYTES));
        let (socket, _) = connect_async_with_config(url, Some(config), false)
            .await
            .context("connect to room server")?;
        Ok(Self {
            socket,
            session: None,
            snapshot: None,
        })
    }

    /// Connect and complete a token reconnect, retaining its full welcome snapshot.
    pub async fn resume(url: &str, session: &ReconnectSession) -> Result<Self> {
        let mut client = Self::connect(url).await?;
        client
            .send(ClientMessage::Reconnect {
                code: session.room.clone(),
                token: session.participant_token.clone(),
            })
            .await?;
        loop {
            match client.recv().await? {
                Some(ServerMessage::Welcome { .. }) => return Ok(client),
                Some(ServerMessage::Error { code, message }) => bail!("{code}: {message}"),
                Some(_) => {}
                None => bail!("Room server closed before welcome"),
            }
        }
    }

    pub async fn send(&mut self, message: ClientMessage) -> Result<()> {
        let text = serde_json::to_string(&ClientEnvelope::from(message))?;
        if text.len() > MAX_MESSAGE_BYTES {
            bail!("Room message exceeds 64 KiB");
        }
        self.socket
            .send(Message::Text(text.into()))
            .await
            .context("send room message")
    }

    /// Receive one typed message. Welcome updates `session`; full-state messages
    /// update `snapshot`. Unknown future server messages are safely skipped.
    pub async fn recv(&mut self) -> Result<Option<ServerMessage>> {
        loop {
            let Some(frame) = self.socket.next().await else {
                return Ok(None);
            };
            match frame.context("receive room message")? {
                Message::Text(text) => {
                    let envelope: ServerEnvelope =
                        serde_json::from_str(&text).context("invalid room response")?;
                    if envelope.v != VERSION {
                        bail!("Room server protocol version mismatch (expected 1)");
                    }
                    match &envelope.message {
                        ServerMessage::Welcome { token, state, .. } => {
                            self.session = Some(ReconnectSession {
                                room: state.code.clone(),
                                participant_token: token.clone(),
                            });
                            self.snapshot = Some(state.clone());
                        }
                        ServerMessage::Tick { state, .. }
                        | ServerMessage::ContestEnded { state, .. } => {
                            self.snapshot = Some(state.clone())
                        }
                        ServerMessage::Unknown => continue,
                        _ => {}
                    }
                    return Ok(Some(envelope.message));
                }
                Message::Ping(bytes) => self.socket.send(Message::Pong(bytes)).await?,
                Message::Close(_) => return Ok(None),
                Message::Pong(_) | Message::Frame(_) => {}
                Message::Binary(_) => return Err(anyhow!("Room server sent a binary message")),
            }
        }
    }

    pub fn session(&self) -> Option<&ReconnectSession> {
        self.session.as_ref()
    }
    pub fn snapshot(&self) -> Option<&Snapshot> {
        self.snapshot.as_ref()
    }
    pub async fn close(&mut self) -> Result<()> {
        self.socket
            .close(None)
            .await
            .context("close room connection")
    }
}
