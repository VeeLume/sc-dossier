//! The game's local telemetry stream (`sc.game.telemetry.v1.GameTelemetryService`)
//! and the `/showlocation` parser.
//!
//! The game hosts the stream itself, on `127.0.0.1:2942`, plaintext, no auth —
//! but only when started with `GRPC_CLIENT_SERVER_ENABLED=true` in the
//! environment of the launcher. A closed port means the game isn't running or
//! the flag isn't set; the two look alike from here (`Game.log` tells them
//! apart). Several clients can read at once.
//!
//! Each message is a full [`Frame`] of the sections that apply right now, about
//! 13–16 per second: `vehicle` + `weapons` + `actor` when seated, `actor` alone
//! on foot. Measured on LIVE 4.10.2.
//!
//! ```no_run
//! # async fn run() -> sc_dossier::Result<()> {
//! use sc_dossier::telemetry;
//! let mut stream = telemetry::connect(telemetry::DEFAULT_ADDR).await?;
//! while let Some(frame) = stream.next().await? {
//!     if let Some(v) = &frame.vehicle {
//!         println!("{} {:?}", v.class_name, v.flight.as_ref().map(|f| f.instruments.speed));
//!     }
//! }
//! # Ok(()) }
//! ```
//!
//! Without an async runtime, [`spawn`] runs the connection on its own thread
//! and reconnects by itself.

use std::ops::ControlFlow;
use std::thread::JoinHandle;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use tonic::transport::Endpoint;

use crate::error::{Error, Result};
use crate::proto::telemetry as pb;

/// Where the game listens unless `grpc-client-server-address` says otherwise.
pub const DEFAULT_ADDR: &str = "http://127.0.0.1:2942";

const CONNECT_TIMEOUT: Duration = Duration::from_secs(2);
const RETRY_MIN: Duration = Duration::from_secs(1);
const RETRY_MAX: Duration = Duration::from_secs(5);
/// No message for this long while connected reports [`Status::Idle`].
const IDLE_AFTER: Duration = Duration::from_secs(5);

// ── Connection ──────────────────────────────────────────────────────────────

/// An open telemetry stream.
pub struct TelemetryStream {
    inner: tonic::Streaming<pb::TelemetryStreamResponse>,
}

/// Connect to the game's telemetry server at `addr` (e.g. [`DEFAULT_ADDR`]) and
/// open the stream. Fails fast ([`Error::Transport`]) when nothing listens.
pub async fn connect(addr: &str) -> Result<TelemetryStream> {
    let channel = Endpoint::from_shared(addr.to_string())?
        .connect_timeout(CONNECT_TIMEOUT)
        .connect()
        .await?;
    let mut client = pb::game_telemetry_service_client::GameTelemetryServiceClient::new(channel);
    let inner = client
        .telemetry_stream(pb::TelemetryStreamRequest {})
        .await?
        .into_inner();
    Ok(TelemetryStream { inner })
}

impl TelemetryStream {
    /// The next frame; `Ok(None)` when the game ends the stream (it quit).
    pub async fn next(&mut self) -> Result<Option<Frame>> {
        Ok(self.inner.message().await?.map(Frame::from))
    }
}

/// What [`spawn`] reports.
#[derive(Debug)]
#[non_exhaustive]
pub enum Status {
    /// The stream is open.
    Connected,
    /// A frame arrived.
    Frame(Box<Frame>),
    /// Connected, but no frame for 5 s (loading, menus). Repeats every 5 s.
    Idle,
    /// Not connected: the attempt failed or the stream ended. Repeats on every
    /// retry (every 1–5 s) until the game is back.
    Unavailable(Error),
}

/// Read `addr` on a new thread, reconnecting forever, and hand every
/// [`Status`] to `on_status`. Return [`ControlFlow::Break`] from it to stop;
/// it is called at least every 5 s, so a stop takes effect within that.
pub fn spawn<F>(addr: impl Into<String>, on_status: F) -> std::io::Result<JoinHandle<()>>
where
    F: FnMut(Status) -> ControlFlow<()> + Send + 'static,
{
    let addr = addr.into();
    std::thread::Builder::new()
        .name("sc-telemetry".into())
        .spawn(move || {
            let rt = match tokio::runtime::Builder::new_current_thread()
                .enable_all()
                .build()
            {
                Ok(rt) => rt,
                Err(_) => return,
            };
            rt.block_on(run(addr, on_status));
        })
}

