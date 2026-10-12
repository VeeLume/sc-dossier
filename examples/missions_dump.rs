//! Smoke test for `Dossier::active_missions()` — dumps the player's accepted
//! missions (ids, location CRCs, objective progress). Read-only, own-account.
//!
//! `cargo run --example missions_dump`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dossier = sc_dossier::Dossier::from_launcher("sc-dossier-example/0.3").await?;
    let missions = dossier.active_missions().await?;
    eprintln!("[+] {} active mission(s)", missions.len());
    for m in &missions {
        println!(
            "\nmission {}  contract_def {}",
            m.mission_id, m.contract_def_id
        );
        println!("  pickups={:?}  dropoffs={:?}", m.pickups, m.dropoffs);
        for o in &m.objectives {
            println!(
                "  {:<48} {:?} {:?} {}/{}",
                o.objective_id, o.kind, o.state, o.progress_current, o.progress_max
            );
        }
    }
    Ok(())
}
