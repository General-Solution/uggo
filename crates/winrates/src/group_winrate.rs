use std::collections::HashMap;

use ddragon::models::champions::ChampionShort;
use uggo_ugg_api::UggApi;
use ugg_types::{mappings, matchups::MatchupData, overview::Overview};

/// Computes the pool's "joint winrate vs meta" for a given role:
///
/// For each enemy champion E (including champs inside the pool),
/// assume you pick the pool champ C that has the highest winrate vs E.
///
/// Then compute:
///     sum_E ( best_wr(pool -> E) * playrate(E) )
///
/// Mirror-match restriction:
/// - If E is in the pool, you may NOT select C == E (no mirrors).
/// - If the pool has no other champ to pick for that E (e.g. pool size=1),
///   we use 0.5 as a neutral fallback for that enemy.
///
/// Notes/assumptions:
/// - `Matchup.winrate` is in 0..1
/// - Enemy playrates are derived from `Overview.matches()` totals in the role.
pub fn calculate_group_winrate(
    api: &UggApi,
    pool_champion_ids: &[i64],
    role: mappings::Role,
    region: mappings::Region,
    mode: mappings::Mode,
    build: mappings::Build,
) -> Result<f64, Box<dyn std::error::Error>> {
    // 1) Map pool ids -> ChampionShort
    let pool_champs: Vec<(i64, &ChampionShort)> = pool_champion_ids
        .iter()
        .copied()
        .filter_map(|id| champ_by_numeric_id(api, id).map(|c| (id, c)))
        .collect();

    if pool_champs.is_empty() {
        return Err("pool_champion_ids did not resolve to any champions".into());
    }

    // 2) Fetch enemy playrates for the role across *all* champions
    //    (includes pool champs as enemies automatically)
    let (_real_wr_by_id, enemy_playrate_by_id, resolved_role_by_id) =
        fetch_real_stats_and_playrates(api, role, region, mode, build);

    // 3) For each pool champ, fetch matchups and build (enemy_id -> wr_vs_enemy)
    //    Also respect per-champ role snapping returned by UGG.
    let mut wr_vs_enemy_by_pool_champ: HashMap<i64, HashMap<i64, f64>> = HashMap::new();

    for (pool_id, champ) in &pool_champs {
        let effective_role = resolved_role_by_id.get(pool_id).copied().unwrap_or(role);

        let (matchup_data, _returned_role) = match api.get_matchups(champ, effective_role, region, mode) {
            Ok(v) => v,
            Err(_e) => continue, // if desired: log and keep going
        };

        wr_vs_enemy_by_pool_champ.insert(*pool_id, matchup_map(&matchup_data));
    }

    // 4) Aggregate: for each enemy, take the best pool champ into that enemy (no mirrors)
    let mut weighted_sum = 0.0;
    let mut total_weight = 0.0;

    for (&enemy_id, &enemy_playrate) in &enemy_playrate_by_id {
        if enemy_playrate <= 0.0 {
            continue;
        }

        let best_wr = best_pool_wr_into_enemy(enemy_id, &pool_champs, &wr_vs_enemy_by_pool_champ);

        weighted_sum += best_wr * enemy_playrate;
        total_weight += enemy_playrate;
    }

    Ok(if total_weight > 0.0 {
        weighted_sum / total_weight
    } else {
        0.5
    })
}

/// --- Helpers ---

fn champ_by_numeric_id<'a>(api: &'a UggApi, numeric_id: i64) -> Option<&'a ChampionShort> {
    api.champ_data.values().find(|c| c.key.parse::<i64>().ok() == Some(numeric_id))
}

/// Build a map: enemy_champion_id -> winrate_vs_enemy (0..1)
fn matchup_map(matchups: &MatchupData) -> HashMap<i64, f64> {
    let mut m = HashMap::new();
    for mu in &matchups.matchups {
        m.insert(mu.champion_id, mu.winrate);
    }
    m
}

/// Choose the best achievable winrate from the pool into a specific enemy.
///
/// Mirror restriction:
/// - If enemy_id == pool champ id, skip that champ.
/// Fallback:
/// - If nothing is selectable or matchup data is missing everywhere, returns 0.5.
fn best_pool_wr_into_enemy(
    enemy_id: i64,
    pool_champs: &[(i64, &ChampionShort)],
    wr_vs_enemy_by_pool_champ: &HashMap<i64, HashMap<i64, f64>>,
) -> f64 {
    let mut best: Option<f64> = None;

    for (pool_id, _champ) in pool_champs {
        // no mirror match
        if *pool_id == enemy_id {
            continue;
        }

        let wr = wr_vs_enemy_by_pool_champ
            .get(pool_id)
            .and_then(|by_enemy| by_enemy.get(&enemy_id))
            .copied();

        if let Some(wr) = wr {
            best = Some(best.map_or(wr, |b| b.max(wr)));
        }
    }

    best.unwrap_or(0.5)
}

/// Fetches (real winrate, playrate, returned_role) for all champions in `role`.
///
/// Returns:
/// - real_wr_by_id: champ_id -> real winrate (0..1)
/// - playrate_by_id: champ_id -> playrate in that role (0..1)
/// - resolved_role_by_id: champ_id -> role (UGG may "snap" role to one it supports)
pub fn fetch_real_stats_and_playrates(
    api: &UggApi,
    role: mappings::Role,
    region: mappings::Region,
    mode: mappings::Mode,
    build: mappings::Build,
) -> (
    HashMap<i64, f64>,            // real winrate
    HashMap<i64, f64>,            // playrate
    HashMap<i64, mappings::Role>, // resolved role
) {
    let champs: Vec<ChampionShort> = api.champ_data.values().cloned().collect();

    let mut overview_by_id: HashMap<i64, Overview> = HashMap::new();
    let mut resolved_role_by_id: HashMap<i64, mappings::Role> = HashMap::new();

    for champ in &champs {
        let champ_id = champ.key.parse::<i64>().unwrap_or(0);
        if champ_id == 0 {
            continue;
        }

        let Ok((overview, returned_role)) = api.get_stats(champ, role, region, mode, build) else {
            continue;
        };

        overview_by_id.insert(champ_id, overview);
        resolved_role_by_id.insert(champ_id, returned_role);
    }

    let total_matches: f64 = overview_by_id.values().map(|o| o.matches() as f64).sum();

    let mut real_wr_by_id: HashMap<i64, f64> = HashMap::new();
    let mut playrate_by_id: HashMap<i64, f64> = HashMap::new();

    for (champ_id, overview) in overview_by_id {
        let matches = overview.matches() as f64;
        if matches <= 0.0 {
            continue;
        }

        let winrate = overview.wins() as f64 / matches;

        let playrate = if total_matches > 0.0 {
            matches / total_matches
        } else {
            0.0
        };

        real_wr_by_id.insert(champ_id, winrate);
        playrate_by_id.insert(champ_id, playrate);
    }

    (real_wr_by_id, playrate_by_id, resolved_role_by_id)
}