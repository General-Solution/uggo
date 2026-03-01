use crate::util::sha256;
use ddragon::models::Augment;
use ddragon::models::champions::ChampionShort;
use ddragon::models::items::Item;
use ddragon::models::runes::RuneElement;
use ddragon::{Client, ClientBuilder};
use levenshtein::levenshtein;
use serde::de::DeserializeOwned;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use thiserror::Error;
use ugg_types::mappings::{Role, Region, Mode, Build, Rank};
use ugg_types::meta_summary::MetaSummary; // adjust module path to wherever you put it
use ugg_types::matchups::{MatchupData, Matchups};
use ugg_types::overview::{ChampOverview, Overview};
use ugg_types::rune::RuneExtended;
use ureq::Agent;

use std::fs;
use std::io::Read;
use std::time::{Duration, SystemTime};


mod util;

type UggAPIVersions = HashMap<String, HashMap<String, String>>;

#[derive(Error, Debug)]
pub enum UggError {
    #[error("DDragon error")]
    DDragonError(#[from] ddragon::ClientError),
    #[error("HTTP request failed")]
    RequestError(#[from] Box<ureq::Error>),
    #[error("JSON parsing failed")]
    ParseError(#[from] simd_json::Error),
    #[error("Missing region or rank entry")]
    MissingRegionOrRank,
    #[error("Missing role entry")]
    MissingRole,
    #[error("Unknown error occurred")]
    Unknown,
}

pub struct DataApi {
    agent: Agent,
    ddragon: Client,

    // New:
    cache_dir: PathBuf,
    cache_ttl: Duration,
    log_cache: bool,
}

#[derive(Debug, Clone)]
pub struct SupportedVersion {
    pub ddragon: String,
    pub ugg: String,
}

pub struct UggApi {
    api: DataApi,
    api_versions: UggAPIVersions,

    pub current_version: String,
    pub allowed_versions: Vec<SupportedVersion>,
    pub patch_version: String,
    pub champ_data: HashMap<String, ChampionShort>,
    pub items: HashMap<String, Item>,
    pub runes: HashMap<i64, RuneExtended<RuneElement>>,
    pub summoner_spells: HashMap<i64, String>,
    pub arena_augments: HashMap<i64, Augment>,
}

impl DataApi {
    pub fn new(version: Option<String>, cache_dir: Option<PathBuf>) -> Result<Self, UggError> {
        // default: 24h TTL, logging off
        Self::new_with_cache_options(version, cache_dir, 24, false)
    }

    pub fn new_with_cache_options(
        version: Option<String>,
        cache_dir: Option<PathBuf>,
        cache_ttl_hours: u64,
        log_cache: bool,
    ) -> Result<Self, UggError> {
        let mut client_builder = ClientBuilder::new();
        let safe_dir = cache_dir.ok_or(UggError::Unknown)?;

        if let Some(v) = version {
            client_builder = client_builder.version(v.as_str());
        }
        if let Some(dir) = safe_dir.clone().to_str() {
            client_builder = client_builder.cache(dir);
        }

        Ok(Self {
            agent: Agent::new_with_defaults(),
            ddragon: client_builder.build()?,
            cache_dir: safe_dir,
            cache_ttl: Duration::from_secs(cache_ttl_hours.saturating_mul(60 * 60)),
            log_cache,
        })
    }

    fn log_cache_event(&self, event: &str, kind: &str, key: &str, extra: Option<&str>) {
        if !self.log_cache {
            return;
        }
        match extra {
            Some(e) => eprintln!("[cache] {event} kind={kind} key={key} {e}"),
            None => eprintln!("[cache] {event} kind={kind} key={key}"),
        }
    }

    fn cache_file_path(&self, kind: &str, cache_path: &str) -> PathBuf {
        // indexed by cache_path (we use sha256(cache_path) as stable filename)
        let key = sha256(cache_path);
        self.cache_dir
            .join("ugg-cache")
            .join(kind)
            .join(format!("{key}.json"))
    }

    fn read_file_bytes(path: &Path) -> std::io::Result<Vec<u8>> {
        let mut f = fs::File::open(path)?;
        let mut buf = Vec::new();
        f.read_to_end(&mut buf)?;
        Ok(buf)
    }

    fn write_file_bytes(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::write(path, bytes)
    }

    fn is_fresh(&self, modified: SystemTime) -> bool {
        match SystemTime::now().duration_since(modified) {
            Ok(age) => age <= self.cache_ttl,
            Err(_) => false, // clock skew => treat as stale
        }
    }

    fn fetch_raw_body(&self, url: &str) -> Result<Vec<u8>, UggError> {
        let mut resp = self
            .agent
            .get(url)
            .call()
            .map_err(Box::new)?
            .into_body();

        let mut reader = resp.as_reader();
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf).map_err(|_| UggError::Unknown)?;
        Ok(buf)
    }

    fn get_data_cached_json<T: DeserializeOwned>(
        &self,
        url: &str,
        kind: &str,
        cache_path: &str,
    ) -> Result<T, UggError> {
        let file_path = self.cache_file_path(kind, cache_path);
        let key = sha256(cache_path);

        // Try cache
        if let Ok(meta) = fs::metadata(&file_path) {
            if let Ok(modified) = meta.modified() {
                if self.is_fresh(modified) {
                    match Self::read_file_bytes(&file_path) {
                        Ok(mut bytes) => match simd_json::serde::from_slice::<T>(&mut bytes) {
                            Ok(v) => {
                                self.log_cache_event("hit", kind, &key, None);
                                return Ok(v);
                            }
                            Err(_) => {
                                // corruption: parse failed
                                self.log_cache_event(
                                    "corruption",
                                    kind,
                                    &key,
                                    Some("cached_json_failed_to_deserialize"),
                                );
                                // fallthrough to network fetch
                            }
                        },
                        Err(_) => {
                            self.log_cache_event(
                                "corruption",
                                kind,
                                &key,
                                Some("cached_file_unreadable"),
                            );
                            // fallthrough
                        }
                    }
                } else {
                    self.log_cache_event("stale", kind, &key, None);
                }
            } else {
                self.log_cache_event("stale", kind, &key, Some("no_mtime"));
            }
        } else {
            self.log_cache_event("miss", kind, &key, None);
        }

        // Fetch from network
        let bytes = self.fetch_raw_body(url)?;
        // Best-effort write
        if let Err(e) = Self::write_file_bytes(&file_path, &bytes) {
            self.log_cache_event(
                "write_failed",
                kind,
                &key,
                Some(&format!("err={e}")),
            );
        }

        // Parse network result
        let mut bytes = bytes;
        simd_json::serde::from_slice::<T>(&mut bytes).map_err(UggError::ParseError)
    }

    // Keep your existing get_data (non-cached) for other endpoints if you want.
    fn get_data<T: DeserializeOwned>(&self, url: &str) -> Result<T, UggError> {
        simd_json::serde::from_reader::<ureq::BodyReader<'_>, T>(
            self.agent
                .get(url)
                .call()
                .map_err(Box::new)?
                .into_body()
                .as_reader(),
        )
        .map_err(UggError::ParseError)
    }

    pub fn get_current_version(&mut self) -> String {
        self.ddragon.version.clone()
    }

    pub fn get_supported_versions(&self) -> Result<Vec<String>, UggError> {
        self.get_data("https://ddragon.leagueoflegends.com/api/versions.json")
    }

    pub fn get_champ_data(&self) -> Result<HashMap<String, ChampionShort>, UggError> {
        Ok(self.ddragon.champions()?.data)
    }

    pub fn get_items(&self) -> Result<HashMap<String, Item>, UggError> {
        Ok(self.ddragon.items()?.data)
    }

    pub fn get_runes(&self) -> Result<HashMap<i64, RuneExtended<RuneElement>>, UggError> {
        let rune_data = self.ddragon.runes()?;

        let mut processed_data = HashMap::new();
        for class in rune_data {
            for (slot_index, slot) in class.slots.iter().enumerate() {
                for (index, rune) in slot.runes.iter().enumerate() {
                    let extended_rune = RuneExtended {
                        rune: (*rune).clone(),
                        slot: slot_index as u64,
                        index: index as u64,
                        siblings: slot.runes.len() as u64,
                        parent: class.name.clone(),
                        parent_id: class.id,
                    };
                    processed_data.insert(rune.id, extended_rune);
                }
            }
        }
        Ok(processed_data)
    }

    pub fn get_summoner_spells(&self) -> Result<HashMap<i64, String>, UggError> {
        let summoner_data = self.ddragon.summoner_spells()?;

        let mut reduced_data: HashMap<i64, String> = HashMap::new();
        for (_spell, spell_info) in summoner_data.data {
            reduced_data.insert(
                spell_info.key.parse::<i64>().ok().unwrap_or(0),
                spell_info.name,
            );
        }
        Ok(reduced_data)
    }

    pub fn get_arena_augments(&self) -> Result<HashMap<i64, Augment>, UggError> {
        let augment_data = self.ddragon.arena_augments()?;
        let mut reduced_data: HashMap<i64, Augment> = HashMap::new();
        for augment in augment_data {
            reduced_data.insert(augment.id, augment);
        }
        Ok(reduced_data)
    }

    pub fn get_ugg_api_versions(&self) -> Result<UggAPIVersions, UggError> {
        self.get_data::<UggAPIVersions>("https://static.bigbrain.gg/assets/lol/riot_patch_update/prod/ugg/ugg-api-versions.json")
    }

    #[allow(clippy::too_many_arguments)]
    pub fn get_stats(
        &self,
        patch: &str,
        champ: &ChampionShort,
        role: Role,
        region: Region,
        mode: Mode,
        build: Build,
        api_versions: &HashMap<String, HashMap<String, String>>,
    ) -> Result<(Overview, Role), UggError> {
        let api_version =
            if api_versions.contains_key(patch) && api_versions[patch].contains_key("overview") {
                api_versions[patch]["overview"].as_str()
            } else {
                "1.5.0"
            };
        let data_path = &format!(
            "{}/{}/{}/{}/{}",
            build.to_api_string(),
            patch,
            mode.to_api_string(),
            champ.key.as_str(),
            api_version
        );
        let cache_path = format!("{data_path}-{region}-{role}");

        let stats_url = format!("https://stats2.u.gg/lol/1.5/{data_path}.json");

        let stats_data: ChampOverview =
            self.get_data_cached_json(&stats_url, "overview", &cache_path)?;
        
        let data_by_role = Rank::preferred_order()
            .iter()
            .find_map(|rank| {
                stats_data
                    .get(&region)
                    .and_then(|region_data| region_data.get(rank))
            })
            .ok_or(UggError::MissingRegionOrRank)?;

        data_by_role
            .get_key_value(&role)
            .or_else(|| {
                data_by_role
                    .iter()
                    .max_by_key(|(_, data)| data.data.matches())
                    .map(|(role, _)| role)
                    .and_then(|r| data_by_role.get_key_value(r))
            })
            .map(|(role, data)| (data.data.clone(), *role))
            .ok_or(UggError::MissingRole)
    }

    pub fn get_matchups(
        &self,
        patch: &str,
        champ: &ChampionShort,
        role: Role,
        region: Region,
        mode: Mode,
        api_versions: &HashMap<String, HashMap<String, String>>,
    ) -> Result<(MatchupData, Role), UggError> {
        let api_version =
            if api_versions.contains_key(patch) && api_versions[patch].contains_key("matchups") {
                api_versions[patch]["matchups"].as_str()
            } else {
                "1.5.0"
            };
        let data_path = &format!(
            "{}/{}/{}/{}",
            patch,
            mode.to_api_string(),
            champ.key.as_str(),
            api_version
        );
        let cache_path = format!("{data_path}-{region}-{role}");

        let matchup_url = format!("https://stats2.u.gg/lol/1.5/matchups/{data_path}.json");

        let matchup_data: Matchups =
            self.get_data_cached_json(&matchup_url, "matchups", &cache_path)?;
            
        let data_by_role = Rank::preferred_order()
            .iter()
            .find_map(|rank| {
                matchup_data
                    .get(&region)
                    .and_then(|region_data| region_data.get(rank))
            })
            .ok_or(UggError::MissingRegionOrRank)?;

        data_by_role
            .get_key_value(&role)
            .or_else(|| {
                data_by_role
                    .iter()
                    .max_by_key(|(_, data)| data.data.total_matches)
                    .map(|(role, _)| role)
                    .and_then(|r| data_by_role.get_key_value(r))
            })
            .map(|(role, data)| (data.data.clone(), *role))
            .ok_or(UggError::MissingRole)
    }

    pub fn get_meta_summary(
        &self,
        patch: &str,                 // e.g. "16_4"
        region: Region,    // e.g. World
        mode: Mode,        // e.g. RankedSolo5x5
        rank: Rank,                  // e.g. EmeraldPlus / Diamond2Plus
        api_versions: &HashMap<String, HashMap<String, String>>,
    ) -> Result<MetaSummary, UggError> {
        // Endpoint version: stored under something like "champion_ranking" most likely.
        // Fallback to 1.5.0 like your other endpoints.
        let api_version = if api_versions.contains_key(patch)
            && api_versions[patch].contains_key("champion_ranking")
        {
            api_versions[patch]["champion_ranking"].as_str()
        } else {
            "1.5.0"
        };

        let region_str = region.to_api_string(); // "world"
        let patch_str = patch;                   // "16_4"
        let mode_str = mode.to_api_string();     // "ranked_solo_5x5"
        let rank_str = rank.to_api_string();     // "emerald_plus" / "diamond_2_plus"

        // Build the URL exactly like your examples
        let url = format!(
            "https://stats2.u.gg/lol/1.5/champion_ranking/{}/{}/{}/{}/{}.json",
            region_str, patch_str, mode_str, rank_str, api_version
        );

        // Cache key should uniquely identify data slice
        let cache_path = format!(
            "champion_ranking/{}/{}/{}/{}/{}",
            region_str, patch_str, mode_str, rank_str, api_version
        );

        self.get_data_cached_json(&url, "champion_ranking", &cache_path)
    }

}

impl UggApi {
    pub fn new(version: Option<String>, cache_dir: Option<PathBuf>) -> Result<Self, UggError> {
        Self::new_with_cache_options(version, cache_dir, 24, false)
    }

