use pumpkin_protocol::{bedrock::client::set_time::CSetTime, java::client::play::CUpdateTime};

use super::World;

#[derive(Clone, Debug, PartialEq)]
pub struct ClockInstance {
    pub total_ticks: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for ClockInstance {
    fn default() -> Self {
        Self::new()
    }
}

impl ClockInstance {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            total_ticks: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }

    pub const fn load_from(
        &mut self,
        total_ticks: i64,
        partial_tick: f32,
        rate: f32,
        paused: bool,
    ) {
        self.total_ticks = total_ticks;
        self.partial_tick = partial_tick;
        self.rate = rate;
        self.paused = paused;
    }

    pub fn tick(&mut self) {
        if !self.paused {
            self.partial_tick += self.rate;
            let full_ticks = self.partial_tick.floor() as i32;
            self.partial_tick -= full_ticks as f32;
            self.total_ticks += full_ticks as i64;
        }
    }

    pub const fn set_total_ticks(&mut self, total_ticks: i64) {
        self.total_ticks = total_ticks;
        self.partial_tick = 0.0;
    }

    pub fn add_ticks(&mut self, ticks: i64) {
        self.total_ticks = (self.total_ticks + ticks).max(0);
    }

    pub const fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub const fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
    }

    #[must_use]
    pub const fn pack_network_state(&self, advance_time: bool) -> (i64, f32, f32) {
        let paused = self.paused || !advance_time;
        let rate = if paused { 0.0 } else { self.rate };
        (self.total_ticks, self.partial_tick, rate)
    }
}

#[derive(Clone, Debug)]
pub struct LevelTime {
    pub time_of_day: i64,
    pub world_age: i64,
    pub partial_tick: f32,
    pub rate: f32,
    pub paused: bool,
}