async fn run<F>(addr: String, mut on_status: F)
where
    F: FnMut(Status) -> ControlFlow<()>,
{
    let mut backoff = RETRY_MIN;
    loop {
        let reason = match connect(&addr).await {
            Ok(mut stream) => {
                backoff = RETRY_MIN;
                if on_status(Status::Connected).is_break() {
                    return;
                }
                loop {
                    let status = match tokio::time::timeout(IDLE_AFTER, stream.next()).await {
                        Err(_) => Status::Idle,
                        Ok(Ok(Some(frame))) => Status::Frame(Box::new(frame)),
                        Ok(Ok(None)) => break Error::Invalid("the game ended the stream".into()),
                        Ok(Err(e)) => break e,
                    };
                    if on_status(status).is_break() {
                        return;
                    }
                }
            }
            Err(e) => e,
        };
        if on_status(Status::Unavailable(reason)).is_break() {
            return;
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(RETRY_MAX);
    }
}

// ── /showlocation ───────────────────────────────────────────────────────────

/// Parse the clipboard line `/showlocation` writes:
/// `Coordinates: x:<m> y:<m> z:<m>`, system-global metres (the star at the
/// origin, bodies at their game-file positions, so a planet's spin moves the
/// surface under you).
///
/// `None` for anything else. Clipboard text is arbitrary, so nothing rejected
/// is reported or kept.
pub fn parse_showlocation(text: &str) -> Option<[f64; 3]> {
    if text.len() > 128 {
        return None;
    }
    let mut parts = text.trim_end().strip_prefix("Coordinates: ")?.split(' ');
    let x = coord(parts.next()?, "x:")?;
    let y = coord(parts.next()?, "y:")?;
    let z = coord(parts.next()?, "z:")?;
    parts.next().is_none().then_some([x, y, z])
}

fn coord(token: &str, prefix: &str) -> Option<f64> {
    let v: f64 = token.strip_prefix(prefix)?.parse().ok()?;
    v.is_finite().then_some(v)
}

// ── Frame model ─────────────────────────────────────────────────────────────

/// One telemetry message: the current state of every section that applies.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Frame {
    /// The vehicle the player is seated in.
    pub vehicle: Option<Vehicle>,
    /// That vehicle's weapons, from a seat that has them.
    pub weapons: Option<Weapons>,
    /// The player character. Sent seated and on foot.
    pub actor: Option<Actor>,
    /// Only in a turret seat.
    pub turret: Option<Turret>,
    /// Events since the last frame. Incomplete: the first seat entry of a
    /// session produced none.
    pub events: Vec<Event>,
}

/// The vehicle the player is in.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Vehicle {
    /// Entity name with an id suffix (`KRIG_S65_Stingray_200000471831`).
    pub name: String,
    /// The clean class key (`KRIG_S65_Stingray`); use this to look the ship up.
    pub class_name: String,
    /// Manufacturer as sent.
    pub manufacturer: String,
    /// Spaceship, ground, …
    pub kind: VehicleKind,
    /// External lights on.
    pub lights: bool,
    /// Door state. Not sent on LIVE 4.10.2.
    pub doors: Option<Doors>,
    /// The ten master annunciator lights.
    pub annunciators: Option<Annunciators>,
    /// Flight state, settings and instruments.
    pub flight: Option<Flight>,
    /// Power toggles.
    pub power: Option<Power>,
}

/// Vehicle class.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum VehicleKind {
    /// Not sent.
    #[default]
    Unknown,
    /// Ship.
    Spaceship,
    /// Ground vehicle.
    Ground,
    /// Grav-lev bike.
    Gravlev,
    /// Boat.
    Boat,
}

/// Door state of the vehicle.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Doors {
    /// `Some(true)` open, `Some(false)` closed, `None` not sent.
    pub open: Option<bool>,
    /// `Some(true)` locked, `Some(false)` unlocked, `None` not sent.
    pub locked: Option<bool>,
}

/// One annunciator light.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub enum Light {
    /// Not sent.
    #[default]
    Unknown,
    /// Nominal.
    Nominal,
    /// Off.
    Off,
    /// Minor fault, flashing.
    MinorFlashing,
    /// Minor fault, steady.
    MinorOn,
    /// Major fault, flashing.
    MajorFlashing,
    /// Major fault, steady.
    MajorOn,
}

