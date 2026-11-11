//! Print the game's local telemetry, one line per second.
//!
//! The game must run with `GRPC_CLIENT_SERVER_ENABLED=true` set for the launcher.
//!
//! ```bash
//! cargo run --example telemetry_dump --no-default-features --features telemetry
//! ```

use std::ops::ControlFlow;
use std::time::{Duration, Instant};

use sc_dossier::telemetry::{self, Status};

fn main() -> std::io::Result<()> {
    let mut last = Instant::now() - Duration::from_secs(1);
    let handle = telemetry::spawn(telemetry::DEFAULT_ADDR, move |status| {
        match status {
            Status::Frame(f) => {
                if last.elapsed() >= Duration::from_secs(1) {
                    last = Instant::now();
                    let ship = f.vehicle.as_ref().map(|v| v.class_name.as_str());
                    let i = f
                        .vehicle
                        .as_ref()
                        .and_then(|v| v.flight.as_ref())
                        .map(|fl| fl.instruments);
                    println!(
                        "ship={ship:?} speed={:?} alt={:?} hdg={:?} hp={:?} events={:?}",
                        i.map(|i| i.speed),
                        i.and_then(|i| i.altitude),
                        i.and_then(|i| i.heading),
                        f.actor.as_ref().map(|a| a.health.current),
                        f.events,
                    );
                }
            }
            other => println!("{other:?}"),
        }
        ControlFlow::Continue(())
    })?;
    let _ = handle.join();
    Ok(())
}
