//! Smoke test for `stashes()` / `loadouts()`: per inventory, item counts,
//! ownership states, crafted items; then loadout totals. Read-only, own-account.
//!
//! `cargo run --example stash_dump`

use std::collections::BTreeMap;

use sc_dossier::{ItemNode, LoadoutSlot, Ownership};

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let dossier = sc_dossier::Dossier::from_launcher("sc-dossier-example/0.4").await?;

    let stashes = dossier.stashes().await?;
    for s in &stashes.stashes {
        let mut t = Tally::default();
        s.items.iter().for_each(|n| t.add(n, 0));
        println!(
            "{:?}: {} roots, {} nodes, depth {}, ownership {:?}, ammo {}, crafted {}",
            s.location.context,
            s.items.len(),
            t.nodes,
            t.depth,
            t.ownership,
            t.ammo,
            t.crafted.len()
        );
        for (geid, c) in &t.crafted {
            println!("  crafted {geid}: {} in, {} props, out {}", c.0, c.1, c.2);
        }
    }
    for (loc, e) in &stashes.failed {
        println!("FAILED {:?}: {e}", loc.context);
    }

    let loadouts = dossier.loadouts().await?;
    let saved = loadouts
        .iter()
        .filter(|l| l.slot == LoadoutSlot::Saved)
        .count();
    let entries: usize = loadouts.iter().map(|l| l.entries.len()).sum();
    println!(
        "\nloadouts: {} ({saved} saved), {entries} entries",
        loadouts.len()
    );
    Ok(())
}

#[derive(Default)]
struct Tally {
    nodes: usize,
    depth: usize,
    ammo: usize,
    ownership: BTreeMap<&'static str, usize>,
    crafted: Vec<(u64, (usize, usize, u32))>,
}

impl Tally {
    fn add(&mut self, n: &ItemNode, depth: usize) {
        self.nodes += 1;
        self.depth = self.depth.max(depth);
        self.ammo += usize::from(n.ammo.is_some());
        let o = match n.ownership {
            Ownership::None => "none",
            Ownership::Imprinted { .. } => "imprinted",
            Ownership::Rented { .. } => "rented",
            Ownership::Bricked { .. } => "bricked",
            _ => "other",
        };
        *self.ownership.entry(o).or_default() += 1;
        if let Some(c) = &n.crafted {
            self.crafted.push((
                n.geid,
                (c.ingredients.len(), c.properties.len(), c.output_class_crc),
            ));
        }
        for c in n.attached.iter().chain(&n.contents) {
            self.add(c, depth + 1);
        }
    }
}
