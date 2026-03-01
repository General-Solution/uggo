use std::collections::HashMap;
use std::path::PathBuf;

use uggo_ugg_api::UggApiBuilder; // adjust crate name/path if needed
use ddragon::models::champions::ChampionShort;
use ugg_types::mappings;
use ugg_types::matchups::MatchupData;
use ugg_types::overview::Overview; // adjust if your Overview lives elsewhere

#[derive(Debug, Clone)]
struct ChampRoleResult<'a> {
    champ: &'a ChampionShort,
    role: mappings::Role,
    real_wr: f64,      // 0..1
    adj_wr: f64,       // 0..1
    diff: f64,         // adj - real
    playrate: f64,     // 0..1 (for that role)
}

pub fn list_meta_adjusted_winrates() -> Result<(), Box<dyn std::error::Error>> {
    // ---- CONFIG ----
    let role = mappings::Role::Top; // pick the role you want (Top/Jungle/Mid/Adc/Support)
    let region = mappings::Region::World;
    let mode = mappings::Mode::Normal; // adjust to whatever your crate calls ranked queue
    let build = mappings::Build::Recommended; // only needed for get_stats; adjust as needed
    let cache_dir = PathBuf::from("./cache");

    // Build API (set cache_dir if you want)
    let api = UggApiBuilder::new()
        .log_cache(true)
        .cache_dir(&cache_dir)
        .build()?;

    // Get list of champions once
    let champs: Vec<ChampionShort> = api.champ_data.values().cloned().collect();
    
    let champ_by_id: std::collections::HashMap<i64, &ChampionShort> = api
    .champ_data
    .values()
    .map(|c| (c.key.parse::<i64>().unwrap_or(0), c))
    .filter(|(id, _)| *id != 0)
    .collect();


    // 1) Fetch playrates (and real winrates) for all champs in the role
    //    We store playrate keyed by champion_id for easy lookup when weighting matchups.
    let (real_stats_by_id, playrate_by_id, resolved_role_by_id) =
        fetch_real_stats_and_playrates(&api, &champs, role, region, mode, build);

    // 2) Fetch matchups and compute meta-adjusted winrates
    let mut results: Vec<ChampRoleResult> = Vec::with_capacity(champs.len());

    for (champ_id, champ_data )in &champ_by_id {

        let Some((real_wr, champ_playrate)) = real_stats_by_id
            .get(&champ_id)
            .copied()
            .zip(playrate_by_id.get(&champ_id).copied())
        else {
            continue;
        };

        // If UGG re-maps the role (some APIs return "closest valid role"), use it consistently:
        let effective_role = resolved_role_by_id.get(&champ_id).copied().unwrap_or(role);

        let (matchup_data, _returned_role) = match api.get_matchups(champ_data, effective_role, region, mode) {
            Ok(v) => v,
            Err(_e) => {
                // You can log errors here if you want
                continue;
            }
        };

        let adj_wr = meta_adjusted_winrate(&matchup_data, &playrate_by_id);

        results.push(ChampRoleResult {
            champ: champ_data,
            role: effective_role,
            real_wr,
            adj_wr,
            diff: adj_wr - real_wr,
            playrate: champ_playrate,
        });
    }

    // 3) Order by difference (largest absolute shift first, or just highest positive)
    results.sort_by(|a, b| b.diff.partial_cmp(&a.diff).unwrap_or(std::cmp::Ordering::Equal));

    // 4) Print table
    print_table(&results);

    Ok(())
}

/// Fetches (real winrate, playrate, returned_role) for all champions in `role`.
///
/// Returns:
/// - real_stats_by_id: champ_id -> real winrate (0..1)
/// - playrate_by_id: champ_id -> playrate in that role (0..1)
/// - resolved_role_by_id: champ_id -> role (UGG may "snap" role to one it supports)
fn fetch_real_stats_and_playrates(
    api: &uggo_ugg_api::UggApi,
    champs: &[ChampionShort],
    role: mappings::Role,
    region: mappings::Region,
    mode: mappings::Mode,
    build: mappings::Build,
) -> (
    HashMap<i64, f64>,                 // real winrate
    HashMap<i64, f64>,                 // playrate
    HashMap<i64, mappings::Role>,      // resolved role
) {
    let mut overview_by_id: HashMap<i64, Overview> = HashMap::new();
    let mut resolved_role_by_id: HashMap<i64, mappings::Role> = HashMap::new();

    // First pass: collect all overviews
    for champ in champs {
        let champ_id = champ.key.parse::<i64>().unwrap_or(0); // adjust if needed

        let Ok((overview, returned_role)) =
            api.get_stats(champ, role, region, mode, build)
        else {
            continue;
        };

        overview_by_id.insert(champ_id, overview);
        resolved_role_by_id.insert(champ_id, returned_role);
    }

    // Compute total matches across role
    let total_matches: f64 = overview_by_id
        .values()
        .map(|o| o.matches() as f64)
        .sum();

    let mut real_wr_by_id: HashMap<i64, f64> = HashMap::new();
    let mut playrate_by_id: HashMap<i64, f64> = HashMap::new();

    for (champ_id, overview) in overview_by_id {
        if overview.matches() == 0 {
            continue;
        }

        let winrate = overview.wins() as f64 / overview.matches() as f64;

        // Playrate as % of total games in that role
        let playrate = if total_matches > 0.0 {
            overview.matches() as f64 / total_matches
        } else {
            0.0
        };

        real_wr_by_id.insert(champ_id, winrate);
        playrate_by_id.insert(champ_id, playrate);
    }

    (real_wr_by_id, playrate_by_id, resolved_role_by_id)
}

/// Compute meta-adjusted winrate for a champ from its matchup list:
/// sum( winrate_vs_enemy * enemy_playrate )
///
/// If some enemies are missing playrate, they are ignored and we renormalize by the total weight used.
fn meta_adjusted_winrate(matchups: &MatchupData, enemy_playrate_by_id: &HashMap<i64, f64>) -> f64 {
    // Adjust this iteration depending on your MatchupData shape.
    // From your earlier snippet you had something like matchups.matchups, and Matchup has champion_id + winrate.
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for m in &matchups.matchups {
        let enemy_id = m.champion_id;

        let Some(w) = enemy_playrate_by_id.get(&enemy_id).copied() else {
            continue;
        };

        // m.winrate is assumed 0..1 already. If it’s 0..100, divide by 100.0 here.
        let wr_vs = m.winrate;

        weighted_sum += wr_vs * w;
        total_weight += w;
    }

    if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        // Fallback if we couldn't weight anything: return NaN-safe 0.5 or 0.0.
        0.5
    }
}

/// Simple fixed-width printing (no external crate).
fn print_table(rows: &[ChampRoleResult]) {

    let min_playrate = 0.005   ; // 2%

    let filtered: Vec<_> = rows
        .into_iter()
        .filter(|r| r.playrate >= min_playrate)
        .collect();


    println!(
        "{:<18} {:<8} {:>10} {:>10} {:>10} {:>10}",
        "Champion", "Role", "RealWR", "AdjWR", "Diff", "PlayRate"
    );
    println!("{}", "-".repeat(72));

    for r in filtered {
        println!(
            "{:<18} {:<8} {:>9.2}% {:>9.2}% {:>+9.2}% {:>9.2}%",
            r.champ.name,
            format!("{:?}", r.role),
            r.real_wr * 100.0,
            r.adj_wr * 100.0,
            r.diff * 100.0,
            r.playrate * 100.0
        );
    }
}