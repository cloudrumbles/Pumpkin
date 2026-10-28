//! Background generation of distant terrain for Ironore's client.
//!
//! A client asks on `ironore:lod_request` for every generated chunk within a
//! radius of its player. Each world runs one service task that walks the disc
//! around every requesting player nearest first. It sends chunks that exist
//! and are newer than what the client has, and generates the rest through
//! background tickets, which the chunk scheduler runs on its own low-priority
//! pool. Ironore's `docs/terrain/lod-generation.md` is the specification.

mod disc;
mod known;
mod protocol;
mod window;

use std::collections::VecDeque;
use std::sync::atomic::Ordering::Relaxed;
use std::sync::{Arc, OnceLock, Weak};
use std::time::{Duration, Instant};

use pumpkin_protocol::java::client::play::CCustomPayload;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::text::TextComponent;
use pumpkin_world::chunk::ChunkData;
use pumpkin_world::chunk::io::{FileIO, LoadedData};
use pumpkin_world::level::Level;
use rustc_hash::{FxHashMap, FxHashSet};
use tokio::sync::mpsc;
use tracing::{error, info, warn};
use uuid::Uuid;

use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::net::{ClientPlatform, DisconnectReason};
use crate::world::World;
use disc::DiscOrder;
use known::KnownDisc;
use protocol::{Progress, Request};
use window::AckWindow;

type ChunkPos = Vector2<i32>;

/// Background tickets one world holds at once. Each pulls the chunks its
/// generation depends on into memory, so this bounds memory as well as
/// keeping the background pool busy.
const MAX_TICKETS: usize = 128;
/// Chunks a session may hold unsent: waiting on generation or a disk read,
/// or read and waiting for room in its acknowledgement window.
const LOOKAHEAD: usize = 512;
/// Disc positions a session looks at in one pass.
const SCAN_BUDGET: usize = 16_384;
const READ_BATCH: usize = 32;
const MAX_READS: usize = 256;
const PASS_INTERVAL: Duration = Duration::from_millis(50);
const PROGRESS_INTERVAL: Duration = Duration::from_millis(250);
const RECOUNT_INTERVAL: Duration = Duration::from_secs(1);
const RATE_LOG_INTERVAL: Duration = Duration::from_secs(10);

/// One world's background generation, started by its first request.
#[derive(Default)]
pub struct LodService {
    sender: OnceLock<mpsc::UnboundedSender<Message>>,
}

enum Message {
    Request {
        player: Arc<Player>,
        request: Request,
    },
    Ack {
        player: Uuid,
        applied: i32,
    },
}

impl LodService {
    fn send(&self, world: &Arc<World>, message: Message) {
        let sender = self.sender.get_or_init(|| {
            let (sender, receiver) = mpsc::unbounded_channel();
            tokio::spawn(Service::new(world, receiver).run());
            sender
        });
        if sender.send(message).is_err() {
            error!(
                "Background generation for {} has stopped; dropping an ironore: packet",
                world.dimension.minecraft_name
            );
        }
    }
}

/// Handles a custom payload if it is on one of the `ironore:` channels a
/// client sends, and returns whether it was. A malformed payload disconnects
/// the player.
pub fn receive_payload(player: &Arc<Player>, channel: &str, data: &[u8]) -> bool {
    let message = match channel {
        protocol::REQUEST => protocol::read_request(data).map(|request| Message::Request {
            player: player.clone(),
            request,
        }),
        protocol::ACK => protocol::read_ack(data).map(|applied| Message::Ack {
            player: player.gameprofile.id,
            applied,
        }),
        _ => return false,
    };
    match message {
        Ok(message) => {
            let world = player.world();
            world.lod.send(&world, message);
        }
        Err(error) => disconnect(player, &error.to_string()),
    }
    true
}

fn disconnect(player: &Player, reason: &str) {
    warn!("Disconnecting {}: {reason}", player.gameprofile.name);
    player.kick(
        DisconnectReason::UnexpectedPacket,
        &TextComponent::text(reason.to_owned()),
    );
}

