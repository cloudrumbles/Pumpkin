use std::{
    fs::{self, File},
    io::BufWriter,
    path::{Path, PathBuf},
};

use pumpkin_data::game_rules::{GameRule, GameRuleRegistry, GameRuleValue};
use pumpkin_nbt::{compound::NbtCompound, nbt_compress::read_gzip_compound_tag, tag::NbtTag};
use serde::{Deserialize, Serialize};
use tracing::warn;

use crate::world_info::{WorldGenSettings, WorldInfoError};

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct DataFileRoot<T> {
    #[serde(rename = "data")]
    pub data: T,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WeatherData {
    #[serde(rename = "rain_time", default)]
    pub rain_time: i32,
    #[serde(rename = "raining", default)]
    pub raining: bool,
    #[serde(rename = "thundering", default)]
    pub thundering: bool,
    #[serde(rename = "thunder_time", default)]
    pub thunder_time: i32,
    #[serde(rename = "clear_weather_time", default)]
    pub clear_weather_time: i32,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
}

impl Default for WeatherData {
    fn default() -> Self {
        Self {
            rain_time: 0,
            raining: false,
            thundering: false,
            thunder_time: 0,
            clear_weather_time: -1,
            data_version: 0,
        }
    }
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WorldGenSettingsData {
    #[serde(flatten)]
    pub settings: WorldGenSettings,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
    #[serde(rename = "bonus_chest", default)]
    pub bonus_chest: bool,
    #[serde(rename = "generate_structures", default = "default_true")]
    pub generate_structures: bool,
}

const fn default_true() -> bool {
    true
}

impl WorldGenSettingsData {
    #[must_use]
    pub const fn new(settings: WorldGenSettings, data_version: i32) -> Self {
        Self {
            settings,
            data_version,
            bonus_chest: false,
            generate_structures: true,
        }
    }
}

#[derive(Clone, PartialEq, Debug)]
pub struct DimensionClock {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
    /// Pumpkin's per-dimension age; absent in legacy/vanilla clock records.
    pub world_age: Option<i64>,
}

impl Default for DimensionClock {
    fn default() -> Self {
        Self {
            total_ticks: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
            world_age: None,
        }
    }
}

#[derive(Clone, PartialEq, Debug, Default)]
pub struct WorldClocksData {
    pub clocks: std::collections::HashMap<String, DimensionClock>,
    pub data_version: i32,
}

#[derive(Serialize, Deserialize, Clone, PartialEq, Eq, Debug)]
pub struct WanderingTraderData {
    #[serde(rename = "spawn_delay", default = "default_wandering_trader_delay")]
    pub spawn_delay: i32,
    #[serde(rename = "spawn_chance", default = "default_wandering_trader_chance")]
    pub spawn_chance: i32,
    #[serde(rename = "DataVersion", default)]
    pub data_version: i32,
}

const fn default_wandering_trader_delay() -> i32 {
    24_000
}
const fn default_wandering_trader_chance() -> i32 {
    25
}

impl Default for WanderingTraderData {
    fn default() -> Self {
        Self {
            spawn_delay: default_wandering_trader_delay(),
            spawn_chance: default_wandering_trader_chance(),
            data_version: 0,
        }
    }
}

#[must_use]
pub fn minecraft_data_dir(level_folder: &Path) -> PathBuf {
    level_folder.join("data").join("minecraft")
}

/// Ensures the `<world>/data/minecraft/` directory exists.
pub fn ensure_minecraft_data_dir(level_folder: &Path) -> Result<PathBuf, WorldInfoError> {
    let dir = minecraft_data_dir(level_folder);
    fs::create_dir_all(&dir)?;
    Ok(dir)
}

pub fn read_weather(level_folder: &Path) -> WeatherData {
    let path = minecraft_data_dir(level_folder).join("weather.dat");
    if !path.exists() {
        return WeatherData::default();
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let data_compound = compound.get_compound("data");
                let c = data_compound.as_ref().map_or(&compound, |v| v);
                WeatherData {
                    clear_weather_time: c.get_int("clear_weather_time").unwrap_or(0),
                    rain_time: c.get_int("rain_time").unwrap_or(0),
                    thunder_time: c.get_int("thunder_time").unwrap_or(0),
                    raining: c.get_bool("raining").unwrap_or(false),
                    thundering: c.get_bool("thundering").unwrap_or(false),
                    data_version: c.get_int("DataVersion").unwrap_or(0),
                }
            }
            Err(e) => {
                warn!("Failed to deserialize weather.dat, using defaults: {e}");
                WeatherData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open weather.dat, using defaults: {e}");
            WeatherData::default()
        }
    }
}

pub fn write_weather(level_folder: &Path, data: &WeatherData) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("weather.dat");
    let file = File::create(&path)?;
    let mut data_comp = NbtCompound::new();
    data_comp.put_int("clear_weather_time", data.clear_weather_time);
    data_comp.put_int("rain_time", data.rain_time);
    data_comp.put_int("thunder_time", data.thunder_time);
    data_comp.put_bool("raining", data.raining);
    data_comp.put_bool("thundering", data.thundering);
    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data.data_version);
    root.put_compound("data", data_comp);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

