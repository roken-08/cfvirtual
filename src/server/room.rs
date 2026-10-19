use super::{proto::*, CodeforcesBackend, RoomBackend};
use crate::{cf::CfClient, domain::*};
use rand::{rngs::OsRng, Rng, RngCore};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap},
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{mpsc, watch, Mutex, Notify, RwLock};
use uuid::Uuid;

pub const MAX_PLAYERS: usize = 32;
pub const MAX_SPECTATORS: usize = 8;
pub const MAX_BOTS: usize = 16;
const MAX_ROOMS: usize = 128;
const CODE_ALPHABET: &[u8] = b"23456789ABCDEFGHJKMNPQRSTVWXYZ";

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct RoomError {
    pub code: &'static str,
    pub message: String,
}

impl RoomError {
    pub(crate) fn new(code: &'static str, message: &str) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
    pub(crate) fn wire(&self) -> ServerMessage {
        ServerMessage::Error {
            code: self.code.into(),
            message: self.message.clone(),
        }
    }
}

// These contain bearer credentials; do not derive Debug.
pub struct CreatedRoom {
    pub code: String,
    pub host_token: String,
}

#[derive(Clone)]
pub struct State {
    pub(crate) inner: Arc<Inner>,
}

pub(crate) struct Inner {
    pub backend: Arc<dyn RoomBackend>,
    pub data_dir: PathBuf,
    pub rooms: RwLock<HashMap<String, Arc<Mutex<Room>>>>,
}

impl State {
    pub fn new(api: CfClient, data_dir: PathBuf) -> Self {
        Self::with_backend(
            Arc::new(CodeforcesBackend {
                api,
                data_dir: data_dir.clone(),
            }),
            data_dir,
        )
    }

    /// Inject a trusted server-side backend; see [`RoomBackend`]'s contract.
    pub fn with_backend(backend: Arc<dyn RoomBackend>, data_dir: PathBuf) -> Self {
        Self {
            inner: Arc::new(Inner {
                backend,
                data_dir,
                rooms: RwLock::new(HashMap::new()),
            }),
        }
    }

    /// Install a prepared contest whose host was already validated by the caller.
    /// All input tokens/start/end/code are replaced; only the host occupies the lobby.
    /// Use `service::prepare(..., "room")` for production contests.
    pub async fn create_room(
        &self,
        mut contest: Contest,
        password: Option<String>,
    ) -> Result<CreatedRoom, RoomError> {
        validate_password(&password)?;
        if contest.bots.len() > MAX_BOTS
            || contest.problems.is_empty()
            || contest.problems.len() > 26
            || contest.duration_sec == 0
            || contest.duration_sec > 86400
            || !valid_handle(&contest.host_handle)
        {
            return Err(RoomError::new(
                "INVALID_OPTIONS",
                "Use 1–26 problems, 1–86400 seconds, at most 16 bots, and a valid host handle.",
            ));
        }
        if contest
            .bots
            .iter()
            .any(|b| b.name.eq_ignore_ascii_case(&contest.host_handle))
        {
            return Err(RoomError::new(
                "INVALID_OPTIONS",
                "Bot names must differ from player handles.",
            ));
        }
        let mut rooms = self.inner.rooms.write().await;
        if rooms.len() >= MAX_ROOMS {
            return Err(RoomError::new(
                "SERVER_FULL",
                "The server's room capacity has been reached.",
            ));
        }
        let code = loop {
            let code: String = (0..6)
                .map(|_| CODE_ALPHABET[OsRng.gen_range(0..CODE_ALPHABET.len())] as char)
                .collect();
            if !rooms.contains_key(&code) {
                break code;
            }
        };
        let host = Member::new(
            contest.host_handle.clone(),
            contest.host_rating,
            true,
            false,
        );
        let host_token = host.token.clone();
        contest.code = Some(code.clone());
        contest.mode = "room".into();
        contest.started_at = 0;
        contest.ended_at = None;
        contest.participants.clear();
        let room = Arc::new(Mutex::new(Room {
            code: code.clone(),
            contest,
            members: vec![host],
            password: password.map(Password::new),
            phase: Phase::Lobby,
            end_reason: None,
            submissions: HashMap::new(),
            wake: Arc::new(Notify::new()),
        }));
        rooms.insert(code.clone(), room.clone());
        drop(rooms);
        tokio::spawn(super::poller::drive(self.clone(), room));
        Ok(CreatedRoom { code, host_token })
    }