/// The master annunciator panel.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub struct Annunciators {
    pub power: Light,
    pub weapons: Light,
    pub thrusters: Light,
    pub shields: Light,
    pub coolers: Light,
    pub quantum_drive: Light,
    pub radar: Light,
    pub proximity: Light,
    pub hit: Light,
    pub missile: Light,
}

/// Power toggles.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub struct Power {
    pub powerplants: bool,
    pub engines: bool,
    pub weapons: bool,
    pub coolers: bool,
    pub life_support: bool,
    /// Always false on LIVE 4.10.2.
    pub gravity_generators: bool,
    pub sensors: bool,
    pub shield_generators: bool,
    pub shield_emitters: bool,
}

/// Flight block of a vehicle.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Flight {
    /// On the ground.
    pub landed: bool,
    /// Landing gear; `None` if the vehicle has none.
    pub gear: Option<Deployment>,
    /// VTOL; `None` if the vehicle has none.
    pub vtol: Option<Deployment>,
    /// Transformation (e.g. wings); `None` if the vehicle has none.
    pub transformation: Option<Deployment>,
    /// IFCS and HUD settings.
    pub settings: FlightSettings,
    /// Instruments.
    pub instruments: Instruments,
}

/// A deployable part.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Deployment {
    #[default]
    Unknown,
    Retracted,
    Deploying,
    Deployed,
    Retracting,
}

/// A setting: `enabled` is the switch, `active` whether it is in effect right
/// now (G-Safe stays enabled but drops out while boosting).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Setting {
    /// Switched on.
    pub enabled: bool,
    /// In effect.
    pub active: bool,
}

/// IFCS and HUD settings. `coupled`, `cruise`, `speed_limiter` and the
/// proximity assists never changed in testing, so their behaviour is unproven.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub struct FlightSettings {
    pub coupled: Setting,
    pub esp: Setting,
    pub gravity_compensation: Setting,
    pub g_safe: Setting,
    pub cruise: Setting,
    pub speed_limiter: Setting,
    pub proximity_assist: Setting,
    pub proximity_assist_landing: Setting,
    pub stability: Setting,
    pub core: Setting,
    pub wind_compensation: Setting,
    pub command_behaviour: Setting,
    pub spacebrake_enables_boost: bool,
    pub g_safe_disabled_by_boost: bool,
    pub advanced_hud_scm: bool,
    pub advanced_hud_nav: bool,
    pub advanced_hud_labels: bool,
    pub course_prediction: bool,
    pub light_amplification: bool,
}

/// Flight instruments. Units measured on LIVE 4.10.2.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Instruments {
    /// m/s, relative to the nearest body (reads ~0 parked on a spinning moon).
    /// **Wrong in quantum travel**: reads 0, then freezes at a stale value
    /// until the drop out; the stream has no quantum flag to detect it.
    pub speed: f32,
    /// 0–1.
    pub hydrogen_fuel: f32,
    /// 0–1 by the schema; never sent on LIVE 4.10.2, so reads 0.
    pub quantum_fuel: f32,
    /// Current load and current limit, in g.
    pub g_meter: Option<Gauge>,
    /// Boost.
    pub boost: Boost,
    /// m above the body's datum. `None` outside a body's atmosphere shell,
    /// where the game sends no reference.
    pub altitude: Option<f32>,
    /// m above the ground, from the ship's origin (not the gear). True ground
    /// distance only below about 500 m; higher it repeats `altitude`.
    pub radar_altitude: Option<f32>,
    /// m/s, positive up.
    pub vertical_speed: f32,
    /// kg/m³ by inference (1.2 in a hangar).
    pub atmospheric_density: f32,
    /// Degrees 0–360, the nose. `None` without a planet reference, where the
    /// game holds the last value instead of updating it.
    pub heading: Option<f32>,
}

/// Boost.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Boost {
    /// Boosting.
    pub engaged: bool,
    /// 0–1.
    pub output: f32,
    /// 0–1.
    pub reserves: f32,
    /// Reserves critical.
    pub critical: bool,
}