/// What one client has asked for and what it has been given.
struct Session {
    player: Arc<Player>,
    radius: i32,
    known: KnownDisc,
    centre: ChunkPos,
    /// Every position of the disc order before this index has been looked at
    /// from the current centre.
    scan: usize,
    /// Positions the client has, with the sequence it has them at: sent on
    /// `lod_chunk`, already known to it, or inside its view distance, which
    /// the vanilla packets carry and which count as had at any sequence.
    delivered: FxHashMap<ChunkPos, i64>,
    /// Positions waiting on generation or a disk read for this client.
    pending: FxHashSet<ChunkPos>,
    ready: VecDeque<ReadyChunk>,
    window: AckWindow,
    /// Positions sent on `lod_chunk` and not yet acknowledged, in send order,
    /// with a set of the same positions for lookups.
    unacknowledged: VecDeque<ChunkPos>,
    unacknowledged_set: FxHashSet<ChunkPos>,
    /// The newest sequence at the last recount, and the radius around the
    /// centre within which the client then had every generated chunk.
    complete_through: i64,
    complete_radius: i32,
    generated_in_disc: usize,
    delivered_in_disc: usize,
    counts_stale: bool,
    last_recount: Option<Instant>,
    last_progress: Option<Instant>,
    reported_done: bool,
}

impl Session {
    fn new(player: Arc<Player>, request: Request) -> Self {
        let centre = player.get_entity().chunk_pos.load();
        Self {
            player,
            radius: request.radius,
            known: KnownDisc::from_request(&request),
            centre,
            scan: 0,
            delivered: FxHashMap::default(),
            pending: FxHashSet::default(),
            ready: VecDeque::new(),
            window: AckWindow::default(),
            unacknowledged: VecDeque::new(),
            unacknowledged_set: FxHashSet::default(),
            complete_through: -1,
            complete_radius: 0,
            generated_in_disc: 0,
            delivered_in_disc: 0,
            counts_stale: true,
            last_recount: None,
            last_progress: None,
            reported_done: false,
        }
    }

    fn in_disc(&self, pos: ChunkPos) -> bool {
        disc::is_within(pos.x - self.centre.x, pos.y - self.centre.y, self.radius)
    }

    fn in_view(&self, pos: ChunkPos) -> bool {
        self.player
            .watched_section
            .load()
            .is_within_distance(pos.x, pos.y)
    }

    /// Records that the client has the chunk at `pos` as of `sequence`, and
    /// returns whether it did not have it at that sequence before.
    fn deliver(&mut self, pos: ChunkPos, sequence: i64) -> bool {
        match self.delivered.insert(pos, sequence) {
            Some(previous) if previous >= sequence => {
                self.delivered.insert(pos, previous);
                false
            }
            Some(_) => true,
            None => {
                if self.in_disc(pos) {
                    self.delivered_in_disc += 1;
                }
                true
            }
        }
    }

    /// Returns whether the client has acknowledged the chunk at `pos` at
    /// `sequence` or later, or had it before.
    fn has(&self, pos: ChunkPos, sequence: i64) -> bool {
        self.delivered
            .get(&pos)
            .is_some_and(|&had| had >= sequence)
            && !self.unacknowledged_set.contains(&pos)
    }

    fn acknowledge(&mut self, applied: i32) -> Result<(), window::AckError> {
        let before = self.window.applied();
        self.window.acknowledge(applied)?;
        let newly = (self.window.applied() - before) as usize;
        for pos in self.unacknowledged.drain(..newly) {
            self.unacknowledged_set.remove(&pos);
        }
        Ok(())
    }

    /// Moves the disc to `centre`. Positions already looked at that are
    /// closer to the new centre than anything not yet looked at stay done;
    /// the scan restarts from there.
    fn recentre(&mut self, centre: ChunkPos, waiting_on_work: &dyn Fn(ChunkPos) -> bool) {
        let order = DiscOrder::get();
        let moved = disc::ring_of(centre.x - self.centre.x, centre.y - self.centre.y) + 1;
        let done = if self.scan < order.len_within(self.radius) {
            order.ring_at(self.scan)
        } else {
            self.radius as u32
        };
        self.scan = order.len_within(done.saturating_sub(moved) as i32);
        self.centre = centre;
        // Positions still queued for a ticket are looked at again from the new
        // centre, in the new order.
        self.pending.retain(|&pos| waiting_on_work(pos));
        self.counts_stale = true;
        self.reported_done = false;
    }

    fn has_work(&self) -> bool {
        !self.pending.is_empty()
            || !self.ready.is_empty()
            || !self.unacknowledged.is_empty()
            || self.scan < DiscOrder::get().len_within(self.radius)
    }
}

/// A chunk encoded for `lod_chunk`.
#[derive(Clone)]
struct ReadyChunk {
    pos: ChunkPos,
    sequence: i64,
    payload: Arc<[u8]>,
}