    pub(crate) async fn lookup(&self, code: &str) -> Result<Arc<Mutex<Room>>, RoomError> {
        self.inner
            .rooms
            .read()
            .await
            .get(&code.to_ascii_uppercase())
            .cloned()
            .ok_or_else(|| RoomError::new("ROOM_NOT_FOUND", "No room has that code."))
    }
}

pub(crate) fn valid_handle(handle: &str) -> bool {
    !handle.is_empty()
        && handle.len() <= 24
        && handle
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b"_.-".contains(&b))
}

pub(crate) fn validate_password(password: &Option<String>) -> Result<(), RoomError> {
    if password.as_ref().is_some_and(|p| p.len() > 1024) {
        return Err(RoomError::new(
            "INVALID_PASSWORD",
            "Room passwords may be at most 1024 bytes.",
        ));
    }
    Ok(())
}

struct Password {
    salt: [u8; 32],
    hash: [u8; 32],
}
impl Password {
    fn new(password: String) -> Self {
        let mut salt = [0; 32];
        OsRng.fill_bytes(&mut salt);
        let hash = Sha256::new()
            .chain_update(salt)
            .chain_update(password.as_bytes())
            .finalize()
            .into();
        Self { salt, hash }
    }
    fn matches(&self, password: &str) -> bool {
        let candidate: [u8; 32] = Sha256::new()
            .chain_update(self.salt)
            .chain_update(password.as_bytes())
            .finalize()
            .into();
        self.hash
            .iter()
            .zip(candidate)
            .fold(0u8, |diff, (a, b)| diff | (*a ^ b))
            == 0
    }
}

pub(crate) struct Connection {
    pub generation: Uuid,
    pub tx: mpsc::Sender<ServerMessage>,
    pub stop: watch::Sender<bool>,
}

pub(crate) struct Member {
    pub id: String,
    pub token: String,
    pub handle: String,
    pub rating: u32,
    pub is_host: bool,
    pub is_spectator: bool,
    pub connection: Option<Connection>,
}

impl Member {
    pub fn new(handle: String, rating: u32, is_host: bool, is_spectator: bool) -> Self {
        Self {
            id: Uuid::new_v4().to_string(),
            token: Uuid::new_v4().to_string(),
            handle,
            rating,
            is_host,
            is_spectator,
            connection: None,
        }
    }
    fn public(&self) -> PublicParticipant {
        PublicParticipant {
            participant_id: self.id.clone(),
            handle: self.handle.clone(),
            is_host: self.is_host,
            is_spectator: self.is_spectator,
            connected: self.connection.is_some(),
        }
    }
}

pub(crate) struct Room {
    pub code: String,
    pub contest: Contest,
    pub members: Vec<Member>,
    password: Option<Password>,
    pub phase: Phase,
    pub end_reason: Option<EndReason>,
    pub submissions: HashMap<String, BTreeMap<u64, Submission>>,
    pub wake: Arc<Notify>,
}

impl Room {
    pub fn check_password(&self, password: &Option<String>) -> Result<(), RoomError> {
        if self
            .password
            .as_ref()
            .is_some_and(|p| !p.matches(password.as_deref().unwrap_or_default()))
        {
            return Err(RoomError::new(
                "BAD_PASSWORD",
                "The room password is incorrect.",
            ));
        }
        Ok(())
    }

    pub fn check_join(&self, handle: &str, spectator: bool) -> Result<(), RoomError> {
        if !spectator && self.phase != Phase::Lobby {
            return Err(RoomError::new(
                "CONTEST_STARTED",
                "Players cannot join after the lobby; reconnect with your token.",
            ));
        }
        let count = self
            .members
            .iter()
            .filter(|p| p.is_spectator == spectator)
            .count();
        if count
            >= if spectator {
                MAX_SPECTATORS
            } else {
                MAX_PLAYERS
            }
        {
            return Err(RoomError::new(
                "ROOM_FULL",
                "The player or spectator limit has been reached.",
            ));
        }
        if self
            .members
            .iter()
            .any(|p| p.handle.eq_ignore_ascii_case(handle))
            || self
                .contest
                .bots
                .iter()
                .any(|p| p.name.eq_ignore_ascii_case(handle))
        {
            return Err(RoomError::new(
                "HANDLE_IN_USE",
                "That handle is already present; reconnect with your token.",
            ));
        }
        Ok(())
    }