/// A value with its maximum.
#[derive(Debug, Clone, Copy, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Gauge {
    /// Current value.
    pub current: f32,
    /// Maximum (for the g-meter: the current limit).
    pub max: f32,
}

/// Weapons of the current seat.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub struct Weapons {
    pub gimbals_locked: bool,
    pub magnified_ui: bool,
    pub precision_lines: bool,
    pub staggered_fire: bool,
    pub fallback_convergence_distance: bool,
    pub lag_pips: bool,
    pub single_pips: bool,
    pub fading_pips: bool,
    /// Active weapon preset.
    pub preset_index: u32,
    /// Empty on LIVE 4.10.2.
    pub preset_name: String,
    pub weapons: Vec<Weapon>,
    pub countermeasures: Vec<Countermeasure>,
}

/// One weapon.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Weapon {
    /// Entity name with an id suffix.
    pub name: String,
    /// Powered.
    pub powered: bool,
    /// Ready to fire.
    pub ready: bool,
    /// Part of the active preset.
    pub in_preset: bool,
}

/// A countermeasure stock.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Countermeasure {
    /// Decoy or noise.
    pub kind: CountermeasureKind,
    /// Remaining.
    pub amount: u32,
}

/// Countermeasure type.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum CountermeasureKind {
    #[default]
    Unknown,
    Decoy,
    Noise,
}

/// The player character.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Actor {
    /// 0–100.
    pub health: Gauge,
    /// 0–1.
    pub oxygen: Gauge,
    /// 0–100.
    pub hunger: Gauge,
    /// 0–100.
    pub thirst: Gauge,
    /// °C (36.5 normal).
    pub body_temperature: Gauge,
    /// Beats per minute.
    pub heart_rate: f32,
    /// Local gravity in g.
    pub gravity: f32,
    /// Ambient pressure as sent (0.012 on Lyria's surface).
    pub pressure: f32,
    /// Intoxication level.
    pub intoxication: i32,
    /// °C; -273.15 in vacuum.
    pub external_temperature: f32,
    /// Ambient radiation rate.
    pub radiation: f32,
    /// Breathable atmosphere.
    pub breathable: bool,
    /// On foot, EVA or seated.
    pub locomotion: Locomotion,
    /// Seat role.
    pub crew_role: CrewRole,
    /// Seat entity name, with an id suffix; empty on foot.
    pub seat_name: String,
    /// aUEC.
    pub uec_balance: i64,
}

/// How the actor moves.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Locomotion {
    #[default]
    Unknown,
    OnFoot,
    Eva,
    Seated,
}

/// The actor's seat role.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum CrewRole {
    #[default]
    None,
    Pilot,
    Gunner,
    Crew,
}

/// Turret seat state.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
pub struct Turret {
    /// Night vision in the turret.
    pub light_amplification: bool,
}

/// A game event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[non_exhaustive]
#[allow(missing_docs)]
pub enum Event {
    Spawn,
    Dead,
    EnterSeat,
    LeaveSeat,
    Respawn,
    Downed,
    /// A type this version doesn't know.
    Other(i32),
}

impl Frame {
    /// Decode one raw stream message (the gRPC payload, without the 5-byte
    /// length prefix), e.g. from a recorded session.
    pub fn decode(bytes: &[u8]) -> Result<Frame> {
        use prost::Message;
        pb::TelemetryStreamResponse::decode(bytes)
            .map(Frame::from)
            .map_err(|e| Error::Invalid(format!("telemetry message: {e}")))
    }
}

// ── Conversion ──────────────────────────────────────────────────────────────

/// The game sends `FLT_MAX` (or garbage) when there is no reference body.
fn reference(v: f32) -> Option<f32> {
    (v.is_finite() && v.abs() < 1.0e30).then_some(v)
}

fn gauge(g: Option<pb::Gauge>) -> Gauge {
    g.map(|g| Gauge {
        current: g.current,
        max: g.max,
    })
    .unwrap_or_default()
}

fn setting(a: Option<pb::Availability>) -> Setting {
    a.map(|a| Setting {
        enabled: a.enabled,
        active: a.active,
    })
    .unwrap_or_default()
}

