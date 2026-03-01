use std::cmp::Ordering;

use uggo_ugg_api::UggApi;
use ugg_types::mappings;
use crate::group_winrate::{fetch_real_stats_and_playrates, calculate_group_winrate};

/// Prints a table of the best 2-champ pool partners for `base_champ_name` in `role`.
///
/// For each possible added champ A (with >= 0.5% playrate in the role),
/// compute combined winrate vs meta of pool {Base, A} using `joint_pool_winrate_vs_meta`,
/// and print rows sorted by highest combined winrate.
///
/// Columns:
/// - Added champ name
/// - Added champ Overview winrate (in-role)
/// - Combined (Base + Added) joint winrate vs meta
pub fn print_best_two_champ_pool_against_meta(
    api: &UggApi,
    base_champ_name: &str,
    role: mappings::Role,
) {
    // ---- CONFIG (match your existing defaults) ----
    let region = mappings::Region::World;
    let mode = mappings::Mode::Normal;
    let build = mappings::Build::Recommended;

    let min_playrate = 0.005; // 0.5%

    // Resolve base champ via fuzzy finder
    let base = api.find_champ(base_champ_name);
    let base_id = match base.key.parse::<i64>() {
        Ok(id) if id != 0 => id,
        _ => {
            eprintln!("Could not parse champion id for base champ: {}", base.name);
            return;
        }
    };

    // Fetch role-wide playrates + overview winrates for filtering & display
    let (real_wr_by_id, playrate_by_id, _resolved_role_by_id) =
        fetch_real_stats_and_playrates(api, role, region, mode, build);

    // Pre-filter candidates by playrate threshold
    // (also skip adding the base champ itself)
    let mut candidates: Vec<(i64, String, f64 /*added_wr*/)> = Vec::new();
    for champ in api.champ_data.values() {
        let id = match champ.key.parse::<i64>() {
            Ok(id) if id != 0 => id,
            _ => continue,
        };

        if id == base_id {
            continue;
        }

        let pr = playrate_by_id.get(&id).copied().unwrap_or(0.0);
        if pr < min_playrate {
            continue;
        }

        let added_wr = real_wr_by_id.get(&id).copied().unwrap_or(0.5);
        candidates.push((id, champ.name.clone(), added_wr));
    }

    // Compute combined winrate for each candidate addition
    #[derive(Debug, Clone)]
    struct Row {
        added_name: String,
        added_wr: f64,    // 0..1
        combined_wr: f64, // 0..1
    }

    let mut rows: Vec<Row> = Vec::with_capacity(candidates.len());

    for (added_id, added_name, added_wr) in candidates {
        let pool = [base_id, added_id];

        let combined_wr = match calculate_group_winrate(api, &pool, role, region, mode, build) {
            Ok(v) => v,
            Err(_e) => continue, // if desired: eprintln!("error for {}: {}", added_name, _e);
        };

        rows.push(Row {
            added_name,
            added_wr,
            combined_wr,
        });
    }

    // Sort by highest combined winrate
    rows.sort_by(|a, b| {
        b.combined_wr
            .partial_cmp(&a.combined_wr)
            .unwrap_or(Ordering::Equal)
    });

    // Print table
    println!(
        "\nBest 2-champ pools for '{}' in {:?} (min added playrate {:.2}%):",
        base.name,
        role,
        min_playrate * 100.0
    );
    println!("{:<22} {:>12} {:>14}", "Added Champ", "Added WR", "Combined WR");
    println!("{}", "-".repeat(52));

    for r in rows {
        println!(
            "{:<22} {:>11.2}% {:>13.2}%",
            r.added_name,
            r.added_wr * 100.0,
            r.combined_wr * 100.0
        );
    }
}