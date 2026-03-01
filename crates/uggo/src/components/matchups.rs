use std::{borrow::Cow, collections::HashMap};

use ddragon::models::champions::ChampionShort;
use ratatui::{
    style::{Color, Style, Stylize},
    widgets::{Paragraph, Widget},
};
use ugg_types::matchups::{Matchup, MatchupData};

/// Select best and worst matchups from the full list.
/// Assumes `matchups` is already sorted by winrate desc (recommended),
/// but still behaves reasonably even if it isn't.
/// Returns borrowed references to avoid cloning.
fn best_and_worst_slices<'a>(matchups: &'a [Matchup], n: usize) -> (&'a [Matchup], &'a [Matchup]) {
    if matchups.is_empty() || n == 0 {
        return (&[], &[]);
    }
    let k = n.min(matchups.len());
    let best = &matchups[..k];
    let worst = &matchups[matchups.len() - k..]; // still in winrate-desc order
    (best, worst)
}

pub fn make_matchup_row<'a>(
    title: &'a str,
    matchups: &'a [Matchup],
    champ_data: &'a HashMap<String, ChampionShort>,
) -> Paragraph<'a> {
    Paragraph::new(format!(
        " {}: {}",
        title,
        matchups
            .iter()
            .filter_map(|m| {
                champ_data
                    .get(&m.champion_id.to_string())
                    .map(|c| Cow::from(c.name.clone()))
            })
            .reduce(|mut acc, s| {
                acc.to_mut().push_str(", ");
                acc.to_mut().push_str(&s);
                acc
            })
            .unwrap_or_default()
            .into_owned()
    ))
}

pub fn make<'a>(
    matchups: &'a MatchupData,
    champ_data: &'a HashMap<String, ChampionShort>,
) -> [Paragraph<'a>; 2] {
    let (best, worst) = best_and_worst_slices(&matchups.matchups, 5);
    [
        make_matchup_row("Best Matchups", best, champ_data)
            .style(Style::default().fg(Color::Cyan).bold()),
        make_matchup_row("Worst Matchups", worst, champ_data)
            .style(Style::default().fg(Color::Red).bold()),
    ]
}