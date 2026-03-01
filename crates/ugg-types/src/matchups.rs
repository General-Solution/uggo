use crate::mappings;
use serde::Serialize;
use serde::de::{Deserialize, Deserializer, IgnoredAny, SeqAccess, Visitor};
use std::collections::HashMap;
use std::fmt;

pub type Matchups =
    HashMap<mappings::Region, HashMap<mappings::Rank, HashMap<mappings::Role, WrappedMatchupData>>>;

#[derive(Debug, Clone, Serialize)]
pub struct WrappedMatchupData {
    pub data: MatchupData,
}

impl<'de> Deserialize<'de> for WrappedMatchupData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct WrappedMatchupDataVisitor;

        impl<'de> Visitor<'de> for WrappedMatchupDataVisitor {
            type Value = WrappedMatchupData;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence whose first element is MatchupData")
            }

            fn visit_seq<V>(self, mut visitor: V) -> Result<WrappedMatchupData, V::Error>
            where
                V: SeqAccess<'de>,
            {
                match visitor.next_element::<MatchupData>() {
                    Ok(Some(data)) => {
                        while let Some(IgnoredAny) = visitor.next_element()? {}
                        Ok(WrappedMatchupData { data })
                    }
                    _ => Err(serde::de::Error::missing_field("top-level element")),
                }
            }
        }

        deserializer.deserialize_seq(WrappedMatchupDataVisitor)
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct MatchupData {
    /// All matchups (no filtering, not truncated).
    pub matchups: Vec<Matchup>,
    pub total_matches: i32,
}

#[derive(Debug, Clone, Serialize)]
pub struct Matchup {
    pub champion_id: i64,
    pub wins: i32,
    pub matches: i32,
    pub winrate: f64,
}

impl<'de> Deserialize<'de> for MatchupData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct MatchupDataVisitor;

        impl<'de> Visitor<'de> for MatchupDataVisitor {
            type Value = MatchupData;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("matchup data as a sequence of inner matchup rows")
            }

            fn visit_seq<V>(self, mut visitor: V) -> Result<MatchupData, V::Error>
            where
                V: SeqAccess<'de>,
            {
                let mut all_matchups: Vec<Matchup> = Vec::new();
                let mut total_matches: i32 = 0;

                while let Ok(row_opt) = visitor.next_element::<InnerData>() {
                    match row_opt {
                        Some(row) => {
                            let wins = row.2 - row.1;
                            let winrate = if row.2 > 0 {
                                f64::from(wins) / f64::from(row.2)
                            } else {
                                0.0
                            };

                            all_matchups.push(Matchup {
                                champion_id: row.0,
                                wins,
                                matches: row.2,
                                winrate,
                            });

                            total_matches += row.2;
                        }
                        None => break,
                    }
                }

                // No filtering. Keep everything.
                // Optional: keep a deterministic ordering (by winrate desc, then matches desc).
                all_matchups.sort_by(|a, b| {
                    b.winrate
                        .partial_cmp(&a.winrate)
                        .unwrap_or(std::cmp::Ordering::Equal)
                        .then_with(|| b.matches.cmp(&a.matches))
                });

                Ok(MatchupData {
                    matchups: all_matchups,
                    total_matches,
                })
            }
        }

        deserializer.deserialize_seq(MatchupDataVisitor)
    }
}

struct InnerData(i64, i32, i32);

impl<'de> Deserialize<'de> for InnerData {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        struct InnerSeqVisitor;

        impl<'de> Visitor<'de> for InnerSeqVisitor {
            type Value = InnerData;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a sequence with at least 3 elements")
            }

            fn visit_seq<A>(self, mut visitor: A) -> Result<Self::Value, A::Error>
            where
                A: SeqAccess<'de>,
            {
                let champion_id = visitor
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(0, &self))?;

                let losses = visitor
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(1, &self))?;

                let matches = visitor
                    .next_element()?
                    .ok_or_else(|| serde::de::Error::invalid_length(2, &self))?;

                while let Some(IgnoredAny) = visitor.next_element()? {}

                Ok(InnerData(champion_id, losses, matches))
            }
        }

        deserializer.deserialize_seq(InnerSeqVisitor)
    }
}