#[must_use]
pub fn json_to_nbt_tag(val: &serde_json::Value) -> NbtTag {
    match val {
        serde_json::Value::Null => NbtTag::End,
        serde_json::Value::Bool(b) => NbtTag::Byte(i8::from(*b)),
        serde_json::Value::Number(n) => n.as_i64().map_or_else(
            || n.as_f64().map_or(NbtTag::End, NbtTag::Double),
            |i| i32::try_from(i).map_or(NbtTag::Long(i), NbtTag::Int),
        ),
        serde_json::Value::String(s) => NbtTag::String(s.clone().into()),
        serde_json::Value::Array(arr) => NbtTag::List(arr.iter().map(json_to_nbt_tag).collect()),
        serde_json::Value::Object(map) => {
            let mut compound = NbtCompound::new();
            for (k, v) in map {
                compound.put(k, json_to_nbt_tag(v));
            }
            NbtTag::Compound(compound)
        }
    }
}

#[must_use]
pub fn nbt_tag_to_json(tag: &NbtTag) -> serde_json::Value {
    match tag {
        NbtTag::Byte(b) => serde_json::Value::Number((*b).into()),
        NbtTag::Short(s) => serde_json::Value::Number((*s).into()),
        NbtTag::Int(i) => serde_json::Value::Number((*i).into()),
        NbtTag::Long(l) => serde_json::Value::Number((*l).into()),
        NbtTag::Float(f) => serde_json::Number::from_f64(*f as f64)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        NbtTag::Double(d) => serde_json::Number::from_f64(*d)
            .map_or(serde_json::Value::Null, serde_json::Value::Number),
        NbtTag::String(s) => serde_json::Value::String(s.to_string()),
        NbtTag::List(list) => serde_json::Value::Array(list.iter().map(nbt_tag_to_json).collect()),
        NbtTag::Compound(comp) => {
            let mut map = serde_json::Map::new();
            for (k, v) in &comp.child_tags {
                map.insert(k.to_string(), nbt_tag_to_json(v));
            }
            serde_json::Value::Object(map)
        }
        NbtTag::ByteArray(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|b| serde_json::Value::Number((*b).into()))
                .collect(),
        ),
        NbtTag::IntArray(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|i| serde_json::Value::Number((*i).into()))
                .collect(),
        ),
        NbtTag::LongArray(arr) => serde_json::Value::Array(
            arr.iter()
                .map(|l| serde_json::Value::Number((*l).into()))
                .collect(),
        ),
        NbtTag::End => serde_json::Value::Null,
    }
}

#[must_use]
pub fn read_world_gen_settings(level_folder: &Path) -> Option<WorldGenSettings> {
    // Support world generation settings locations used by vanilla and Paper-derived 26.x worlds.
    let paths = [
        minecraft_data_dir(level_folder).join("world_gen_settings.dat"),
        level_folder
            .join("dimensions")
            .join("minecraft")
            .join("overworld")
            .join("data")
            .join("minecraft")
            .join("world_gen_settings.dat"),
    ];

    for path in paths.iter().filter(|path| path.exists()) {
        if let Some(settings) = read_world_gen_settings_file(path) {
            return Some(settings);
        }
    }

    None
}

