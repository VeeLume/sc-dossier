//! Smoke test for `Dossier::mission_tokens()`: the token wallet, newest first.
//! Read-only, own-account.
//!
//! `cargo run --example tokens_dump`

use sc_dossier::TokenKind;

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dossier = sc_dossier::Dossier::from_launcher("sc-dossier-example/0.15").await?;
    let mut tokens = dossier.mission_tokens().await?;
    eprintln!("[+] {} token(s)", tokens.len());
    tokens.sort_by_key(|t| std::cmp::Reverse(t.created_at_ms));
    for t in &tokens {
        let what = match &t.kind {
            TokenKind::Contract { name } => format!("done  {name}"),
            TokenKind::Tag { tag_id, contract, added } => {
                format!("tag   {tag_id} +{added} ({contract})")
            }
            _ => format!("other {} {:?}", t.id, t.reason),
        };
        println!("{:>14} {:>6}  {what}", t.created_at_ms, t.count);
    }
    Ok(())
}
