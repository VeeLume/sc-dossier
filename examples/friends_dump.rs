//! Smoke test for `Dossier::friends()`: online friends first. Read-only,
//! own-account.
//!
//! `cargo run --example friends_dump`

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dossier =
        sc_dossier::Dossier::from_launcher("sc-dossier-example/0.17", sc_dossier::auth::Env::Live)
            .await?;
    let mut friends = dossier.friends().await?;
    friends.sort_by_key(|f| (!f.is_online(), f.nickname.to_lowercase()));
    let online = friends.iter().filter(|f| f.is_online()).count();
    eprintln!("[+] {} friend(s), {online} online", friends.len());
    for f in &friends {
        let where_ = match &f.presence {
            None => "offline".to_string(),
            Some(p) => format!("{:?} / {:?}", p.status, p.activity),
        };
        println!("{:<28} {:<24} {where_}", f.nickname, f.display_name);
    }
    Ok(())
}