fn read_world_gen_settings_file(path: &Path) -> Option<WorldGenSettings> {
    match File::open(path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let Some(c) = world_gen_settings_payload(&compound) else {
                    warn!("{} has no seed", path.display());
                    return None;
                };
                let seed = c.get_long("seed")?;
                let mut dimensions = std::collections::HashMap::new();
                if let Some(dims_comp) = c.get_compound("dimensions") {
                    for (dim_name, dim_tag) in &dims_comp.child_tags {
                        if let NbtTag::Compound(dim_c) = dim_tag {
                            let dim_type = dim_c.get_string("type").unwrap_or(dim_name).to_string();
                            if let Some(gen_c) = dim_c.get_compound("generator") {
                                let generator_type = gen_c
                                    .get_string("type")
                                    .unwrap_or("minecraft:noise")
                                    .to_string();
                                let settings = gen_c
                                    .get_string("settings")
                                    .map(|s| {
                                        crate::world_info::GeneratorSettings::Reference(
                                            s.to_string(),
                                        )
                                    })
                                    .or_else(|| {
                                        gen_c.get_compound("settings").map(|settings_c| {
                                            let json_val = nbt_tag_to_json(&NbtTag::Compound(
                                                settings_c.clone(),
                                            ));
                                            crate::world_info::GeneratorSettings::Compound(json_val)
                                        })
                                    });
                                let biome_source = gen_c.get_compound("biome_source").map(|bs_c| {
                                    let biome_type = bs_c
                                        .get_string("type")
                                        .unwrap_or("minecraft:multi_noise")
                                        .to_string();
                                    if let Some(preset) = bs_c.get_string("preset") {
                                        crate::world_info::BiomeSource::WithPreset {
                                            preset: preset.to_string(),
                                            biome_type,
                                        }
                                    } else if let Some(biome) = bs_c.get_string("biome") {
                                        crate::world_info::BiomeSource::Fixed {
                                            biome: biome.to_string(),
                                            biome_type,
                                        }
                                    } else {
                                        crate::world_info::BiomeSource::Simple { biome_type }
                                    }
                                });
                                dimensions.insert(
                                    dim_name.to_string(),
                                    crate::world_info::Dimension {
                                        generator: crate::world_info::Generator {
                                            settings,
                                            biome_source,
                                            generator_type,
                                        },
                                        dimension_type: dim_type,
                                    },
                                );
                            }
                        }
                    }
                }
                Some(WorldGenSettings { seed, dimensions })
            }
            Err(e) => {
                warn!("Failed to deserialize {}: {e}", path.display());
                None
            }
        },
        Err(e) => {
            warn!("Failed to open {}: {e}", path.display());
            None
        }
    }
}

fn world_gen_settings_payload(mut compound: &NbtCompound) -> Option<&NbtCompound> {
    loop {
        if compound.get_long("seed").is_some() {
            return Some(compound);
        }

        compound = compound
            .get_compound("data")
            .or_else(|| compound.get_compound("Data"))?;
    }
}