struct ReadResult {
    pos: ChunkPos,
    chunk: Result<ReadyChunk, String>,
}

struct Service {
    world: Weak<World>,
    level: Arc<Level>,
    messages: mpsc::UnboundedReceiver<Message>,
    completed: crossbeam::channel::Receiver<(ChunkPos, Weak<ChunkData>)>,
    read_sender: mpsc::UnboundedSender<ReadResult>,
    read_results: mpsc::UnboundedReceiver<ReadResult>,
    sessions: Vec<Session>,
    /// Positions this service holds a background ticket for.
    tickets: FxHashSet<ChunkPos>,
    /// Positions to generate once a ticket is free, nearest first.
    to_generate: VecDeque<ChunkPos>,
    to_read: Vec<ChunkPos>,
    reading: FxHashSet<ChunkPos>,
    rate: RateLog,
}

impl Service {
    fn new(world: &Arc<World>, messages: mpsc::UnboundedReceiver<Message>) -> Self {
        let (read_sender, read_results) = mpsc::unbounded_channel();
        Self {
            world: Arc::downgrade(world),
            level: world.level.clone(),
            messages,
            completed: world.level.chunk_listener.add_global_chunk_listener(),
            read_sender,
            read_results,
            sessions: Vec::new(),
            tickets: FxHashSet::default(),
            to_generate: VecDeque::new(),
            to_read: Vec::new(),
            reading: FxHashSet::default(),
            rate: RateLog::new(world.level.generation_journal.generated_count()),
        }
    }

