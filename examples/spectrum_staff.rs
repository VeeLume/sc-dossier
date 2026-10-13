//! Smoke test for `spectrum` — the newest page of the CIG staff feed (forum +
//! chat), optionally for one author. Read-only, own-account session.
//!
//! `cargo run --example spectrum_staff [-- <nickname>]`

use sc_dossier::spectrum::{SearchQuery, Spectrum, SC_COMMUNITY};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let spectrum = Spectrum::from_launcher("sc-dossier-example/0.5")?;
    let mut query = SearchQuery::staff();
    if let Some(nick) = std::env::args().nth(1) {
        query = query.author(&spectrum.member_id(&nick).await?);
    }
    let page = spectrum.search(&query).await?;
    eprintln!("[+] {} of {} staff posts", page.posts.len(), page.total);
    for p in &page.posts {
        let place = p
            .lobby
            .as_ref()
            .or(p.channel.as_ref())
            .map_or("", |pl| pl.name.as_str());
        let body: String = p.body.chars().take(80).collect();
        println!(
            "{} {:?} {place} {}: {body}",
            p.time_created, p.kind, p.member.nickname
        );
    }

    let staff = spectrum.community_roles(SC_COMMUNITY).await?;
    eprintln!("[+] {} roles in community {SC_COMMUNITY}", staff.len());

    // sc-testing-chat; needs the session.
    let history = spectrum.lobby_history("38230", None).await?;
    eprintln!("[+] {} messages in lobby 38230", history.len());
    if let Some(m) = history.last() {
        let roles = m
            .member
            .roles
            .get(SC_COMMUNITY)
            .cloned()
            .unwrap_or_default();
        println!(
            "newest: {} {} roles={roles:?}",
            m.time_created, m.member.nickname
        );
    }
    Ok(())
}