pub fn write_world_gen_settings(
    level_folder: &Path,
    settings: &WorldGenSettings,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("world_gen_settings.dat");
    let file = File::create(&path)?;
    let mut inner = NbtCompound::new();
    inner.put_int("DataVersion", data_version);
    inner.put_long("seed", settings.seed);
    inner.put_bool("generate_structures", true);
    inner.put_bool("bonus_chest", false);

    let mut dims_comp = NbtCompound::new();
    for (dim_name, dim) in &settings.dimensions {
        let mut dim_comp = NbtCompound::new();
        dim_comp.put_string("type", dim.dimension_type.clone());

        let mut gen_comp = NbtCompound::new();
        gen_comp.put_string("type", dim.generator.generator_type.clone());
        if let Some(s) = &dim.generator.settings {
            match s {
                crate::world_info::GeneratorSettings::Reference(r) => {
                    gen_comp.put_string("settings", r.clone());
                }
                crate::world_info::GeneratorSettings::Compound(json_val) => {
                    gen_comp.put("settings", json_to_nbt_tag(json_val));
                }
            }
        }
        if let Some(bs) = &dim.generator.biome_source {
            let mut bs_comp = NbtCompound::new();
            match bs {
                crate::world_info::BiomeSource::WithPreset { preset, biome_type } => {
                    bs_comp.put_string("preset", preset.clone());
                    bs_comp.put_string("type", biome_type.clone());
                }
                crate::world_info::BiomeSource::Fixed { biome, biome_type } => {
                    bs_comp.put_string("biome", biome.clone());
                    bs_comp.put_string("type", biome_type.clone());
                }
                crate::world_info::BiomeSource::Simple { biome_type } => {
                    bs_comp.put_string("type", biome_type.clone());
                }
            }
            gen_comp.put_compound("biome_source", bs_comp);
        }
        dim_comp.put_compound("generator", gen_comp);
        dims_comp.put_compound(dim_name, dim_comp);
    }
    inner.put_compound("dimensions", dims_comp);

    let mut root = NbtCompound::new();
    root.put_compound("data", inner);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

#[must_use]
pub fn game_rules_to_nbt(rules: &GameRuleRegistry, data_version: i32) -> NbtCompound {
    let mut inner = NbtCompound::new();
    for rule in GameRule::all() {
        let key = format!("minecraft:{rule}");
        match rules.get(rule) {
            GameRuleValue::Bool(b) => inner.put(&key, NbtTag::Byte(i8::from(*b))),
            GameRuleValue::Int(i) => inner.put(&key, NbtTag::Int(*i as i32)),
        }
    }
    inner.put_int("DataVersion", data_version);

    let mut root = NbtCompound::new();
    root.put_compound("data", inner);
    root
}

pub fn game_rules_from_nbt(root: &NbtCompound) -> GameRuleRegistry {
    let mut registry = GameRuleRegistry::default();

    let Some(inner) = root.get_compound("data") else {
        warn!("game_rules.dat missing 'data' compound, using defaults");
        return registry;
    };

    for rule in GameRule::all() {
        let key = format!("minecraft:{rule}");
        match registry.get_mut(rule) {
            GameRuleValue::Bool(b) => {
                if let Some(v) = inner.get_byte(&key) {
                    *b = v != 0;
                }
            }
            GameRuleValue::Int(i) => {
                if let Some(v) = inner.get_int(&key) {
                    *i = i64::from(v);
                }
            }
        }
    }

    registry
}

pub fn read_game_rules(level_folder: &Path) -> GameRuleRegistry {
    let path = minecraft_data_dir(level_folder).join("game_rules.dat");
    if !path.exists() {
        return GameRuleRegistry::default();
    }

    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => game_rules_from_nbt(&compound),
            Err(e) => {
                warn!("Failed to parse game_rules.dat: {e}");
                GameRuleRegistry::default()
            }
        },
        Err(e) => {
            warn!("Failed to open game_rules.dat: {e}");
            GameRuleRegistry::default()
        }
    }
}