    async fn run(mut self) {
        let mut interval = tokio::time::interval(PASS_INTERVAL);
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                message = self.messages.recv() => match message {
                    Some(message) => self.handle(message),
                    None => break,
                },
                _ = interval.tick() => {}
            }
            while let Ok(message) = self.messages.try_recv() {
                self.handle(message);
            }
            let Some(world) = self.world.upgrade() else {
                break;
            };
            if self.level.shut_down_chunk_system.load(Relaxed)
                || crate::SERVER_IS_STOPPING.load(Relaxed)
            {
                break;
            }
            if let Some(failure) = self.level.generation_journal.failure() {
                error!("Stopping the server, because the generation journal failed: {failure}");
                crate::stop_server();
                break;
            }
            self.pass(&world).await;
        }
        self.release_tickets();
    }

    fn handle(&mut self, message: Message) {
        match message {
            Message::Request { player, request } => {
                if !(1..=disc::MAX_RADIUS).contains(&request.radius)
                    || !(0..=disc::MAX_RADIUS).contains(&request.known_radius)
                {
                    disconnect(
                        &player,
                        &format!(
                            "{} asked for radius {} with a known radius of {}; both must be at most {}",
                            protocol::REQUEST,
                            request.radius,
                            request.known_radius,
                            disc::MAX_RADIUS
                        ),
                    );
                    return;
                }
                let id = player.gameprofile.id;
                self.sessions.retain(|session| session.player.gameprofile.id != id);
                self.sessions.push(Session::new(player, request));
            }
            Message::Ack { player, applied } => {
                let Some(index) = self
                    .sessions
                    .iter()
                    .position(|session| session.player.gameprofile.id == player)
                else {
                    // The session ended, by a dimension change for example.
                    return;
                };
                if let Err(error) = self.sessions[index].acknowledge(applied) {
                    let session = self.sessions.swap_remove(index);
                    disconnect(&session.player, &format!("{}: {error}", protocol::ACK));
                }
            }
        }
    }

    async fn pass(&mut self, world: &Arc<World>) {
        self.retire_sessions(world);
        self.follow_players();
        self.collect_completed();
        self.collect_reads();
        self.scan();
        self.issue_tickets();
        self.issue_reads();
        self.send_ready().await;
        self.send_progress().await;
        self.rate.maybe_log(world, &self.sessions, self.tickets.len());
    }

    fn retire_sessions(&mut self, world: &World) {
        let players = world.players.load();
        self.sessions.retain(|session| {
            let connected = match session.player.client.as_ref() {
                ClientPlatform::Java(client) => !client.is_closed(),
                ClientPlatform::Bedrock(_) => false,
            };
            connected
                && players
                    .iter()
                    .any(|player| Arc::ptr_eq(player, &session.player))
        });
    }

    fn follow_players(&mut self) {
        let tickets = &self.tickets;
        let reading = &self.reading;
        let waiting_on_work = |pos: ChunkPos| tickets.contains(&pos) || reading.contains(&pos);
        for session in &mut self.sessions {
            let centre = session.player.get_entity().chunk_pos.load();
            if centre != session.centre {
                session.recentre(centre, &waiting_on_work);
            }
        }
    }

    fn collect_completed(&mut self) {
        let mut finished = Vec::new();
        while let Ok((pos, chunk)) = self.completed.try_recv() {
            if self.tickets.remove(&pos) {
                finished.push((pos, chunk));
            }
        }
        if finished.is_empty() {
            return;
        }
        self.remove_tickets(finished.iter().map(|(pos, _)| *pos));
        for (pos, chunk) in finished {
            match chunk.upgrade() {
                Some(chunk) => self.deliver_chunk(pos, &chunk),
                // Unloaded again before this pass saw it; generate it again.
                None => self.to_generate.push_front(pos),
            }
        }
    }

    /// Encodes a chunk that finished generating or was found loaded, and
    /// queues it for every session waiting on it.
    fn deliver_chunk(&mut self, pos: ChunkPos, chunk: &ChunkData) {
        let journal = &self.level.generation_journal;
        let sequence = journal
            .sequence(pos)
            .unwrap_or_else(|| journal.record_existing(pos));
        match protocol::write_chunk(sequence, chunk) {
            Ok(payload) => self.queue_ready(&ReadyChunk {
                pos,
                sequence,
                payload: payload.into(),
            }),
            Err(error) => {
                error!("Could not encode chunk {pos:?} for {}: {error}", protocol::CHUNK);
                crate::stop_server();
            }
        }
    }

    fn queue_ready(&mut self, chunk: &ReadyChunk) {
        for session in &mut self.sessions {
            if session.pending.remove(&chunk.pos) {
                session.ready.push_back(chunk.clone());
                session.counts_stale = true;
            }
        }
    }

    fn collect_reads(&mut self) {
        while let Ok(result) = self.read_results.try_recv() {
            self.reading.remove(&result.pos);
            match result.chunk {
                Ok(chunk) => self.queue_ready(&chunk),
                Err(reason) => {
                    error!(
                        "The generation journal lists chunk {:?}, but it could not be read ({reason}); generating it again",
                        result.pos
                    );
                    self.to_generate.push_front(result.pos);
                }
            }
        }
    }

    fn scan(&mut self) {
        let order = DiscOrder::get();
        let journal = &self.level.generation_journal;
        let mut loaded = Vec::new();
        for session in &mut self.sessions {
            let offsets = order.within(session.radius);
            let mut looked = 0;
            while session.scan < offsets.len()
                && looked < SCAN_BUDGET
                && session.pending.len() + session.ready.len() < LOOKAHEAD
            {
                let (dx, dz) = offsets[session.scan];
                let pos = session
                    .centre
                    .add_raw(i32::from(dx), i32::from(dz));
                session.scan += 1;
                looked += 1;
                if session.delivered.contains_key(&pos) || session.pending.contains(&pos) {
                    continue;
                }
                if session.in_view(pos) {
                    session.deliver(pos, i64::MAX);
                    continue;
                }
                match journal.sequence(pos) {
                    Some(sequence) if session.known.has(pos, sequence) => {
                        session.deliver(pos, sequence);
                    }
                    Some(_) => {
                        session.pending.insert(pos);
                        // A chunk generated but not yet saved is only in memory.
                        if let Some(chunk) = self.level.loaded_chunks.get(&pos) {
                            loaded.push((pos, chunk.clone()));
                        } else if !self.reading.contains(&pos) {
                            self.to_read.push(pos);
                        }
                    }
                    None => {
                        session.pending.insert(pos);
                        if !self.tickets.contains(&pos) {
                            self.to_generate.push_back(pos);
                        }
                    }
                }
            }
        }
        for (pos, chunk) in loaded {
            self.deliver_chunk(pos, &chunk);
        }
    }

    fn issue_tickets(&mut self) {
        let mut issued = Vec::new();
        while self.tickets.len() < MAX_TICKETS {
            let Some(pos) = self.to_generate.pop_front() else {
                break;
            };
            let wanted = self
                .sessions
                .iter()
                .any(|session| session.pending.contains(&pos));
            if wanted && self.tickets.insert(pos) {
                issued.push(pos);
            }
        }
        if issued.is_empty() {
            return;
        }
        let mut loading = self
            .level
            .chunk_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for &pos in &issued {
            loading.add_background_ticket(pos);
        }
        loading.send_change();
        drop(loading);
        // A chunk that is already loaded is not announced again, so take it now.
        let mut already_loaded = Vec::new();
        for pos in issued {
            if let Some(chunk) = self.level.loaded_chunks.get(&pos) {
                already_loaded.push((pos, chunk.clone()));
            }
        }
        if already_loaded.is_empty() {
            return;
        }
        for (pos, _) in &already_loaded {
            self.tickets.remove(pos);
        }
        self.remove_tickets(already_loaded.iter().map(|(pos, _)| *pos));
        for (pos, chunk) in already_loaded {
            self.deliver_chunk(pos, &chunk);
        }
    }

    fn remove_tickets(&self, positions: impl Iterator<Item = ChunkPos>) {
        let mut loading = self
            .level
            .chunk_loading
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        for pos in positions {
            loading.remove_background_ticket(pos);
        }
        loading.send_change();
    }

    fn release_tickets(&mut self) {
        let tickets: Vec<_> = self.tickets.drain().collect();
        if !tickets.is_empty() {
            self.remove_tickets(tickets.into_iter());
        }
    }

    fn issue_reads(&mut self) {
        while self.reading.len() < MAX_READS && !self.to_read.is_empty() {
            let take = self.to_read.len().min(READ_BATCH);
            let batch: Vec<_> = self
                .to_read
                .drain(..take)
                .filter(|&pos| self.reading.insert(pos))
                .collect();
            if batch.is_empty() {
                continue;
            }
            tokio::spawn(read_chunks(
                self.level.clone(),
                batch,
                self.read_sender.clone(),
            ));
        }
    }

    async fn send_ready(&mut self) {
        for session in &mut self.sessions {
            let player = session.player.clone();
            let ClientPlatform::Java(client) = player.client.as_ref() else {
                continue;
            };
            while session.window.has_room() {
                let Some(chunk) = session.ready.pop_front() else {
                    break;
                };
                if !session.in_disc(chunk.pos) {
                    // The player moved away; it is looked at again if they return.
                    continue;
                }
                if session.in_view(chunk.pos) {
                    session.deliver(chunk.pos, i64::MAX);
                    continue;
                }
                if !session.deliver(chunk.pos, chunk.sequence) {
                    continue;
                }
                client
                    .enqueue_client_packet(&CCustomPayload::new(protocol::CHUNK, &chunk.payload))
                    .await;
                session.window.record_sent();
                session.unacknowledged.push_back(chunk.pos);
                session.unacknowledged_set.insert(chunk.pos);
            }
        }
    }

    async fn send_progress(&mut self) {
        let order = DiscOrder::get();
        let journal = &self.level.generation_journal;
        let now = Instant::now();
        for session in &mut self.sessions {
            let has_work = session.has_work();
            let due = session
                .last_progress
                .is_none_or(|last| now.duration_since(last) >= PROGRESS_INTERVAL);
            let report = if has_work {
                due
            } else {
                !session.reported_done
            };
            if !report {
                continue;
            }
            let recount_due = session
                .last_recount
                .is_none_or(|last| now.duration_since(last) >= RECOUNT_INTERVAL);
            // Chunks generated for players nearby count too, so recount on a
            // timer rather than only after this service changed something.
            if recount_due || (session.counts_stale && !has_work) {
                let mut generated = 0;
                let mut nearest_missing: Option<u32> = None;
                let centre = session.centre;
                let last_sequence =
                    journal.for_each_within(centre, session.radius, |pos, _| {
                        generated += 1;
                        // A chunk re-saved after it went out still counts:
                        // the client follows its changes on the vanilla
                        // packets while it is loaded near the player, so
                        // only a chunk never acknowledged holds the disc in.
                        if !session.delivered.contains_key(&pos)
                            || session.unacknowledged_set.contains(&pos)
                        {
                            let ring = disc::ring_of(pos.x - centre.x, pos.y - centre.y);
                            nearest_missing = Some(nearest_missing.map_or(ring, |near| near.min(ring)));
                        }
                    });
                session.generated_in_disc = generated;
                session.complete_through = last_sequence;
                session.complete_radius = known::complete_radius(nearest_missing, session.radius);
                session.delivered_in_disc = session
                    .delivered
                    .keys()
                    .filter(|&&pos| session.in_disc(pos))
                    .count();
                session.counts_stale = false;
                session.last_recount = Some(now);
            }
            let ClientPlatform::Java(client) = session.player.client.as_ref() else {
                continue;
            };
            let progress = Progress {
                total: saturating_i32(order.len_within(session.radius)),
                generated: saturating_i32(session.generated_in_disc),
                sent: saturating_i32(session.delivered_in_disc),
                centre_x: session.centre.x,
                centre_z: session.centre.y,
                complete_through: session.complete_through,
                complete_radius: session.complete_radius,
            };
            client
                .enqueue_client_packet(&CCustomPayload::new(
                    protocol::PROGRESS,
                    &protocol::write_progress(&progress),
                ))
                .await;
            session.last_progress = Some(now);
            session.reported_done = !has_work;
        }
    }
}

