//! Headless bot swarm for `RustGot` benchmarks.
//!
//! Each bot joins in offline mode, rises to a fixed height and then flies outward in its own
//! direction, so the server has to generate, load and send fresh chunks for every bot (like real
//! players exploring). Bots answer keep-alives, pings, teleports and chunk batches like the
//! vanilla client, so this works against both `RustGot` and Paper/Purpur.
//!
//! Usage: `rustgot-loadbot <addr> <bots> [fly-seconds]`, for example
//! `rustgot-loadbot 127.0.0.1:25565 50 60`. Runs until killed.
//!
//! Paper/Purpur need `allow-flight=true` and `online-mode=false` in `server.properties`.

use std::{
    f64::consts::TAU,
    net::SocketAddr,
    process::ExitCode,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use bytes::Bytes;
use pumpkin_data::packet::CURRENT_MC_VERSION;
use pumpkin_protocol::{
    ClientPacket, ConnectionState, ServerPacket,
    codec::var_int::VarInt,
    java::{
        client::{
            config::{CFinishConfig, CKnownPacks},
            login::{CLoginSuccess, CSetCompression},
            play::{CChunkBatchEnd, CKeepAlive, CLogin, CPlayPing, CPlayerPosition},
        },
        packet_decoder::TCPNetworkDecoder,
        packet_encoder::TCPNetworkEncoder,
        server::{
            config::{SAcknowledgeFinishConfig, SClientInformationConfig, SKnownPacks},
            handshake::SHandShake,
            login::{SLoginAcknowledged, SLoginStart},
            play::{
                SChunkBatch, SClientTickEnd, SConfirmTeleport, SKeepAlive, SPlayPong,
                SPlayerLoaded, SPlayerPosition,
            },
        },
    },
    packet::MultiVersionJavaPacket,
    ser::NetworkWriteExt,
};
use pumpkin_util::math::vector3::Vector3;
use tokio::{
    io::{BufReader, BufWriter},
    net::{
        TcpStream,
        tcp::{OwnedReadHalf, OwnedWriteHalf},
    },
    sync::mpsc,
    time::{MissedTickBehavior, interval, sleep},
};
use uuid::Uuid;

/// Height bots fly at, above any terrain so movement never collides.
const FLY_Y: f64 = 200.0;
/// Default blocks per tick (10 blocks/s), below Paper's "moved too quickly" limit.
const DEFAULT_SPEED: f64 = 0.5;
/// Chunks all bots received, summed from the server's chunk batch sizes.
static CHUNKS_RECEIVED: AtomicU64 = AtomicU64::new(0);
const VIEW_DISTANCE: i8 = 10;
/// Chunks per tick the bot asks for after each batch, like a fast vanilla client.
const CHUNKS_PER_TICK: f32 = 25.0;

type Decoder = TCPNetworkDecoder<BufReader<OwnedReadHalf>>;
type Encoder = TCPNetworkEncoder<BufWriter<OwnedWriteHalf>>;

/// Something the reader saw that the writer has to answer.
enum Reply {
    KeepAlive(i64),
    Pong(i32),
    ChunkBatch,
    Teleport(CPlayerPosition),
}

#[expect(clippy::print_stderr, reason = "CLI tool reports progress on stderr")]
#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let (Some(addr), Some(bots)) = (
        args.first().and_then(|a| a.parse::<SocketAddr>().ok()),
        args.get(1).and_then(|a| a.parse::<u32>().ok()),
    ) else {
        eprintln!("usage: rustgot-loadbot <addr> <bots> [fly-seconds] [blocks-per-tick]");
        return ExitCode::FAILURE;
    };
    let fly_secs = args
        .get(2)
        .and_then(|a| a.parse::<u64>().ok())
        .unwrap_or(60);
    let speed = args
        .get(3)
        .and_then(|a| a.parse::<f64>().ok())
        .unwrap_or(DEFAULT_SPEED);

    for i in 0..bots {
        let angle = TAU * f64::from(i) / f64::from(bots);
        tokio::spawn(async move {
            let name = format!("bot{i}");
            if let Err(err) = run_bot(addr, &name, angle, fly_secs, speed).await {
                eprintln!("{name}: {err}");
            }
        });
        // Stagger joins like real players instead of one burst.
        sleep(Duration::from_millis(100)).await;
    }
    eprintln!("spawned {bots} bots");
    let mut report = interval(Duration::from_secs(10));
    report.tick().await;
    loop {
        report.tick().await;
        eprintln!(
            "chunks received: {}",
            CHUNKS_RECEIVED.load(Ordering::Relaxed)
        );
    }
}