pub fn write_game_rules(
    level_folder: &Path,
    rules: &GameRuleRegistry,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("game_rules.dat");

    let compound = game_rules_to_nbt(rules, data_version);
    let file = File::create(&path)?;

    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(compound, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

/// Compatibility reader for callers that already tolerate missing/corrupt metadata.
/// Runtime clock restoration and saving must use the strict variant below.
pub fn read_world_clocks(level_folder: &Path) -> WorldClocksData {
    read_world_clocks_strict(level_folder).unwrap_or_else(|error| {
        warn!("Failed to read world_clocks.dat: {error}");
        WorldClocksData::default()
    })
}

fn read_clock_root(path: &Path) -> Result<NbtCompound, WorldInfoError> {
    match File::open(path) {
        Ok(file) => read_gzip_compound_tag(file)
            .map_err(|error| WorldInfoError::DeserializationError(error.to_string())),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            let mut root = NbtCompound::new();
            root.put_compound("data", NbtCompound::new());
            Ok(root)
        }
        Err(error) => Err(error.into()),
    }
}

/// Missing files are new-world defaults. Existing malformed files are errors.
pub fn read_world_clocks_strict(level_folder: &Path) -> Result<WorldClocksData, WorldInfoError> {
    let path = minecraft_data_dir(level_folder).join("world_clocks.dat");
    world_clocks_from_nbt(&read_clock_root(&path)?)
}

/// Parse only the requested dimension, so a malformed unloaded record does not
/// prevent an otherwise valid world from loading.
pub fn read_dimension_clock(
    level_folder: &Path,
    dimension: &str,
) -> Result<Option<DimensionClock>, WorldInfoError> {
    let path = minecraft_data_dir(level_folder).join("world_clocks.dat");
    let root = read_clock_root(&path)?;
    let inner = clock_data(&root)?;
    inner
        .get(dimension)
        .map(|tag| parse_dimension_clock(dimension, tag))
        .transpose()
}

fn clock_data(root: &NbtCompound) -> Result<&NbtCompound, WorldInfoError> {
    root.get_compound("data").ok_or_else(|| {
        WorldInfoError::DeserializationError("world_clocks.dat has no data compound".into())
    })
}

fn malformed_clock(dimension: &str) -> WorldInfoError {
    WorldInfoError::DeserializationError(format!("Malformed world clock {dimension}"))
}

fn validate_dimension_clock(dimension: &str, clock: &DimensionClock) -> Result<(), WorldInfoError> {
    if clock.partial_tick.is_finite() && clock.rate.is_finite() && clock.rate > 0.0 {
        Ok(())
    } else {
        Err(malformed_clock(dimension))
    }
}

fn parse_dimension_clock(dimension: &str, tag: &NbtTag) -> Result<DimensionClock, WorldInfoError> {
    let NbtTag::Compound(clock) = tag else {
        return Err(malformed_clock(dimension));
    };
    let malformed = || malformed_clock(dimension);
    let total_ticks = clock.get_long("total_ticks").ok_or_else(malformed)?;
    let partial_tick = if clock.has("partial_tick") {
        clock.get_float("partial_tick").ok_or_else(malformed)?
    } else {
        0.0
    };
    let rate = if clock.has("rate") {
        clock.get_float("rate").ok_or_else(malformed)?
    } else {
        1.0
    };
    let paused = if clock.has("paused") {
        clock.get_bool("paused").ok_or_else(malformed)?
    } else {
        false
    };
    let world_age = if clock.has("pumpkin:world_age") {
        Some(clock.get_long("pumpkin:world_age").ok_or_else(malformed)?)
    } else {
        None
    };
    let clock = DimensionClock {
        total_ticks,
        partial_tick,
        rate,
        paused,
        world_age,
    };
    validate_dimension_clock(dimension, &clock)?;
    Ok(clock)
}

fn world_clocks_from_nbt(root: &NbtCompound) -> Result<WorldClocksData, WorldInfoError> {
    let inner = clock_data(root)?;
    let mut result = WorldClocksData {
        data_version: root
            .get_int("DataVersion")
            .or_else(|| inner.get_int("DataVersion"))
            .unwrap_or(0),
        ..WorldClocksData::default()
    };

    for (key, tag) in &inner.child_tags {
        if let NbtTag::Compound(clock) = tag {
            // Preserve unrelated compounds without treating them as clock records.
            if !clock.has("total_ticks") {
                continue;
            }
            result
                .clocks
                .insert(key.to_string(), parse_dimension_clock(key, tag)?);
        }
    }
    Ok(result)
}

/// The caller serializes read/modify/write for all dimensions sharing this root.
/// Merge into the old NBT, preserving unknown root, data, and clock fields.
pub fn write_world_clocks(
    level_folder: &Path,
    clocks: &WorldClocksData,
) -> Result<(), WorldInfoError> {
    for (dimension, clock) in &clocks.clocks {
        validate_dimension_clock(dimension, clock)?;
    }
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("world_clocks.dat");
    let path_new = dir.join("world_clocks.dat_new");
    let mut root = read_clock_root(&path)?;
    // Refuse to replace a malformed file with defaults.
    world_clocks_from_nbt(&root)?;
    let mut inner = root.get_compound("data").cloned().unwrap_or_default();
    for (dim_name, clock) in &clocks.clocks {
        if inner.child_tags.contains_key(dim_name.as_str())
            && !inner
                .get_compound(dim_name)
                .is_some_and(|saved| saved.get_long("total_ticks").is_some())
        {
            return Err(WorldInfoError::DeserializationError(format!(
                "Refusing to overwrite malformed world clock {dim_name}"
            )));
        }
        let mut dim_compound = inner.get_compound(dim_name).cloned().unwrap_or_default();
        dim_compound.put_long("total_ticks", clock.total_ticks);
        dim_compound.put_float("partial_tick", clock.partial_tick);
        dim_compound.put_float("rate", clock.rate);
        dim_compound.put_bool("paused", clock.paused);
        if let Some(world_age) = clock.world_age {
            dim_compound.put_long("pumpkin:world_age", world_age);
        }
        inner.put_compound(dim_name, dim_compound);
    }
    // SavedData stores DataVersion at the root. Normalize Pumpkin's older
    // inner placement when that legacy layout is encountered.
    inner.child_tags.remove("DataVersion");
    root.put_int("DataVersion", clocks.data_version);
    root.put_compound("data", inner);

    let mut file = File::create(&path_new)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, &mut file)
        .map_err(|error| WorldInfoError::SerializationError(error.to_string()))?;
    file.sync_all()?;
    drop(file);
    fs::rename(path_new, path)?;
    Ok(())
}

