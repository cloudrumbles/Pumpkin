use super::{ChunkLevel, ChunkPos, HashMapType};
use crate::chunk_system::chunk_state::StagedChunkEnum;
use crate::level::Level;
use std::collections::hash_map::Entry;
use std::sync::atomic::Ordering::SeqCst;
use std::sync::{Arc, Condvar, Mutex};

pub type LevelChange = (
    HashMapType<ChunkPos, (StagedChunkEnum, StagedChunkEnum)>,
    ChunkLevel,
);

/// Foreground ticket positions and the lowest foreground level at each.
pub type ForegroundSources = Vec<(ChunkPos, i8)>;

/// Everything the ticket side has changed since the scheduler last looked.
#[derive(Default)]
pub struct LevelUpdate {
    pub level: Option<LevelChange>,
    pub high_priority: Option<Vec<ChunkPos>>,
    pub foreground: Option<ForegroundSources>,
}

impl LevelUpdate {
    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.level.is_none() && self.high_priority.is_none() && self.foreground.is_none()
    }
}

pub struct LevelChannel {
    pub value: Mutex<LevelUpdate>,
    pub notify: Condvar,
}

impl Default for LevelChannel {
    fn default() -> Self {
        Self::new()
    }
}

impl LevelChannel {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            value: Mutex::new(LevelUpdate {
                level: None,
                high_priority: None,
                foreground: None,
            }),
            notify: Condvar::new(),
        }
    }
    pub fn set_both(
        &self,
        new_value: (
            HashMapType<ChunkPos, (StagedChunkEnum, StagedChunkEnum)>,
            ChunkLevel,
        ),
        pos: Vec<ChunkPos>,
    ) {
        // debug!("set new level and priority");
        let mut value = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        value.high_priority = Some(pos);
        if let Some(old) = &mut value.level {
            for (pos, change) in new_value.0 {
                match old.0.entry(pos) {
                    Entry::Occupied(mut entry) => {
                        let tmp = entry.get_mut();
                        debug_assert_eq!(tmp.1, change.0);
                        if tmp.0 == change.1 {
                            entry.remove();
                        } else {
                            tmp.1 = change.1;
                        }
                    }
                    Entry::Vacant(entry) => {
                        debug_assert_ne!(change.0, change.1);
                        entry.insert(change);
                    }
                }
            }
            old.1 = new_value.1;
        } else {
            value.level = Some(new_value);
        }
        self.notify.notify_one();
    }
    pub fn set_level(
        &self,
        new_value: (
            HashMapType<ChunkPos, (StagedChunkEnum, StagedChunkEnum)>,
            ChunkLevel,
        ),
    ) {
        // debug!("set new level");
        let mut value = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(old) = &mut value.level {
            for (pos, change) in new_value.0 {
                match old.0.entry(pos) {
                    Entry::Occupied(mut entry) => {
                        let tmp = entry.get_mut();
                        debug_assert_eq!(tmp.1, change.0);
                        if tmp.0 == change.1 {
                            entry.remove();
                        } else {
                            tmp.1 = change.1;
                        }
                    }
                    Entry::Vacant(entry) => {
                        debug_assert_ne!(change.0, change.1);
                        entry.insert(change);
                    }
                }
            }
            old.1 = new_value.1;
        } else {
            value.level = Some(new_value);
        }
        self.notify.notify_one();
    }
    pub fn set_priority(&self, pos: Vec<ChunkPos>) {
        // debug!("set new priority");
        self.value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .high_priority = Some(pos);
        self.notify.notify_one();
    }
    pub fn set_foreground(&self, sources: ForegroundSources) {
        self.value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .foreground = Some(sources);
        self.notify.notify_one();
    }
    pub fn get(&self) -> LevelUpdate {
        let mut lock = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        std::mem::take(&mut *lock)
    }
    pub fn wait_and_get(&self, level: &Arc<Level>) -> LevelUpdate {
        let mut lock = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        while lock.is_empty()
            && !level.should_unload.load(SeqCst)
            && !level.should_save.load(SeqCst)
            && !level.shut_down_chunk_system.load(SeqCst)
        {
            let (new_lock, timeout_res) = self
                .notify
                .wait_timeout(lock, std::time::Duration::from_secs(1))
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            lock = new_lock;
            if timeout_res.timed_out() {
                break;
            }
        }
        std::mem::take(&mut *lock)
    }
    pub fn notify(&self) {
        let val = self
            .value
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        drop(val);
        self.notify.notify_one();
    }
}