    pub fn new_with_cache_options(
        version: Option<String>,
        cache_dir: Option<PathBuf>,
        cache_ttl_hours: u64,
        log_cache: bool,
    ) -> Result<Self, UggError> {
        let mut inner_api =
            DataApi::new_with_cache_options(version, cache_dir.clone(), cache_ttl_hours, log_cache)?;

        let mut current_version = inner_api.get_current_version();
        let allowed_versions = inner_api.get_supported_versions()?;
        let ugg_api_versions = inner_api.get_ugg_api_versions()?;
        let versions_ugg_supports = allowed_versions
            .into_iter()
            .map(|v| SupportedVersion {
                ddragon: v.clone(),
                ugg: (v.split('.').take(2).collect::<Vec<&str>>()).join("_"),
            })
            .filter(|v| ugg_api_versions.contains_key(&v.ugg))
            .collect::<Vec<_>>();

        if let Some(default_if_fails) = versions_ugg_supports.first() {
            if !versions_ugg_supports
                .iter()
                .any(|v| v.ddragon == current_version)
            {
                inner_api = DataApi::new(Some(default_if_fails.ddragon.clone()), cache_dir)?;
                current_version = inner_api.get_current_version();
            }
        } else {
            return Err(UggError::Unknown);
        }

        let champ_data = inner_api.get_champ_data()?;
        let items = inner_api.get_items()?;
        let runes = inner_api.get_runes()?;
        let summoner_spells = inner_api.get_summoner_spells()?;
        let arena_augments = inner_api
            .get_arena_augments()
            .unwrap_or_else(|_| HashMap::new());

        let mut patch_version_split = current_version.split('.').collect::<Vec<&str>>();
        patch_version_split.remove(patch_version_split.len() - 1);
        let patch_version = patch_version_split.join("_");

        Ok(Self {
            api: inner_api,
            allowed_versions: versions_ugg_supports,
            api_versions: ugg_api_versions,
            current_version,
            patch_version,
            champ_data,
            items,
            runes,
            summoner_spells,
            arena_augments,
        })
    }