pub fn read_wandering_trader(level_folder: &Path) -> WanderingTraderData {
    let path = minecraft_data_dir(level_folder).join("wandering_trader.dat");
    if !path.exists() {
        return WanderingTraderData::default();
    }
    match File::open(&path) {
        Ok(f) => match read_gzip_compound_tag(f) {
            Ok(compound) => {
                let data_compound = compound.get_compound("data");
                let c = data_compound.as_ref().map_or(&compound, |v| v);
                let data_version = compound
                    .get_int("DataVersion")
                    .or_else(|| c.get_int("DataVersion"))
                    .unwrap_or(0);
                WanderingTraderData {
                    spawn_delay: c
                        .get_int("spawn_delay")
                        .or_else(|| c.get_int("WanderingTraderSpawnDelay"))
                        .unwrap_or(24_000),
                    spawn_chance: c
                        .get_int("spawn_chance")
                        .or_else(|| c.get_int("WanderingTraderSpawnChance"))
                        .unwrap_or(25),
                    data_version,
                }
            }
            Err(e) => {
                warn!("Failed to deserialize wandering_trader.dat, using defaults: {e}");
                WanderingTraderData::default()
            }
        },
        Err(e) => {
            warn!("Failed to open wandering_trader.dat, using defaults: {e}");
            WanderingTraderData::default()
        }
    }
}

pub fn write_wandering_trader(
    level_folder: &Path,
    data: &WanderingTraderData,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("wandering_trader.dat");
    let file = File::create(&path)?;
    let mut data_comp = NbtCompound::new();
    data_comp.put_int("spawn_delay", data.spawn_delay);
    data_comp.put_int("spawn_chance", data.spawn_chance);
    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data.data_version);
    root.put_compound("data", data_comp);
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, BufWriter::new(file))
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_custom_boss_events_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("custom_boss_events.dat");
    if path.exists() {
        return Ok(());
    }

    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data_version);
    root.put_compound("data", NbtCompound::new());

    let file = File::create(&path)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_scheduled_events_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("scheduled_events.dat");
    if path.exists() {
        return Ok(());
    }

    let mut inner = NbtCompound::new();
    inner.put("events", NbtTag::List(vec![]));
    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data_version);
    root.put_compound("data", inner);

    let file = File::create(&path)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_random_sequences_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("random_sequences.dat");
    if path.exists() {
        return Ok(());
    }

    let mut inner = NbtCompound::new();
    inner.put_int("salt", 0);
    inner.put_compound("sequences", NbtCompound::new());
    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data_version);
    root.put_compound("data", inner);

    let file = File::create(&path)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_scoreboard_stub(level_folder: &Path, data_version: i32) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("scoreboard.dat");
    if path.exists() {
        return Ok(());
    }

    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data_version);
    root.put_compound("data", NbtCompound::new());

    let file = File::create(&path)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

pub fn write_stopwatches_stub(
    level_folder: &Path,
    data_version: i32,
) -> Result<(), WorldInfoError> {
    let dir = ensure_minecraft_data_dir(level_folder)?;
    let path = dir.join("stopwatches.dat");
    if path.exists() {
        return Ok(());
    }

    let mut inner = NbtCompound::new();
    inner.put_compound("stopwatches", NbtCompound::new());
    let mut root = NbtCompound::new();
    root.put_int("DataVersion", data_version);
    root.put_compound("data", inner);

    let file = File::create(&path)?;
    pumpkin_nbt::nbt_compress::write_gzip_compound_tag(root, file)
        .map_err(|e| WorldInfoError::SerializationError(e.to_string()))
}

#[cfg(test)]
mod world_clock_tests {
    use super::*;
    use pumpkin_nbt::nbt_compress::{read_gzip_compound_tag, write_gzip_compound_tag};
    use tempfile::TempDir;