fn deployment(v: Option<i32>) -> Option<Deployment> {
    use pb::VehicleComponentDeploymentState as S;
    Some(match S::try_from(v?).ok()? {
        S::Unspecified => return None,
        S::Retracted => Deployment::Retracted,
        S::Deploying => Deployment::Deploying,
        S::Deployed => Deployment::Deployed,
        S::Retracting => Deployment::Retracting,
    })
}

fn light(v: i32) -> Light {
    use pb::vehicle_annunciator::AnnunciatorState as S;
    match S::try_from(v) {
        Ok(S::Nominal) => Light::Nominal,
        Ok(S::Off) => Light::Off,
        Ok(S::MinorFlashing) => Light::MinorFlashing,
        Ok(S::MinorOn) => Light::MinorOn,
        Ok(S::MajorFlashing) => Light::MajorFlashing,
        Ok(S::MajorOn) => Light::MajorOn,
        _ => Light::Unknown,
    }
}

impl From<pb::TelemetryStreamResponse> for Frame {
    fn from(m: pb::TelemetryStreamResponse) -> Self {
        Frame {
            vehicle: m.vehicle.and_then(|v| v.state).map(Vehicle::from),
            weapons: m.weapons.map(Weapons::from),
            actor: m.actor.map(Actor::from),
            turret: m.turret.map(|t| Turret {
                light_amplification: t.light_amplification,
            }),
            events: m
                .events
                .map(|e| {
                    e.events
                        .into_iter()
                        .map(|e| Event::from(e.r#type))
                        .collect()
                })
                .unwrap_or_default(),
        }
    }
}

impl From<pb::VehicleState> for Vehicle {
    fn from(s: pb::VehicleState) -> Self {
        use pb::VehicleType as T;
        let kind = match T::try_from(s.r#type) {
            Ok(T::Spaceship) => VehicleKind::Spaceship,
            Ok(T::Ground) => VehicleKind::Ground,
            Ok(T::Gravlev) => VehicleKind::Gravlev,
            Ok(T::Boat) => VehicleKind::Boat,
            _ => VehicleKind::Unknown,
        };
        Vehicle {
            name: s.name,
            class_name: s.class_name,
            manufacturer: s.manufacturer,
            kind,
            lights: s.lights_enabled,
            doors: s.doors.map(|d| {
                use pb::vehicle_door_state::{DoorLockState as L, DoorOpenState as O};
                Doors {
                    open: match O::try_from(d.open_state) {
                        Ok(O::Opened) => Some(true),
                        Ok(O::Closed) => Some(false),
                        _ => None,
                    },
                    locked: match L::try_from(d.lock_state) {
                        Ok(L::Locked) => Some(true),
                        Ok(L::Unlocked) => Some(false),
                        _ => None,
                    },
                }
            }),
            annunciators: s.annunciators.map(|a| Annunciators {
                power: light(a.pwr),
                weapons: light(a.wpn),
                thrusters: light(a.thr),
                shields: light(a.shld),
                coolers: light(a.cool),
                quantum_drive: light(a.qdrv),
                radar: light(a.radr),
                proximity: light(a.prox),
                hit: light(a.hit),
                missile: light(a.misl),
            }),
            flight: s.flight.map(Flight::from),
            power: s.power.map(|p| Power {
                powerplants: p.powerplants,
                engines: p.engines,
                weapons: p.weapons,
                coolers: p.coolers,
                life_support: p.life_support,
                gravity_generators: p.gravity_generators,
                sensors: p.sensors,
                shield_generators: p.shield_generators,
                shield_emitters: p.shield_emitters,
            }),
        }
    }
}

impl From<pb::VehicleFlight> for Flight {
    fn from(f: pb::VehicleFlight) -> Self {
        let state = f.state.unwrap_or_default();
        let s = f.settings.unwrap_or_default();
        let i = f.instruments.unwrap_or_default();
        let altitude = reference(i.altitude);
        let boost = i.boost.unwrap_or_default();
        Flight {
            landed: state.is_landed,
            gear: deployment(state.gear),
            vtol: deployment(state.vtol),
            transformation: deployment(state.transformation),
            settings: FlightSettings {
                coupled: setting(s.ifcs_coupled),
                esp: setting(s.ifcs_esp),
                gravity_compensation: setting(s.ifcs_gravity_compensation),
                g_safe: setting(s.ifcs_g_safe),
                cruise: setting(s.ifcs_cruise_mode),
                speed_limiter: setting(s.ifcs_speed_limiter),
                proximity_assist: setting(s.ifcs_proximity_assist),
                proximity_assist_landing: setting(s.ifcs_proximity_assist_landing),
                stability: setting(s.ifcs_stability),
                core: setting(s.ifcs_core),
                wind_compensation: setting(s.ifcs_wind_compensation),
                command_behaviour: setting(s.ifcs_command_behaviour),
                spacebrake_enables_boost: s.ifcs_spacebreak_enables_boost,
                g_safe_disabled_by_boost: s.ifcs_g_safe_disabled_by_boost,
                advanced_hud_scm: s.advanced_hud_enabled_scm,
                advanced_hud_nav: s.advanced_hud_enabled_nav,
                advanced_hud_labels: s.advanced_hud_labels,
                course_prediction: s.course_prediction,
                light_amplification: s.flight_light_amplification,
            },
            instruments: Instruments {
                speed: i.speed,
                hydrogen_fuel: i.hydrogen_fuel,
                quantum_fuel: i.quantum_fuel,
                g_meter: i.g_meter.map(|g| Gauge {
                    current: g.current,
                    max: g.max,
                }),
                boost: Boost {
                    engaged: boost.engaged,
                    output: boost.output,
                    reserves: boost.reserves,
                    critical: boost.critical,
                },
                altitude,
                radar_altitude: reference(i.radar_altitude),
                vertical_speed: i.vertical_speed_indicator,
                atmospheric_density: i.atmospheric_density,
                // Without a planet reference the game holds the last heading.
                heading: altitude.map(|_| i.heading),
            },
        }
    }
}

impl From<pb::WeaponsFrame> for Weapons {
    fn from(w: pb::WeaponsFrame) -> Self {
        use pb::countermeasures::CountermeasureType as C;
        let s = w.settings.unwrap_or_default();
        Weapons {
            gimbals_locked: s.gimbals_locked,
            magnified_ui: s.magnified_ui,
            precision_lines: s.precision_lines,
            staggered_fire: s.staggered_fire,
            fallback_convergence_distance: s.fallback_convergence_distance,
            lag_pips: s.use_lag_pips,
            single_pips: s.use_single_pips,
            fading_pips: s.use_fading_pips,
            preset_index: w.preset_index,
            preset_name: w.preset_name,
            weapons: w
                .weapons
                .into_iter()
                .map(|x| Weapon {
                    name: x.name,
                    powered: x.is_powered,
                    ready: x.ready_to_fire,
                    in_preset: x.contained_in_preset,
                })
                .collect(),
            countermeasures: w
                .countermeasures
                .into_iter()
                .map(|c| Countermeasure {
                    kind: match C::try_from(c.r#type) {
                        Ok(C::Decoy) => CountermeasureKind::Decoy,
                        Ok(C::Noise) => CountermeasureKind::Noise,
                        _ => CountermeasureKind::Unknown,
                    },
                    amount: c.amount,
                })
                .collect(),
        }
    }
}

impl From<pb::ActorFrame> for Actor {
    fn from(a: pb::ActorFrame) -> Self {
        use pb::{ActorCrewRole as R, ActorLocomotionState as L};
        let v = a.vitals.unwrap_or_default();
        let s = a.state.unwrap_or_default();
        let e = a.environment.unwrap_or_default();
        Actor {
            health: gauge(v.health),
            oxygen: gauge(v.oxygen),
            hunger: gauge(v.hunger),
            thirst: gauge(v.thirst),
            body_temperature: gauge(v.body_temperature),
            heart_rate: v.heart_rate,
            gravity: v.gravity,
            pressure: v.pressure,
            intoxication: v.intoxication_level,
            external_temperature: e.external_temperature,
            radiation: e.ambient_radiation_rate,
            breathable: e.breathable_atmosphere,
            locomotion: match L::try_from(s.locomotion) {
                Ok(L::OnFoot) => Locomotion::OnFoot,
                Ok(L::Eva) => Locomotion::Eva,
                Ok(L::Seated) => Locomotion::Seated,
                _ => Locomotion::Unknown,
            },
            crew_role: match R::try_from(s.crew_role) {
                Ok(R::Pilot) => CrewRole::Pilot,
                Ok(R::Gunner) => CrewRole::Gunner,
                Ok(R::Crew) => CrewRole::Crew,
                _ => CrewRole::None,
            },
            seat_name: s.seat_name,
            uec_balance: s.uec_balance,
        }
    }
}

impl From<i32> for Event {
    fn from(v: i32) -> Self {
        use pb::game_event::GameEventType as T;
        match T::try_from(v) {
            Ok(T::ActorSpawn) => Event::Spawn,
            Ok(T::ActorDead) => Event::Dead,
            Ok(T::ActorEnterSeat) => Event::EnterSeat,
            Ok(T::ActorLeaveSeat) => Event::LeaveSeat,
            Ok(T::ActorRespawn) => Event::Respawn,
            Ok(T::ActorDowned) => Event::Downed,
            _ => Event::Other(v),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use prost::Message;

    #[test]
    #[allow(clippy::excessive_precision)]
    fn showlocation_real_line() {
        let s = "Coordinates: x:18703236341.617867 y:-22121750258.425072 z:-565.772854";
        assert_eq!(
            parse_showlocation(s),
            Some([18703236341.617867, -22121750258.425072, -565.772854])
        );
        assert!(parse_showlocation(&format!("{s}\r\n")).is_some());
    }

    #[test]
    fn showlocation_rejects_everything_else() {
        for bad in [
            "",
            "hello",
            "Coordinates: x:1 y:2",
            "Coordinates: x:1 y:2 z:3 w:4",
            "Coordinates: y:1 x:2 z:3",
            "Coordinates: x:a y:2 z:3",
            "Coordinates: x:inf y:2 z:3",
            "Coordinates: x:NaN y:2 z:3",
            " Coordinates: x:1 y:2 z:3",
            "password Coordinates: x:1 y:2 z:3",
        ] {
            assert_eq!(parse_showlocation(bad), None, "{bad:?}");
        }
        assert_eq!(parse_showlocation(&"9".repeat(500)), None);
    }

    fn instruments(altitude: f32, heading: f32) -> Instruments {
        let msg = pb::TelemetryStreamResponse {
            vehicle: Some(pb::VehicleFrame {
                state: Some(pb::VehicleState {
                    class_name: "KRIG_S65_Stingray".into(),
                    flight: Some(pb::VehicleFlight {
                        instruments: Some(pb::VehicleFlightInstruments {
                            speed: 120.0,
                            altitude,
                            radar_altitude: altitude,
                            heading,
                            ..Default::default()
                        }),
                        ..Default::default()
                    }),
                    ..Default::default()
                }),
            }),
            ..Default::default()
        };
        // Round-trip through the wire format, as the stream delivers it.
        let frame =
            Frame::from(pb::TelemetryStreamResponse::decode(&*msg.encode_to_vec()).unwrap());
        let v = frame.vehicle.unwrap();
        assert_eq!(v.class_name, "KRIG_S65_Stingray");
        v.flight.unwrap().instruments
    }

    #[test]
    fn no_reference_body_means_no_altitude_or_heading() {
        let i = instruments(f32::MAX, 127.75);
        assert_eq!(
            (i.altitude, i.radar_altitude, i.heading),
            (None, None, None)
        );
        assert_eq!(i.speed, 120.0);
    }

    #[test]
    fn inside_atmosphere_keeps_altitude_and_heading() {
        let i = instruments(1103.7, 92.5);
        assert_eq!((i.altitude, i.heading), (Some(1103.7), Some(92.5)));
    }

    #[test]
    fn on_foot_is_actor_only() {
        let msg = pb::TelemetryStreamResponse {
            actor: Some(pb::ActorFrame {
                state: Some(pb::ActorState {
                    locomotion: pb::ActorLocomotionState::OnFoot as i32,
                    ..Default::default()
                }),
                ..Default::default()
            }),
            events: Some(pb::EventsFrame {
                events: vec![pb::GameEvent { r#type: 4 }, pb::GameEvent { r#type: 99 }],
            }),
            ..Default::default()
        };
        let f = Frame::from(msg);
        assert!(f.vehicle.is_none() && f.weapons.is_none());
        assert_eq!(f.actor.unwrap().locomotion, Locomotion::OnFoot);
        assert_eq!(f.events, vec![Event::LeaveSeat, Event::Other(99)]);
    }
}