    pub fn find_champ(&self, name: &str) -> &ChampionShort {
        if self.champ_data.contains_key(name) {
            &self.champ_data[name]
        } else {
            let mut lowest_distance = usize::MAX;
            let mut closest_champ: &ChampionShort = &self.champ_data["Annie"];

            let mut substring_lowest_dist = usize::MAX;
            let mut substring_closest_champ: Option<&ChampionShort> = None;

            for value in self.champ_data.values() {
                let query_compare = name.to_ascii_lowercase();
                let champ_compare = value.name.to_ascii_lowercase();
                // Prefer matches where search query is an exact starting substring
                let distance = levenshtein(query_compare.as_str(), champ_compare.as_str());
                if champ_compare.starts_with(&query_compare) {
                    if distance <= substring_lowest_dist {
                        substring_lowest_dist = distance;
                        substring_closest_champ = Some(value);
                    }
                } else if distance <= lowest_distance {
                    lowest_distance = distance;
                    closest_champ = value;
                }
            }

            substring_closest_champ.unwrap_or(closest_champ)
        }
    }

    pub fn get_stats(
        &self,
        champ: &ChampionShort,
        role: Role,
        region: Region,
        mode: Mode,
        build: Build,
    ) -> Result<(Overview, Role), UggError> {
        self.api.get_stats(
            &self.patch_version,
            champ,
            role,
            region,
            mode,
            build,
            &self.api_versions,
        )
    }

