use serde::de::{Deserialize, Deserializer, IgnoredAny, MapAccess, SeqAccess, Visitor};
use serde::Serialize;
use std::collections::HashMap;
use std::fmt;

use crate::mappings::{Role, get_role};

/// If you already have this helper, reuse it.
/// It typically turns Option<T> into T (defaulting when missing / null / etc).
use crate::overview::handle_unknown;

// -----------------------------
// Raw "winrates" packed response
// -----------------------------
//
// Based on your example, each role key maps to a list of champions.
//
// role -> [
//   [ champ_id_string, [ matchups, wins, matches, total_damage, total_gold, kills, assists, deaths, total_cs ] ],
//   ...
// ]
//
// matchups is a list of [enemy_id, enemy_wins, games] where enemy_wins are the opponent's wins.
//
// This module only "names fields" and keeps integers. No derived floats.

fn log_stage(stage: &str) {
    // swap to `tracing::debug!` if you prefer
    eprintln!("[meta_summary parse] {stage}");
}

fn log_err<E: std::fmt::Debug>(stage: &str, err: &E) {
    eprintln!("[meta_summary parse] ERROR at {stage}: {err:?}");
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct RoleWinrates {
    /// keyed by role name from the JSON (e.g. "adc", "top", "jungle", ...)
    pub roles: HashMap<Role, Vec<ChampionWinrateRaw>>,
}

impl<'de> Deserialize<'de> for RoleWinrates {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RoleWinratesVisitor;

        impl<'de> Visitor<'de> for RoleWinratesVisitor {
            type Value = RoleWinrates;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a map of role -> champion winrate entries")
            }

            fn visit_map<M>(self, mut map: M) -> Result<RoleWinrates, M::Error>
            where
                M: MapAccess<'de>,
            {
                let mut roles = HashMap::<Role, Vec<ChampionWinrateRaw>>::new();

                while let Some(role_key) = map.next_key::<String>()? {
                    let role = get_role(&role_key);
                
                    let champs = map.next_value::<Vec<ChampionWinrateRaw>>()?;
                    roles.insert(role, champs);
                }

                Ok(RoleWinrates { roles })
            }
        }

        deserializer.deserialize_map(RoleWinratesVisitor)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ChampionWinrateRaw {
    pub champion_id: String,
    pub stats: ChampionWinrateStatsRaw,
}

impl<'de> Deserialize<'de> for ChampionWinrateRaw {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct ChampVisitor;

        impl<'de> Visitor<'de> for ChampVisitor {
            type Value = ChampionWinrateRaw;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str(r#"["<champion_id>", [[matchups...]], wins, matches, total_damage, total_gold, kills, assists, deaths, cs, ...]"#)
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<ChampionWinrateRaw, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let champion_id = handle_unknown(seq.next_element::<String>());

                // Now parse the packed fields from the SAME seq (not a nested stats array)
                let worst_matchups = handle_unknown(seq.next_element::<Vec<MatchupVsEnemyRaw>>());

                let wins = handle_unknown(seq.next_element::<i64>());
                let matches = handle_unknown(seq.next_element::<i64>());

                let total_damage = handle_unknown(seq.next_element::<i64>());
                let total_gold = handle_unknown(seq.next_element::<i64>());

                let total_kills = handle_unknown(seq.next_element::<i64>());
                let total_assists = handle_unknown(seq.next_element::<i64>());
                let total_deaths = handle_unknown(seq.next_element::<i64>());

                let total_cs = handle_unknown(seq.next_element::<i64>());

                // Keep any future appended ints.
                let mut extra = Vec::<i64>::new();
                while let Some(v) = seq.next_element::<i64>()? {
                    extra.push(v);
                }

                Ok(ChampionWinrateRaw {
                    champion_id,
                    stats: ChampionWinrateStatsRaw {
                        worst_matchups,
                        wins,
                        matches,
                        total_damage,
                        total_gold,
                        total_kills,
                        total_assists,
                        total_deaths,
                        total_cs,
                        extra,
                    },
                })
            }
        }

        deserializer.deserialize_seq(ChampVisitor)
    }
}

#[derive(Debug, Clone, Serialize, Default)]
pub struct ChampionWinrateStatsRaw {
    pub worst_matchups: Vec<MatchupVsEnemyRaw>,

    // totals for this champ in this role
    pub wins: i64,
    pub matches: i64,

    // summed stats across matches
    pub total_damage: i64,
    pub total_gold: i64,
    pub total_kills: i64,
    pub total_assists: i64,
    pub total_deaths: i64,
    pub total_cs: i64,

