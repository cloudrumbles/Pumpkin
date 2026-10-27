use serde::{Deserialize, Serialize};

/// What shapes the overworld's terrain.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum TerrainGenerator {
    /// Vanilla's noise router, in vanilla's 384-block overworld.
    #[default]
    Vanilla,
    /// A heightmap from terrain-diffusion, pre-generated into the world.
    TerrainDiffusion,
}

/// The world's terrain generator, an ironore addition.
#[derive(Serialize, Deserialize, Clone, Copy, Debug)]
#[serde(default)]
pub struct TerrainConfig {
    /// Which generator shapes the overworld.
    pub generator: TerrainGenerator,
    /// Terrain-diffusion world scale, 1 to 6: a block is 30 / scale metres,
    /// across and up.
    pub scale: u8,
}

impl Default for TerrainConfig {
    fn default() -> Self {
        Self {
            generator: TerrainGenerator::Vanilla,
            scale: 6,
        }
    }
}

impl TerrainConfig {
    /// The overworld's height, from y -64, or `None` for vanilla's own.
    /// Each scale fits the pipeline's 10,000 m ceiling (y = metres * scale
    /// / 30 + 63): the terrain-diffusion mod's heights for scales 1 to 5,
    /// and for 6 the tallest a dimension may be, up to y 2031.
    #[must_use]
    pub const fn overworld_height(&self) -> Option<i32> {
        match self.generator {
            TerrainGenerator::Vanilla => None,
            TerrainGenerator::TerrainDiffusion => Some(match self.scale {
                1 => 464,
                2 => 800,
                3 => 1136,
                4 => 1472,
                5 => 1808,
                _ => 2096,
            }),
        }
    }

    /// Panics when the scale is outside 1 to 6.
    pub fn validate(&self) {
        assert!(
            (1..=6).contains(&self.scale),
            "terrain.scale is {}; it must be 1 to 6",
            self.scale
        );
    }
}
