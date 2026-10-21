//! Smoke test for `Dossier::reputations()` — dumps the player's standings
//! (org GEID, scope, score, current rank). Read-only, own-account.
//!
//! `cargo run --example reputations_dump`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dossier = sc_dossier::Dossier::from_launcher("sc-dossier-example/0.12").await?;
    let reps = dossier.reputations().await?;
    eprintln!("[+] {} standing(s)", reps.len());
    for r in &reps {
        let s = r.standing.as_ref();
        println!(
            "{:<14} {:<40} {:>8}  {:<40} min={:<7} gated={}",
            r.org_geid.map_or_else(|| r.entity.clone(), |g| g.to_string()),
            r.scope,
            r.score,
            s.map_or("-", |s| s.name.as_str()),
            s.map_or(0, |s| s.min),
            s.is_some_and(|s| s.gated),
        );
    }
    Ok(())
}