    /// Any extra integers appended by the API that we don't know about yet.
    /// Keeping them lets you debug/new-field without breaking parsing.
    pub extra: Vec<i64>,
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchupVsEnemyRaw {
    pub enemy_champion_id: i64,
    /// opponent wins (so your wins are matches - enemy_wins)
    pub enemy_wins: i64,
    pub matches: i64,
}

impl<'de> Deserialize<'de> for MatchupVsEnemyRaw {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MatchupVisitor;

        impl<'de> Visitor<'de> for MatchupVisitor {
            type Value = MatchupVsEnemyRaw;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("[enemy_id, enemy_wins, matches]")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<MatchupVsEnemyRaw, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let enemy_champion_id = handle_unknown(seq.next_element::<i64>());
                let enemy_wins = handle_unknown(seq.next_element::<i64>());
                let matches = handle_unknown(seq.next_element::<i64>());

                // Ignore any trailing elements if present
                while let Some(IgnoredAny) = seq.next_element()? {}

                Ok(MatchupVsEnemyRaw {
                    enemy_champion_id,
                    enemy_wins,
                    matches,
                })
            }
        }

        deserializer.deserialize_seq(MatchupVisitor)
    }
}

// ----------------------------------
// Optional: top-level wrapper shape
// ----------------------------------
//
// Your raw response looked like: [ { "adc": [...], "top": [...], ... } ]
// i.e. an array with a single object. This lets you parse that.
//
// If your endpoint sometimes returns multiple objects in the array, this still works.
#[derive(Debug, Clone, Serialize, Default)]
pub struct WinratesRawResponse {
    pub pages: Vec<RoleWinrates>,
}

impl<'de> Deserialize<'de> for WinratesRawResponse {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct RespVisitor;

        impl<'de> Visitor<'de> for RespVisitor {
            type Value = WinratesRawResponse;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("a list of role->champion winrate maps")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<WinratesRawResponse, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let mut pages = Vec::new();
                while let Some(page) = seq.next_element::<RoleWinrates>()? {
                    pages.push(page);
                }
                Ok(WinratesRawResponse { pages })
            }
        }

        deserializer.deserialize_seq(RespVisitor)
    }
}

// Expected raw shape (heterogeneous array):
// [
//   { ...role->[...]... },          // RoleWinrates
//   { "<champ_id>": <count>, ...,
//     "total_matches": <count>,
//     "-1": <count>, ... },         // champion_totals
//   "2026-02-26T20:03:05.373444Z",  // snapshot timestamp
//   1410269.0                       // total_games float
//   ...maybe more...
// ]

#[derive(Debug, Clone, Serialize, Default)]
pub struct MetaSummary {
    pub role_winrates: RoleWinrates,

    /// Raw counts keyed by champion id *as a string*, plus special keys like
    /// "total_matches" and "-1" from the API.
    pub champion_totals: HashMap<String, i64>,

    /// The API snapshot timestamp string (ISO-8601).
    /// (Keep as String here to avoid forcing chrono in this layer.)
    pub snapshot: String,

    /// Final numeric value appended by the API (often a "total processed games" type number).
    pub total_games: f64,
}

impl<'de> Deserialize<'de> for MetaSummary {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MetaSummaryVisitor;

        impl<'de> Visitor<'de> for MetaSummaryVisitor {
            type Value = MetaSummary;

            fn expecting(&self, f: &mut fmt::Formatter) -> fmt::Result {
                f.write_str("meta summary array: [role_winrates, champion_totals, snapshot, total_games, ...]")
            }

            fn visit_seq<V>(self, mut seq: V) -> Result<MetaSummary, V::Error>
            where
                V: SeqAccess<'de>,
            {
                // 1) Role winrates object (role -> packed champion arrays)
                let role_winrates = seq
                    .next_element::<RoleWinrates>()?
                    .ok_or_else(|| serde::de::Error::custom("Missing role winrates"))?;

                // 2) Champion totals map (string champ_id -> count), includes "total_matches" and "-1"
                let champion_totals = seq
                    .next_element::<HashMap<String, i64>>()?
                    .ok_or_else(|| serde::de::Error::custom("Missing champion totals map"))?;

                // 3) Snapshot timestamp string
                let snapshot = seq
                    .next_element::<String>()?
                    .ok_or_else(|| serde::de::Error::custom("Missing snapshot timestamp"))?;

                // 4) Total games float
                let total_games = seq
                    .next_element::<f64>()?
                    .ok_or_else(|| serde::de::Error::custom("Missing total games value"))?;

                // If the API appends more stuff later, ignore it (forward compatible).
                while let Some(IgnoredAny) = seq.next_element()? {}

                Ok(MetaSummary {
                    role_winrates,
                    champion_totals,
                    snapshot,
                    total_games,
                })
            }
        }

        deserializer.deserialize_seq(MetaSummaryVisitor)
    }
}