    pub fn attach(&mut self, index: usize, connection: Connection) {
        if let Some(old) = self.members[index].connection.replace(connection) {
            let _ = old.stop.send(true);
        }
        let state = self.snapshot(index);
        let member = &self.members[index];
        self.send(
            index,
            ServerMessage::Welcome {
                participant_id: member.id.clone(),
                token: member.token.clone(),
                state,
            },
        );
        self.broadcast_roster();
        self.broadcast_tick();
    }

    pub fn active(&self, index: usize, generation: Uuid) -> bool {
        self.members
            .get(index)
            .and_then(|p| p.connection.as_ref())
            .is_some_and(|c| c.generation == generation)
    }

    pub fn disconnect(&mut self, index: usize, generation: Uuid) {
        if self.active(index, generation) {
            self.members[index].connection = None;
            self.broadcast_roster();
            self.broadcast_tick();
        }
    }

    pub fn start(&mut self, index: usize) -> Result<(), RoomError> {
        self.authorize(index)?;
        if self.phase != Phase::Lobby {
            return Err(RoomError::new(
                "INVALID_STATE",
                "The room is no longer in the lobby.",
            ));
        }
        self.contest.started_at = chrono::Utc::now().timestamp();
        self.phase = Phase::Running;
        self.sync_participants();
        let problems = self.public_problems();
        self.broadcast(ServerMessage::ContestStarted {
            contest_id: self.contest.id.clone(),
            started_at: self.contest.started_at,
            duration_sec: self.contest.duration_sec,
            problems,
            blind: self.contest.blind,
            server_time: chrono::Utc::now().timestamp(),
        });
        self.broadcast_tick();
        self.wake.notify_one();
        Ok(())
    }

    pub fn end(&mut self, index: usize) -> Result<(), RoomError> {
        self.authorize(index)?;
        if self.phase != Phase::Running {
            return Err(RoomError::new(
                "INVALID_STATE",
                "Only a running contest can be ended.",
            ));
        }
        let now = chrono::Utc::now().timestamp();
        let reason = if now >= self.contest.deadline() {
            EndReason::Timeout
        } else {
            EndReason::Host
        };
        self.begin_finish(reason);
        Ok(())
    }

    fn authorize(&self, index: usize) -> Result<(), RoomError> {
        if !self.members[index].is_host {
            return Err(RoomError::new(
                "NOT_HOST",
                "Only the host may perform this action.",
            ));
        }
        Ok(())
    }

    pub fn begin_finish(&mut self, reason: EndReason) {
        if self.phase != Phase::Running {
            return;
        }
        self.contest.ended_at = Some(chrono::Utc::now().timestamp().min(self.contest.deadline()));
        self.end_reason = Some(reason);
        self.phase = Phase::Finishing;
        self.broadcast_tick();
        self.wake.notify_one();
    }

    pub fn sync_participants(&mut self) {
        self.contest.participants = self
            .members
            .iter()
            .map(|m| Participant {
                handle: m.handle.clone(),
                is_host: m.is_host,
                is_spectator: m.is_spectator,
                token: String::new(),
                connected: m.connection.is_some(),
            })
            .collect();
    }

    pub fn scores(&self) -> Vec<Standing> {
        if self.phase == Phase::Lobby {
            return Vec::new();
        }
        let submissions: Vec<Submission> = self
            .submissions
            .values()
            .flat_map(|s| s.values().cloned())
            .collect();
        // Domain scoring is the sole authority for humans and elapsed bot events.
        let mut scores =
            scoring::score(&self.contest, &submissions, chrono::Utc::now().timestamp());
        for standing in &mut scores {
            if !standing.is_bot {
                if let Some(member) = self
                    .members
                    .iter()
                    .find(|m| m.handle.eq_ignore_ascii_case(&standing.name))
                {
                    standing.rating = member.rating;
                }
            }
        }
        scores
    }

