use std::path::PathBuf;

use ugg_types::mappings::{Build, Mode, Region, Role};

use uggo_ugg_api::{UggApiBuilder}; // <-- replace with your crate name
use ddragon::models::champions::ChampionShort;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    // -----------------------------
    // Hardcoded champion name here
    // -----------------------------
    let champ_query = "Ahri";

    // Choose some reasonable defaults
    let region = Region::World;
    let role = Role::Mid;
    let mode = Mode::Normal;

    // Cache directory (must exist or be creatable)
    let cache_dir = PathBuf::from("./cache");

    let api = UggApiBuilder::new()
        .cache_dir(&cache_dir)
        .cache_ttl_hours(24)
        .log_cache(true)
        .build()?;

    let champ_by_id: std::collections::HashMap<i64, &ChampionShort> = api
        .champ_data
        .values()
        .map(|c| (c.key.parse::<i64>().unwrap_or(0), c))
        .filter(|(id, _)| *id != 0)
        .collect();

    let champ: &ChampionShort = api.find_champ(champ_query);

    println!("Champion: {}", champ.name);
    println!("Searching matchups for role: {:?}\n", role);

    let (matchup_data, resolved_role) =
        api.get_matchups(champ, role, region, mode)?;

    println!("Resolved role: {:?}\n", resolved_role);

    let mut matchups = matchup_data.matchups.clone();

    // Sort descending by winrate
    matchups.sort_by(|a, b| {
        b.winrate
            .partial_cmp(&a.winrate)
            .unwrap()
    });
    
    println!("Top 5 Matchups:\n");

    for (i, matchup) in matchups.iter().take(5).enumerate() {
        let enemy_name: &str = champ_by_id
            .get(&matchup.champion_id)
            .map(|c| c.name.as_str())
            .unwrap_or("<unknown>");

        println!(
            "{}. {:<15} | Winrate: {:>5.2}% | Matches: {}",
            i + 1,
            enemy_name,
            matchup.winrate * 100.0,
            matchup.matches
        );
    }

    Ok(())
}