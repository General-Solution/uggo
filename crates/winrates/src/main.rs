mod meta_adjusted_winrate; // adjust if your Overview lives elsewhere
mod best_2_pool;
mod group_winrate;
use ugg_types::mappings::{Build, Mode, Region, Role};
use uggo_ugg_api::UggApiBuilder;

use std::path::PathBuf;
fn main() -> Result<(), Box<dyn std::error::Error>> {
    let cache_dir = PathBuf::from("./cache");

    let api = UggApiBuilder::new()
        .log_cache(false)
        .cache_dir(&cache_dir)
        .build()?;


    best_2_pool::print_best_two_champ_pool_against_meta(&api, "darius", Role::Top);
    best_2_pool::print_best_two_champ_pool_against_meta(&api, "ambessa", Role::Top);

    Ok(())
}