    fn settings(&self) -> Settings {
        Settings {
            duration_sec: self.contest.duration_sec,
            problem_count: self.contest.problems.len(),
            blind: self.contest.blind,
            password_required: self.password.is_some(),
        }
    }

    fn public_bots(&self) -> Vec<PublicBot> {
        self.contest
            .bots
            .iter()
            .map(|b| PublicBot {
                name: b.name.clone(),
                rating: b.rating,
                is_bot: true,
            })
            .collect()
    }

    fn public_problems(&self) -> Vec<PublicProblem> {
        if self.phase == Phase::Lobby {
            return Vec::new();
        }
        let hide = self.contest.blind && self.phase != Phase::Ended;
        self.contest
            .problems
            .iter()
            .map(|p| PublicProblem {
                slot: p.slot,
                contest_id: p.problem.key.contest_id,
                index: p.problem.key.index.clone(),
                name: p.problem.name.clone(),
                url: p.problem.url(),
                rating: if hide { None } else { p.problem.rating },
                tags: if hide {
                    Vec::new()
                } else {
                    p.problem.tags.clone()
                },
            })
            .collect()
    }

    pub fn snapshot(&self, recipient: usize) -> Snapshot {
        let scores = self.scores();
        self.snapshot_with_scores(recipient, &scores)
    }

    fn snapshot_with_scores(&self, recipient: usize, scores: &[Standing]) -> Snapshot {
        let member = &self.members[recipient];
        let problem_status = if member.is_spectator {
            BTreeMap::new()
        } else {
            scores
                .iter()
                .find(|s| !s.is_bot && s.name.eq_ignore_ascii_case(&member.handle))
                .map(|s| BTreeMap::from([(member.handle.clone(), s.problems.clone())]))
                .unwrap_or_default()
        };
        Snapshot {
            code: self.code.clone(),
            contest_id: self.contest.id.clone(),
            host_handle: self.contest.host_handle.clone(),
            phase: self.phase,
            started_at: self.contest.started_at,
            deadline: (self.phase != Phase::Lobby).then(|| self.contest.deadline()),
            ended_at: self.contest.ended_at,
            end_reason: self.end_reason,
            server_time: chrono::Utc::now().timestamp(),
            settings: self.settings(),
            participants: self.members.iter().map(Member::public).collect(),
            bots: self.public_bots(),
            problems: self.public_problems(),
            problem_status,
            standings: scores
                .iter()
                .map(|s| PublicStanding {
                    name: s.name.clone(),
                    is_bot: s.is_bot,
                    rating: s.rating,
                    solved: s.solved,
                    penalty: s.penalty,
                    last_ac: s.last_ac,
                })
                .collect(),
        }
    }

    pub fn send(&mut self, index: usize, message: ServerMessage) {
        let failed = self.members[index]
            .connection
            .as_ref()
            .is_some_and(|c| c.tx.try_send(message).is_err());
        if failed {
            if let Some(c) = self.members[index].connection.take() {
                let _ = c.stop.send(true);
            }
        }
    }

    pub fn broadcast(&mut self, message: ServerMessage) {
        for i in 0..self.members.len() {
            self.send(i, message.clone());
        }
    }

    pub fn broadcast_roster(&mut self) {
        self.broadcast(ServerMessage::Roster {
            participants: self.members.iter().map(Member::public).collect(),
            bots: self.public_bots(),
            settings: self.settings(),
        });
    }

    pub fn broadcast_tick(&mut self) {
        let scores = self.scores();
        for i in 0..self.members.len() {
            if self.members[i].connection.is_none() {
                continue;
            }
            let state = self.snapshot_with_scores(i, &scores);
            self.send(
                i,
                ServerMessage::Tick {
                    server_time: state.server_time,
                    standings: state.standings.clone(),
                    problem_status: state.problem_status.clone(),
                    state,
                },
            );
        }
    }

    pub fn finish(&mut self) {
        self.phase = Phase::Ended;
        self.sync_participants();
        let scores = self.scores();
        for i in 0..self.members.len() {
            let state = self.snapshot_with_scores(i, &scores);
            self.send(
                i,
                ServerMessage::ContestEnded {
                    reason: self.end_reason.unwrap_or(EndReason::Timeout),
                    standings: state.standings.clone(),
                    state,
                },
            );
        }
    }
}