    fn clock_path(level_folder: &Path) -> PathBuf {
        minecraft_data_dir(level_folder).join("world_clocks.dat")
    }

    fn write_clock_root(level_folder: &Path, root: NbtCompound) {
        let data_dir = ensure_minecraft_data_dir(level_folder).unwrap();
        write_gzip_compound_tag(
            root,
            File::create(data_dir.join("world_clocks.dat")).unwrap(),
        )
        .unwrap();
    }

    fn read_clock_root(level_folder: &Path) -> NbtCompound {
        read_gzip_compound_tag(File::open(clock_path(level_folder)).unwrap()).unwrap()
    }

    fn complete_clock(total_ticks: i64) -> DimensionClock {
        DimensionClock {
            total_ticks,
            partial_tick: 0.75,
            rate: 0.5,
            paused: true,
            world_age: Some(98_765),
        }
    }

    #[test]
    fn world_clock_round_trip_preserves_runtime_and_unmanaged_nbt() {
        let temp_dir = TempDir::new().unwrap();
        let mut overworld = NbtCompound::new();
        overworld.put_long("total_ticks", 6_000);
        overworld.put_float("partial_tick", 0.25);
        overworld.put_float("rate", 2.0);
        overworld.put_bool("paused", false);
        overworld.put_long("pumpkin:world_age", 42_000);
        overworld.put_string("clock_sentinel", "keep-clock".to_string());

        let mut unloaded = NbtCompound::new();
        unloaded.put_long("total_ticks", 19_000);
        unloaded.put_string("unloaded_sentinel", "keep-unloaded".to_string());

        let mut data = NbtCompound::new();
        data.put_int("DataVersion", 4_902);
        data.put_string("data_sentinel", "keep-data".to_string());
        data.put_compound("minecraft:overworld", overworld);
        data.put_compound("example:unloaded", unloaded);

        let mut root = NbtCompound::new();
        root.put_int("DataVersion", 4_902);
        root.put_string("root_sentinel", "keep-root".to_string());
        root.put_compound("data", data);
        write_clock_root(temp_dir.path(), root);

        let mut clocks = read_world_clocks_strict(temp_dir.path()).unwrap();
        assert_eq!(
            clocks.clocks.get("minecraft:overworld"),
            Some(&DimensionClock {
                total_ticks: 6_000,
                partial_tick: 0.25,
                rate: 2.0,
                paused: false,
                world_age: Some(42_000),
            })
        );
        clocks.data_version = 4_903;
        clocks
            .clocks
            .insert("minecraft:overworld".to_string(), complete_clock(7_000));
        write_world_clocks(temp_dir.path(), &clocks).unwrap();

        let root = read_clock_root(temp_dir.path());
        assert_eq!(root.get_int("DataVersion"), Some(4_903));
        assert_eq!(root.get_string("root_sentinel"), Some("keep-root"));
        let data = root.get_compound("data").unwrap();
        assert_eq!(data.get_int("DataVersion"), None);
        assert_eq!(data.get_string("data_sentinel"), Some("keep-data"));
        let overworld = data.get_compound("minecraft:overworld").unwrap();
        assert_eq!(overworld.get_long("total_ticks"), Some(7_000));
        assert_eq!(overworld.get_float("partial_tick"), Some(0.75));
        assert_eq!(overworld.get_float("rate"), Some(0.5));
        assert_eq!(overworld.get_bool("paused"), Some(true));
        assert_eq!(overworld.get_long("pumpkin:world_age"), Some(98_765));
        assert_eq!(overworld.get_string("clock_sentinel"), Some("keep-clock"));
        assert_eq!(
            data.get_compound("example:unloaded")
                .unwrap()
                .get_string("unloaded_sentinel"),
            Some("keep-unloaded")
        );
        assert!(
            !minecraft_data_dir(temp_dir.path())
                .join("world_clocks.dat_new")
                .exists()
        );
    }