    pub fn get_matchups(
        &self,
        champ: &ChampionShort,
        role: Role,
        region: Region,
        mode: Mode,
    ) -> Result<(MatchupData, Role), UggError> {
        self.api.get_matchups(
            &self.patch_version,
            champ,
            role,
            region,
            mode,
            &self.api_versions,
        )
    }
}

pub struct UggApiBuilder {
    version: Option<String>,
    cache_dir: Option<PathBuf>,
    cache_ttl_hours: u64,
    log_cache: bool,
}

impl UggApiBuilder {
    #[must_use]
    pub fn new() -> Self {
        Self {
            version: None,
            cache_dir: None,
            cache_ttl_hours: 24,
            log_cache: false,
        }
    }

    #[must_use] pub fn version(mut self, version: &str) -> Self {
        self.version = Some(version.to_owned());
        self 
    }

    #[must_use] pub fn cache_dir(mut self, cache_dir: &Path) -> Self {
        self.cache_dir = Some(cache_dir.to_path_buf()); 
        self
    }
    
    #[must_use]
    pub fn cache_ttl_hours(mut self, hours: u64) -> Self {
        self.cache_ttl_hours = hours.max(1);
        self
    }

    #[must_use]
    pub fn log_cache(mut self, enabled: bool) -> Self {
        self.log_cache = enabled;
        self
    }

    pub fn build(self) -> Result<UggApi, UggError> {
        UggApi::new_with_cache_options(
            self.version,
            self.cache_dir,
            self.cache_ttl_hours,
            self.log_cache,
        )
    }
}

impl Default for UggApiBuilder {
    fn default() -> Self {
        Self::new()
    }
}