fn saturating_i32(value: usize) -> i32 {
    i32::try_from(value).unwrap_or(i32::MAX)
}

/// Reads chunks the journal lists from their region files and encodes them.
async fn read_chunks(
    level: Arc<Level>,
    batch: Vec<ChunkPos>,
    results: mpsc::UnboundedSender<ReadResult>,
) {
    let (sender, mut receiver) = mpsc::channel(batch.len());
    let fetch = level
        .chunk_saver
        .fetch_chunks(&level.level_folder, &batch, sender);
    let mut loaded = Vec::with_capacity(batch.len());
    let collect = async {
        while let Some(data) = receiver.recv().await {
            loaded.push(data);
        }
    };
    tokio::join!(fetch, collect);
    for data in loaded {
        let (pos, chunk) = match data {
            LoadedData::Loaded(chunk) => {
                let pos = Vector2::new(chunk.x, chunk.z);
                let journal = &level.generation_journal;
                let sequence = journal
                    .sequence(pos)
                    .unwrap_or_else(|| journal.record_existing(pos));
                let chunk = protocol::write_chunk(sequence, &chunk)
                    .map(|payload| ReadyChunk {
                        pos,
                        sequence,
                        payload: payload.into(),
                    })
                    .map_err(|error| error.to_string());
                (pos, chunk)
            }
            LoadedData::Missing(pos) => (pos, Err("it is missing from its region file".to_owned())),
            LoadedData::Error((pos, error)) => (pos, Err(error.to_string())),
        };
        if results.send(ReadResult { pos, chunk }).is_err() {
            return;
        }
    }
}