async fn run_bot(
    addr: SocketAddr,
    name: &str,
    angle: f64,
    fly_secs: u64,
    speed: f64,
) -> Result<(), String> {
    let stream = TcpStream::connect(addr)
        .await
        .map_err(|err| format!("connect failed: {err}"))?;
    let _ = stream.set_nodelay(true);
    let (read_half, write_half) = stream.into_split();
    let mut reader = TCPNetworkDecoder::new(BufReader::new(read_half));
    let mut writer = TCPNetworkEncoder::new(BufWriter::new(write_half));

    let mut position = join(&mut reader, &mut writer, addr.port(), name).await?;

    let (tx, mut rx) = mpsc::unbounded_channel();
    let reader_task = tokio::spawn(read_loop(reader, tx));

    let mut tick = interval(Duration::from_millis(50));
    tick.set_missed_tick_behavior(MissedTickBehavior::Skip);
    let fly_ticks = fly_secs * 20;
    let mut flown = 0;
    let (dx, dz) = (angle.cos() * speed, angle.sin() * speed);

    loop {
        tokio::select! {
            reply = rx.recv() => {
                let Some(reply) = reply else { break };
                match reply {
                    Reply::KeepAlive(id) => send(&mut writer, &SKeepAlive { keep_alive_id: id }).await?,
                    Reply::Pong(id) => send(&mut writer, &SPlayPong { id }).await?,
                    Reply::ChunkBatch => send(&mut writer, &SChunkBatch { chunks_per_tick: CHUNKS_PER_TICK }).await?,
                    Reply::Teleport(tp) => {
                        position = tp.position;
                        confirm_teleport(&mut writer, &tp).await?;
                    }
                }
            }
            _ = tick.tick() => {
                if position.y < FLY_Y {
                    position.y = (position.y + DEFAULT_SPEED).min(FLY_Y);
                } else if flown < fly_ticks {
                    position.x += dx;
                    position.z += dz;
                    flown += 1;
                }
                send(&mut writer, &SPlayerPosition { position, collision: 0 }).await?;
                // Since 26.3 the server allows one position per client tick and only resets
                // that on tick end, like the vanilla client sends every tick.
                send(&mut writer, &SClientTickEnd).await?;
            }
        }
    }
    reader_task.abort();
    Err("disconnected by server".into())
}

async fn read_loop(mut reader: Decoder, tx: mpsc::UnboundedSender<Reply>) {
    let keep_alive = CKeepAlive::to_id(CURRENT_MC_VERSION);
    let ping = CPlayPing::to_id(CURRENT_MC_VERSION);
    let batch_end = CChunkBatchEnd::to_id(CURRENT_MC_VERSION);
    let teleport = CPlayerPosition::to_id(CURRENT_MC_VERSION);

    while let Ok(packet) = reader.get_raw_packet().await {
        let mut payload = &packet.payload[..];
        let reply = if packet.id == keep_alive {
            CKeepAlive::read(&mut payload, &CURRENT_MC_VERSION)
                .ok()
                .map(|p| Reply::KeepAlive(p.keep_alive_id))
        } else if packet.id == ping {
            // `CPlayPing` has no reader; its payload is a single big-endian i32.
            payload
                .get(..4)
                .and_then(|id| id.try_into().ok())
                .map(|id| Reply::Pong(i32::from_be_bytes(id)))
        } else if packet.id == batch_end {
            CHUNKS_RECEIVED.fetch_add(read_var_int(payload), Ordering::Relaxed);
            Some(Reply::ChunkBatch)
        } else if packet.id == teleport {
            CPlayerPosition::read(&mut payload, &CURRENT_MC_VERSION)
                .ok()
                .map(Reply::Teleport)
        } else {
            None
        };
        if let Some(reply) = reply
            && tx.send(reply).is_err()
        {
            return;
        }
    }
}

/// A protocol `VarInt` at the start of `bytes`, or 0 if it is malformed.
fn read_var_int(bytes: &[u8]) -> u64 {
    let mut value = 0u64;
    for (i, byte) in bytes.iter().take(5).enumerate() {
        value |= u64::from(byte & 0x7F) << (7 * i);
        if byte & 0x80 == 0 {
            return value;
        }
    }
    0
}