    #[test]
    fn legacy_clock_fields_receive_compatible_defaults() {
        let temp_dir = TempDir::new().unwrap();
        let mut legacy_clock = NbtCompound::new();
        legacy_clock.put_long("total_ticks", 12_345);
        let mut data = NbtCompound::new();
        data.put_compound("minecraft:overworld", legacy_clock);
        let mut root = NbtCompound::new();
        root.put_compound("data", data);
        write_clock_root(temp_dir.path(), root);

        assert_eq!(
            read_dimension_clock(temp_dir.path(), "minecraft:overworld").unwrap(),
            Some(DimensionClock {
                total_ticks: 12_345,
                partial_tick: 0.0,
                rate: 1.0,
                paused: false,
                world_age: None,
            })
        );
    }

    #[test]
    fn active_clock_read_does_not_parse_an_unloaded_malformed_clock() {
        let temp_dir = TempDir::new().unwrap();
        let mut overworld = NbtCompound::new();
        overworld.put_long("total_ticks", 1_000);
        let mut malformed_nether = NbtCompound::new();
        malformed_nether.put_long("total_ticks", 2_000);
        malformed_nether.put_string("rate", "invalid".to_string());
        let mut data = NbtCompound::new();
        data.put_compound("minecraft:overworld", overworld);
        data.put_compound("minecraft:the_nether", malformed_nether);
        let mut root = NbtCompound::new();
        root.put_compound("data", data);
        write_clock_root(temp_dir.path(), root);

        let overworld = read_dimension_clock(temp_dir.path(), "minecraft:overworld")
            .unwrap()
            .unwrap();
        assert_eq!(overworld.total_ticks, 1_000);
        assert!(read_dimension_clock(temp_dir.path(), "minecraft:the_nether").is_err());
    }

    #[test]
    fn invalid_runtime_clock_is_rejected_without_replacing_saved_data() {
        let temp_dir = TempDir::new().unwrap();
        let mut clocks = WorldClocksData::default();
        clocks
            .clocks
            .insert("minecraft:overworld".to_string(), complete_clock(6_000));
        write_world_clocks(temp_dir.path(), &clocks).unwrap();
        let original = fs::read(clock_path(temp_dir.path())).unwrap();

        for invalid_rate in [f32::INFINITY, f32::NAN, 0.0, -1.0] {
            clocks.clocks.get_mut("minecraft:overworld").unwrap().rate = invalid_rate;
            assert!(write_world_clocks(temp_dir.path(), &clocks).is_err());
            assert_eq!(fs::read(clock_path(temp_dir.path())).unwrap(), original);
        }
        let clock = clocks.clocks.get_mut("minecraft:overworld").unwrap();
        clock.rate = 1.0;
        clock.partial_tick = f32::NAN;
        assert!(write_world_clocks(temp_dir.path(), &clocks).is_err());
        assert_eq!(fs::read(clock_path(temp_dir.path())).unwrap(), original);
    }

    #[test]
    fn malformed_saved_clock_is_an_error_and_is_not_replaced() {
        let temp_dir = TempDir::new().unwrap();
        let mut malformed = NbtCompound::new();
        malformed.put_string("total_ticks", "invalid".to_string());
        let mut data = NbtCompound::new();
        data.put_compound("minecraft:overworld", malformed);
        let mut root = NbtCompound::new();
        root.put_compound("data", data);
        write_clock_root(temp_dir.path(), root);
        let original = fs::read(clock_path(temp_dir.path())).unwrap();

        assert!(read_dimension_clock(temp_dir.path(), "minecraft:overworld").is_err());
        let mut replacement = WorldClocksData::default();
        replacement
            .clocks
            .insert("minecraft:overworld".to_string(), complete_clock(6_000));
        assert!(write_world_clocks(temp_dir.path(), &replacement).is_err());
        assert_eq!(fs::read(clock_path(temp_dir.path())).unwrap(), original);
    }

    #[test]
    fn malformed_clock_file_is_not_replaced() {
        let temp_dir = TempDir::new().unwrap();
        let data_dir = ensure_minecraft_data_dir(temp_dir.path()).unwrap();
        let path = data_dir.join("world_clocks.dat");
        fs::write(&path, b"not gzip nbt").unwrap();
        let original = fs::read(&path).unwrap();

        assert!(read_world_clocks_strict(temp_dir.path()).is_err());
        let mut replacement = WorldClocksData::default();
        replacement
            .clocks
            .insert("minecraft:overworld".to_string(), complete_clock(6_000));
        assert!(write_world_clocks(temp_dir.path(), &replacement).is_err());
        assert_eq!(fs::read(path).unwrap(), original);
    }
}
