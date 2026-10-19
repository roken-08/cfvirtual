use super::{
    proto::*,
    room::{self, Connection, Member, Room, RoomError},
    State,
};
use axum::{
    extract::{
        ws::{Message, WebSocket, WebSocketUpgrade},
        State as AxumState,
    },
    response::Response,
};
use futures_util::{SinkExt, StreamExt};
use std::{sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch, Mutex};
use uuid::Uuid;

pub(crate) async fn upgrade(AxumState(state): AxumState<State>, ws: WebSocketUpgrade) -> Response {
    ws.max_message_size(MAX_MESSAGE_BYTES)
        .max_frame_size(MAX_MESSAGE_BYTES)
        .on_upgrade(move |socket| connection(socket, state))
}

struct Binding {
    room: Arc<Mutex<Room>>,
    index: usize,
    generation: Uuid,
}

async fn connection(socket: WebSocket, state: State) {
    let (mut sink, mut stream) = socket.split();
    let (tx, mut rx) = mpsc::channel::<ServerMessage>(OUTBOUND_CAPACITY);
    let (stop, mut stopped) = watch::channel(false);
    let writer_stop = stop.clone();
    let writer = tokio::spawn(async move {
        loop {
            let message = tokio::select! {
                biased;
                _ = stopped.changed() => break,
                message = rx.recv() => match message { Some(m) => m, None => break },
            };
            let Ok(text) = serde_json::to_string(&ServerEnvelope::from(message)) else {
                break;
            };
            if text.len() > MAX_MESSAGE_BYTES {
                break;
            }
            if !matches!(
                tokio::time::timeout(
                    Duration::from_secs(5),
                    sink.send(Message::Text(text.into()))
                )
                .await,
                Ok(Ok(()))
            ) {
                break;
            }
        }
        let _ = writer_stop.send(true);
        let _ = tokio::time::timeout(Duration::from_secs(1), sink.close()).await;
    });
    let mut reader_stop = stop.subscribe();
    let mut binding: Option<Binding> = None;
    loop {
        let frame = tokio::select! {
            biased;
            _ = reader_stop.changed() => break,
            frame = tokio::time::timeout(if binding.is_none() { Duration::from_secs(30) } else { Duration::from_secs(3600) }, stream.next()) => frame,
        };
        let Ok(Some(Ok(frame))) = frame else {
            break;
        };
        let text = match frame {
            Message::Text(text) => text,
            Message::Close(_) => break,
            Message::Ping(_) | Message::Pong(_) => continue,
            Message::Binary(_) => {
                if tx
                    .try_send(RoomError::new("INVALID_MESSAGE", "Use JSON text frames.").wire())
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        let request = match serde_json::from_str::<ClientEnvelope>(&text) {
            Ok(request) => request,
            Err(_) => {
                if tx
                    .try_send(
                        RoomError::new("INVALID_MESSAGE", "Expected a versioned JSON request.")
                            .wire(),
                    )
                    .is_err()
                {
                    break;
                }
                continue;
            }
        };
        if request.v != VERSION {
            let _ = tx.try_send(
                RoomError::new("VERSION_MISMATCH", "This server requires protocol v: 1.").wire(),
            );
            break;
        }
        if let Some(b) = &binding {
            if !b.room.lock().await.active(b.index, b.generation) {
                break;
            }
        }
        let result = match request.message {
            ClientMessage::Unknown => Ok(()),
            ClientMessage::Ping { t } => tx
                .try_send(ServerMessage::Pong {
                    t,
                    server_time: chrono::Utc::now().timestamp_millis(),
                })
                .map_err(|_| RoomError::new("SLOW_CLIENT", "The outgoing queue is full.")),
            ClientMessage::Start | ClientMessage::End => {
                if let Some(b) = &binding {
                    let mut room = b.room.lock().await;
                    // Replacement may have happened after the first generation check.
                    if !room.active(b.index, b.generation) {
                        break;
                    }
                    if matches!(request.message, ClientMessage::Start) {
                        room.start(b.index)
                    } else {
                        room.end(b.index)
                    }
                } else {
                    Err(RoomError::new("NOT_JOINED", "Join a room first."))
                }
            }
            message => {
                if binding.is_some() {
                    Err(RoomError::new(
                        "ALREADY_JOINED",
                        "This socket already belongs to a room.",
                    ))
                } else {
                    match join(&state, message, tx.clone(), stop.clone()).await {
                        Ok(joined) => {
                            binding = Some(joined);
                            Ok(())
                        }
                        Err(error) => Err(error),
                    }
                }
            }
        };
        if let Err(error) = result {
            if tx.try_send(error.wire()).is_err() {
                break;
            }
        }
    }
    if let Some(b) = binding {
        b.room.lock().await.disconnect(b.index, b.generation);
    }
    drop(tx);
    // Drain errors (notably version mismatch) before closing the writer.
    let _ = writer.await;
}

async fn join(
    state: &State,
    message: ClientMessage,
    tx: mpsc::Sender<ServerMessage>,
    stop: watch::Sender<bool>,
) -> Result<Binding, RoomError> {
    let generation = Uuid::new_v4();
    let connection = Connection {
        generation,
        tx,
        stop,
    };
    match message {
        ClientMessage::Create { options, password } => {
            room::validate_password(&password)?;
            if options.bots > room::MAX_BOTS
                || options.problems == 0
                || options.problems > 26
                || options.duration_sec == 0
                || options.duration_sec > 86400
                || !room::valid_handle(&options.handle)
            {
                return Err(RoomError::new(
                    "INVALID_OPTIONS",
                    "Invalid contest options or room limits exceeded.",
                ));
            }
            let contest = tokio::time::timeout(Duration::from_secs(180), state.inner.backend.prepare(&options)).await
                .map_err(|_| RoomError::new("CF_UNAVAILABLE", "Contest preparation timed out; retry later."))?
                .map_err(|_| RoomError::new("PREPARE_FAILED", "Could not prepare the room; check the handle, options and Codeforces availability."))?;
            let created = state.create_room(contest, password).await?;
            let room = state.lookup(&created.code).await?;
            room.lock().await.attach(0, connection);
            Ok(Binding {
                room,
                index: 0,
                generation,
            })
        }
        ClientMessage::Reconnect { code, token } => {
            let room = state.lookup(&code).await?;
            let mut guard = room.lock().await;
            let index = guard
                .members
                .iter()
                .position(|p| p.token == token)
                .ok_or_else(|| {
                    RoomError::new(
                        "BAD_TOKEN",
                        "That reconnect token is not valid for this room.",
                    )
                })?;
            guard.attach(index, connection);
            drop(guard);
            Ok(Binding {
                room,
                index,
                generation,
            })
        }
        ClientMessage::Hello {
            code,
            handle,
            password,
        } => {
            if !room::valid_handle(&handle) {
                return Err(RoomError::new(
                    "INVALID_HANDLE",
                    "Invalid Codeforces handle.",
                ));
            }
            room::validate_password(&password)?;
            let room = state.lookup(&code).await?;
            {
                let guard = room.lock().await;
                guard.check_password(&password)?;
                guard.check_join(&handle, false)?;
            }
            // Never keep a room/network lock while validating with the public API.
            let user = tokio::time::timeout(Duration::from_secs(90), state.inner.backend.validate_handle(&handle)).await
                .map_err(|_| RoomError::new("CF_UNAVAILABLE", "Handle validation timed out; retry later."))?
                .map_err(|_| RoomError::new("INVALID_HANDLE", "Could not validate this Codeforces handle; check spelling and API availability."))?;
            if !room::valid_handle(&user.handle) {
                return Err(RoomError::new(
                    "INVALID_HANDLE",
                    "The profile returned an invalid handle.",
                ));
            }
            let mut guard = room.lock().await;
            guard.check_join(&user.handle, false)?;
            let index = guard.members.len();
            guard
                .members
                .push(Member::new(user.handle, user.rating, false, false));
            guard.attach(index, connection);
            drop(guard);
            Ok(Binding {
                room,
                index,
                generation,
            })
        }
        ClientMessage::Spectate { code, password } => {
            room::validate_password(&password)?;
            let room = state.lookup(&code).await?;
            let mut guard = room.lock().await;
            guard.check_password(&password)?;
            let handle = format!("spectator_{}", Uuid::new_v4().simple());
            guard.check_join(&handle, true)?;
            let index = guard.members.len();
            guard.members.push(Member::new(handle, 0, false, true));
            guard.attach(index, connection);
            drop(guard);
            Ok(Binding {
                room,
                index,
                generation,
            })
        }
        _ => Err(RoomError::new(
            "NOT_JOINED",
            "Send hello, reconnect, spectate or create first.",
        )),
    }
}
