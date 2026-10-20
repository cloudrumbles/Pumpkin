//! Save format metadata for the Java release represented by the generated data.

/// Java 26.3's `world_version`, read from the official server's `version.json`.
/// All newly serialized world, entity, player, POI and structure data must use
/// this value so their format agrees with the advertised game version.
pub const DATA_VERSION: i32 = 5023;