/// Logs the generation rate every [`RATE_LOG_INTERVAL`] while there is work.
struct RateLog {
    last: Instant,
    last_generated: u64,
}

impl RateLog {
    fn new(generated: u64) -> Self {
        Self {
            last: Instant::now(),
            last_generated: generated,
        }
    }

    fn maybe_log(&mut self, world: &World, sessions: &[Session], tickets: usize) {
        let elapsed = self.last.elapsed();
        if elapsed < RATE_LOG_INTERVAL {
            return;
        }
        let generated = world.level.generation_journal.generated_count();
        let delta = generated - self.last_generated;
        self.last = Instant::now();
        self.last_generated = generated;
        if delta == 0 && tickets == 0 && sessions.iter().all(|session| !session.has_work()) {
            return;
        }
        let rate = delta as f64 / elapsed.as_secs_f64();
        let (tick_mean, tick_max) = world.server.upgrade().map_or((0.0, 0.0), |server| {
            let times = server
                .tick_times_nanos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let recorded: Vec<i64> = times.iter().copied().filter(|&t| t > 0).collect();
            let max = recorded.iter().copied().max().unwrap_or(0);
            let mean = if recorded.is_empty() {
                0.0
            } else {
                recorded.iter().sum::<i64>() as f64 / recorded.len() as f64
            };
            (mean / 1e6, max as f64 / 1e6)
        });
        let order = DiscOrder::get();
        let progress = sessions.first().map_or_else(String::new, |session| {
            format!(
                "; {} of {} chunks in the disc around {:?}, {} generated, {} sent, held whole to radius {}",
                session.player.gameprofile.name,
                order.len_within(session.radius),
                session.centre,
                session.generated_in_disc,
                session.delivered_in_disc,
                session.complete_radius,
            )
        });
        info!(
            "Background generation in {}: {rate:.1} chunks/s, {tickets} in progress{progress}; tick mean {tick_mean:.1} ms, max {tick_max:.1} ms over the last 100 ticks",
            world.dimension.minecraft_name
        );
    }
}