/// Logs in and finishes configuration; returns the spawn position.
async fn join(
    reader: &mut Decoder,
    writer: &mut Encoder,
    port: u16,
    name: &str,
) -> Result<Vector3<f64>, String> {
    send(
        writer,
        &SHandShake {
            protocol_version: VarInt(CURRENT_MC_VERSION.protocol_version()),
            server_address: "127.0.0.1".into(),
            server_port: port,
            next_state: ConnectionState::Login,
        },
    )
    .await?;
    send(
        writer,
        &SLoginStart {
            name: name.into(),
            uuid: Uuid::new_v4(),
        },
    )
    .await?;

    let compression_id = CSetCompression::to_id(CURRENT_MC_VERSION);
    let login_success_id = CLoginSuccess::to_id(CURRENT_MC_VERSION);
    loop {
        let packet = next(reader).await?;
        if packet.id == compression_id {
            let threshold = CSetCompression::read(&mut &packet.payload[..], &CURRENT_MC_VERSION)
                .map_err(|err| format!("bad compression packet: {err}"))?
                .threshold
                .0;
            if let Ok(threshold) = usize::try_from(threshold) {
                reader.set_compression(threshold);
                writer.set_compression((threshold, 6));
            }
        } else if packet.id == login_success_id {
            break;
        }
    }
    send(writer, &SLoginAcknowledged).await?;

    send_client_information(writer).await?;

    let known_packs_id = CKnownPacks::to_id(CURRENT_MC_VERSION);
    let finish_config_id = CFinishConfig::to_id(CURRENT_MC_VERSION);
    loop {
        let packet = next(reader).await?;
        if packet.id == known_packs_id {
            send(
                writer,
                &SKnownPacks {
                    known_packs: Vec::new(),
                },
            )
            .await?;
        } else if packet.id == finish_config_id {
            send(writer, &SAcknowledgeFinishConfig).await?;
            break;
        }
    }

    let play_login_id = CLogin::to_id(CURRENT_MC_VERSION);
    let position_id = CPlayerPosition::to_id(CURRENT_MC_VERSION);
    let keep_alive_id = CKeepAlive::to_id(CURRENT_MC_VERSION);
    let mut logged_in = false;
    loop {
        let packet = next(reader).await?;
        if packet.id == play_login_id {
            logged_in = true;
        } else if packet.id == keep_alive_id {
            let ka = CKeepAlive::read(&mut &packet.payload[..], &CURRENT_MC_VERSION)
                .map_err(|err| format!("bad keep-alive: {err}"))?;
            send(
                writer,
                &SKeepAlive {
                    keep_alive_id: ka.keep_alive_id,
                },
            )
            .await?;
        } else if logged_in && packet.id == position_id {
            let tp = CPlayerPosition::read(&mut &packet.payload[..], &CURRENT_MC_VERSION)
                .map_err(|err| format!("bad position packet: {err}"))?;
            confirm_teleport(writer, &tp).await?;
            send(writer, &SPlayerLoaded).await?;
            return Ok(tp.position);
        }
    }
}

/// Pumpkin's `SClientInformationConfig` writer lacks the `particle_status` field that vanilla
/// has required since 1.21.2, so Paper/Purpur reject it. Write the vanilla layout by hand.
async fn send_client_information(writer: &mut Encoder) -> Result<(), String> {
    let mut buf = Vec::new();
    let write = |buf: &mut Vec<u8>| -> Result<(), pumpkin_protocol::ser::WritingError> {
        buf.write_var_int(&VarInt(SClientInformationConfig::to_id(CURRENT_MC_VERSION)))?;
        buf.write_string("en_us")?;
        buf.write_i8(VIEW_DISTANCE)?;
        buf.write_var_int(&VarInt(0))?; // chat mode: enabled
        buf.write_bool(true)?; // chat colors
        buf.write_u8(0x7F)?; // all skin parts
        buf.write_var_int(&VarInt(1))?; // main hand: right
        buf.write_bool(false)?; // text filtering
        buf.write_bool(true)?; // allow server listing
        buf.write_var_int(&VarInt(0))?; // particle status: all
        Ok(())
    };
    write(&mut buf).map_err(|err| format!("failed to serialize client information: {err:?}"))?;
    writer
        .write_packet(Bytes::from(buf))
        .await
        .map_err(|err| format!("failed to write packet: {err}"))?;
    writer
        .flush()
        .await
        .map_err(|err| format!("failed to flush: {err}"))
}

async fn confirm_teleport(writer: &mut Encoder, tp: &CPlayerPosition) -> Result<(), String> {
    send(
        writer,
        &SConfirmTeleport {
            teleport_id: tp.teleport_id,
            position: tp.position,
            yaw: tp.yaw,
            pitch: tp.pitch,
        },
    )
    .await
}

async fn next(reader: &mut Decoder) -> Result<pumpkin_protocol::RawPacket, String> {
    tokio::time::timeout(Duration::from_secs(60), reader.get_raw_packet())
        .await
        .map_err(|_| "timed out waiting for the server".to_string())?
        .map_err(|err| format!("decode error: {err}"))
}

async fn send<P: ClientPacket + MultiVersionJavaPacket>(
    writer: &mut Encoder,
    packet: &P,
) -> Result<(), String> {
    let id = P::to_id(CURRENT_MC_VERSION);
    let mut buf = Vec::new();
    buf.write_var_int(&VarInt(id))
        .map_err(|err| format!("failed to write packet id {id}: {err:?}"))?;
    packet
        .write_packet_data(&mut buf, &CURRENT_MC_VERSION)
        .map_err(|err| format!("failed to serialize packet {id}: {err:?}"))?;
    writer
        .write_packet(Bytes::from(buf))
        .await
        .map_err(|err| format!("failed to write packet: {err}"))?;
    writer
        .flush()
        .await
        .map_err(|err| format!("failed to flush: {err}"))
}
