//! Smoke test for `Dossier::event_status()` / `event_progress()`. Pass
//! `MissionScenario` GUIDs (from the game data). Read-only, own-account.
//!
//! `cargo run --example events_dump -- 291fcca7-5a15-4c22-8d98-d16a2e76b9e5 …`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let ids: Vec<String> = std::env::args().skip(1).collect();
    let dossier =
        sc_dossier::Dossier::from_launcher("sc-dossier-example/0.15", sc_dossier::auth::Env::Live)
            .await?;
    for s in dossier.event_status(&ids).await? {
        println!("{} {:?}", s.template_id, s.state);
    }
    for id in &ids {
        match dossier.event_progress(id).await? {
            None => println!("{id}: no progress tracking"),
            Some(p) => {
                println!("{id}: active={}", p.active);
                for (faction, points) in &p.overall {
                    let own = p.own.get(faction).copied().unwrap_or(0);
                    println!("  {faction}  overall {points}  own {own}");
                }
            }
        }
    }
    Ok(())
}