impl Default for LevelTime {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelTime {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            time_of_day: 0,
            world_age: 0,
            partial_tick: 0.0,
            rate: 1.0,
            paused: false,
        }
    }

    pub const fn load_from(&mut self, time_of_day: i64, world_age: i64) {
        self.time_of_day = time_of_day;
        self.world_age = world_age;
    }

    pub fn tick(&mut self, advance_time: bool) {
        self.world_age += 1;
        if advance_time && !self.paused {
            self.partial_tick += self.rate;
            let full_ticks = self.partial_tick.floor() as i32;
            self.partial_tick -= full_ticks as f32;
            self.time_of_day += full_ticks as i64;
        }
    }

    pub fn send_time(&self, world: &World) {
        let advance_time = {
            let lock = world.level_info.load();
            lock.game_rules.advance_time
        };

        let (total_ticks, partial_tick, rate) = self.pack_network_state(advance_time);

        world.broadcast_editioned(
            &CUpdateTime::new_clock(self.world_age, 0, total_ticks, partial_tick, rate),
            &CSetTime::new(self.time_of_day as _), // TODO do we need to tell bedrock that time is frozen?
        );
    }

    pub fn add_time(&mut self, time: i64) {
        self.time_of_day = (self.time_of_day + time).max(0);
    }

    pub const fn set_time(&mut self, time: i64) {
        self.time_of_day = time;
        self.partial_tick = 0.0;
    }

    pub const fn set_paused(&mut self, paused: bool) {
        self.paused = paused;
    }

    pub const fn set_rate(&mut self, rate: f32) {
        self.rate = rate;
    }

    #[must_use]
    pub const fn pack_network_state(&self, advance_time: bool) -> (i64, f32, f32) {
        let paused = self.paused || !advance_time;
        let rate = if paused { 0.0 } else { self.rate };
        (self.time_of_day, self.partial_tick, rate)
    }

    #[must_use]
    pub const fn query_daytime(&self) -> i64 {
        self.time_of_day % 24000
    }

    #[must_use]
    pub const fn query_gametime(&self) -> i64 {
        self.world_age
    }

    #[must_use]
    pub const fn query_day(&self) -> i64 {
        self.time_of_day / 24000
    }

    #[must_use]
    pub const fn is_night(&self) -> bool {
        (self.time_of_day % 24000) >= 12000 && (self.time_of_day % 24000) <= 23999
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_nbt::{NbtCompound, nbt_compress::write_gzip_compound_tag};
    use pumpkin_world::world_info::data_files::{
        DimensionClock, WorldClocksData, write_world_clocks,
    };
    use std::{fs, fs::File};
    use tempfile::TempDir;

    fn write_legacy_level_time(level_folder: &Path, day_time: i64, world_age: i64) {
        let mut data = NbtCompound::new();
        data.put_long("DayTime", day_time);
        data.put_long("Time", world_age);
        let mut root = NbtCompound::new();
        root.put_compound("Data", data);
        write_gzip_compound_tag(root, File::create(level_folder.join("level.dat")).unwrap())
            .unwrap();
    }

    fn write_clock(level_folder: &Path, clock: DimensionClock) {
        let mut clocks = WorldClocksData::default();
        clocks
            .clocks
            .insert("minecraft:overworld".to_string(), clock);
        write_world_clocks(level_folder, &clocks).unwrap();
    }

    #[test]
    fn clock_instance_ticking() {
        let mut clock = ClockInstance::new();
        assert_eq!(clock.total_ticks, 0);
        assert_eq!(clock.partial_tick, 0.0);
        assert_eq!(clock.rate, 1.0);
        assert!(!clock.paused);

        // Standard tick at rate 1.0
        clock.tick();
        assert_eq!(clock.total_ticks, 1);
        assert_eq!(clock.partial_tick, 0.0);

        // Half rate tick
        clock.set_rate(0.5);
        clock.tick();
        assert_eq!(clock.total_ticks, 1);
        assert_eq!(clock.partial_tick, 0.5);
        clock.tick();
        assert_eq!(clock.total_ticks, 2);
        assert_eq!(clock.partial_tick, 0.0);

        // Double rate tick
        clock.set_rate(2.0);
        clock.tick();
        assert_eq!(clock.total_ticks, 4);
        assert_eq!(clock.partial_tick, 0.0);

        // Paused clock
        clock.set_paused(true);
        clock.tick();
        assert_eq!(clock.total_ticks, 4);
    }

    #[test]
    fn level_time_set_and_add() {
        let mut time = LevelTime::new();
        time.set_time(1000);
        assert_eq!(time.time_of_day, 1000);
        assert_eq!(time.partial_tick, 0.0);

        time.add_time(500);
        assert_eq!(time.time_of_day, 1500);

        time.add_time(-2000);
        assert_eq!(time.time_of_day, 0);
    }

    #[test]
    fn complete_dimension_clock_does_not_depend_on_legacy_level_dat() {
        let temp_dir = TempDir::new().unwrap();
        write_clock(
            temp_dir.path(),
            DimensionClock {
                total_ticks: 6_000,
                partial_tick: 0.75,
                rate: 0.5,
                paused: true,
                world_age: Some(90_000),
            },
        );
        fs::write(
            temp_dir.path().join("level.dat"),
            b"corrupt legacy metadata",
        )
        .unwrap();

        let restored = LevelTime::load(temp_dir.path(), "minecraft:overworld").unwrap();
        assert_eq!(restored.time_of_day, 6_000);
        assert_eq!(restored.world_age, 90_000);
        assert_eq!(restored.partial_tick, 0.75);
        assert_eq!(restored.rate, 0.5);
        assert!(restored.paused);
    }

    #[test]
    fn absent_clock_uses_legacy_overworld_time_and_world_age() {
        let temp_dir = TempDir::new().unwrap();
        write_legacy_level_time(temp_dir.path(), 7_000, 123_456);

        let overworld = LevelTime::load(temp_dir.path(), "minecraft:overworld").unwrap();
        assert_eq!(overworld.time_of_day, 7_000);
        assert_eq!(overworld.world_age, 123_456);
        assert_eq!(overworld.partial_tick, 0.0);
        assert_eq!(overworld.rate, 1.0);
        assert!(!overworld.paused);

        let nether = LevelTime::load(temp_dir.path(), "minecraft:the_nether").unwrap();
        assert_eq!(nether.time_of_day, 0);
        assert_eq!(nether.world_age, 123_456);
    }

    #[test]
    fn legacy_dimension_clock_uses_level_dat_only_for_missing_world_age() {
        let temp_dir = TempDir::new().unwrap();
        write_legacy_level_time(temp_dir.path(), 7_000, 123_456);
        write_clock(
            temp_dir.path(),
            DimensionClock {
                total_ticks: 18_000,
                partial_tick: 0.0,
                rate: 1.0,
                paused: false,
                world_age: None,
            },
        );

        let restored = LevelTime::load(temp_dir.path(), "minecraft:overworld").unwrap();
        assert_eq!(restored.time_of_day, 18_000);
        assert_eq!(restored.world_age, 123_456);
    }

    #[test]
    fn new_world_without_clock_metadata_uses_defaults() {
        let temp_dir = TempDir::new().unwrap();

        let restored = LevelTime::load(temp_dir.path(), "minecraft:overworld").unwrap();
        assert_eq!(restored.time_of_day, 0);
        assert_eq!(restored.world_age, 0);
        assert_eq!(restored.partial_tick, 0.0);
        assert_eq!(restored.rate, 1.0);
        assert!(!restored.paused);
    }
}
