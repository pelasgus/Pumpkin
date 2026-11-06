use crate::entity::player::Player;
use dashmap::DashMap;
use pumpkin_data::dimension::Dimension;
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use std::sync::{Arc, Mutex};
use std::path::PathBuf;
use std::fs;
use serde::{Deserialize, Serialize};

/// The server derives `plugin` from the authenticated WASM host state.
#[derive(Clone)]
pub struct OwnedMap {
    pub plugin: String,
    pub dimension: Dimension,
}

#[derive(Serialize, Deserialize)]
struct OwnershipFile { version: u32, maps: Vec<OwnershipRecord> }

#[derive(Serialize, Deserialize)]
struct OwnershipRecord {
    id: i32,
    plugin: String,
    dimension: String,
    center_x: i32,
    center_z: i32,
    scale: i8,
}

pub struct MapManager {
    pub maps: DashMap<i32, Arc<Mutex<MapData>>>,
    /// Server-controlled plugin ownership; never inferred from item NBT.
    pub owned_maps: DashMap<i32, OwnedMap>,
    ownership_file: Option<PathBuf>,
    ownership_write_lock: Mutex<()>,
}

impl Default for MapManager {
    fn default() -> Self {
        Self::new()
    }
}

impl MapManager {
    #[must_use]
    pub fn new() -> Self {
        Self {
            maps: DashMap::new(),
            owned_maps: DashMap::new(),
            ownership_file: None,
            ownership_write_lock: Mutex::new(()),
        }
    }

    /// Load persisted plugin maps and reconstruct their map entries at startup.
    /// Invalid/corrupt persistence is an error, not an empty ownership registry.
    pub fn load_owned_maps(world_path: &std::path::Path) -> std::io::Result<(Self, i32)> {
        let file = world_path.join("data").join("pumpkin_map_ownership.json");
        let mut manager = Self::new();
        manager.ownership_file = Some(file.clone());
        if !file.exists() { return Ok((manager, 0)); }
        let bytes = fs::read(&file)?;
        let persisted: OwnershipFile = serde_json::from_slice(&bytes)
            .map_err(std::io::Error::other)?;
        if persisted.version != 1 {
            return Err(std::io::Error::other("Unsupported map ownership format"));
        }
        let mut next_id = 0i32;
        for record in persisted.maps {
            if record.id < 0 || record.plugin.is_empty() || record.scale < 0 || record.scale > 4 {
                return Err(std::io::Error::other("Invalid persisted map ownership record"));
            }
            let dimension = Dimension::from_name(&record.dimension)
                .ok_or_else(|| std::io::Error::other("Unknown persisted map dimension"))?
                .clone();
            if manager.owned_maps.contains_key(&record.id) {
                return Err(std::io::Error::other("Duplicate persisted map ID"));
            }
            next_id = next_id.max(record.id.checked_add(1)
                .ok_or_else(|| std::io::Error::other("Map ID overflow"))?);
            manager.create_map(record.id, dimension.clone(), record.center_x, record.center_z, record.scale);
            manager.owned_maps.insert(record.id, OwnedMap { plugin: record.plugin, dimension });
        }
        Ok((manager, next_id))
    }

    /// Persist registration before returning success to the WASM guest.
    /// If persistence fails, roll back the ownership record.
    pub fn register_owner(&self, id: i32, plugin: String, dimension: Dimension) -> bool {
        let _guard = self.ownership_write_lock.lock().unwrap();
        if self.get_map(id).is_none() || self.ownership_file.is_none() { return false; }
        match self.owned_maps.entry(id) {
            dashmap::mapref::entry::Entry::Vacant(entry) => {
                entry.insert(OwnedMap { plugin, dimension });
            }
            dashmap::mapref::entry::Entry::Occupied(_) => return false,
        }
        if self.persist_owned_maps().is_err() {
            self.owned_maps.remove(&id);
            return false;
        }
        true
    }

    fn persist_owned_maps(&self) -> std::io::Result<()> {
        let path = self.ownership_file.as_ref()
            .ok_or_else(|| std::io::Error::other("Map persistence is not configured"))?;
        let mut maps = Vec::new();
        for entry in &self.owned_maps {
            let map = self.get_map(*entry.key())
                .ok_or_else(|| std::io::Error::other("Owned map data missing"))?;
            let map = map.lock().map_err(|_| std::io::Error::other("Map lock poisoned"))?;
            maps.push(OwnershipRecord {
                id: *entry.key(), plugin: entry.plugin.clone(),
                dimension: entry.dimension.minecraft_name.to_string(),
                center_x: map.center_x, center_z: map.center_z, scale: map.scale,
            });
        }
        maps.sort_by_key(|m| m.id);
        let data = serde_json::to_vec_pretty(&OwnershipFile { version: 1, maps })
            .map_err(std::io::Error::other)?;
        let parent = path.parent().ok_or_else(|| std::io::Error::other("Invalid ownership path"))?;
        fs::create_dir_all(parent)?;
        let temp = path.with_extension("json.tmp");
        {
            use std::io::Write;
            let mut output = fs::File::create(&temp)?;
            output.write_all(&data)?;
            output.sync_all()?;
        }
        fs::rename(&temp, path)?;
        Ok(())
    }

    #[must_use]
    pub fn verify_owner(&self, id: i32, plugin: &str, dimension: &Dimension) -> bool {
        self.get_map(id).is_some()
            && self.owned_maps.get(&id).is_some_and(|entry| {
                entry.plugin == plugin && entry.dimension.id == dimension.id
            })
    }

    #[must_use]
    pub fn get_map(&self, id: i32) -> Option<Arc<Mutex<MapData>>> {
        self.maps.get(&id).map(|m| m.clone())
    }

    #[must_use]
    pub fn create_map(
        &self,
        id: i32,
        dimension: Dimension,
        x: i32,
        z: i32,
        scale: i8,
    ) -> Arc<Mutex<MapData>> {
        let map = Arc::new(Mutex::new(MapData::new(dimension, x, z, scale)));
        self.maps.insert(id, map.clone());
        map
    }
}

pub struct MapData {
    pub scale: i8,
    pub locked: bool,
    pub dimension: Dimension,
    pub center_x: i32,
    pub center_z: i32,
    pub colors: Box<[u8; 128 * 128]>,
    pub decorations: Vec<MapDecoration>,
    pub dirty: bool,
    pub fully_updated: bool,
}

impl MapData {
    #[must_use]
    pub fn new(dimension: Dimension, x: i32, z: i32, scale: i8) -> Self {
        Self {
            scale,
            locked: false,
            dimension,
            center_x: x,
            center_z: z,
            colors: Box::new([0; 128 * 128]),
            decorations: Vec::new(),
            dirty: true,
            fully_updated: false,
        }
    }

    pub fn set_color(&mut self, x: usize, z: usize, color: u8) {
        if x < 128 && z < 128 {
            let idx = z * 128 + x;
            if self.colors[idx] != color {
                self.colors[idx] = color;
                self.dirty = true;
            }
        }
    }

    pub fn update(&mut self, player: &Player) {
        let world = player.world();
        let scale = 1 << self.scale;
        let center_x = self.center_x;
        let center_z = self.center_z;

        let player_pos = player.position();
        let player_x = player_pos.x as i32;
        let player_z = player_pos.z as i32;

        let start_img_x = ((player_x - center_x) / scale + 64).clamp(0, 127) as usize;
        let start_img_z = ((player_z - center_z) / scale + 64).clamp(0, 127) as usize;

        let radius = 16;
        let (range_x, range_z) = if self.fully_updated {
            (
                (start_img_x.saturating_sub(radius))..(start_img_x + radius).min(128),
                (start_img_z.saturating_sub(radius))..(start_img_z + radius).min(128),
            )
        } else {
            self.fully_updated = true;
            (0..128, 0..128)
        };

        for img_x in range_x {
            let mut prev_y = -1;
            for img_z in range_z.clone() {
                let world_x = (img_x as i32 - 64) * scale + center_x;
                let world_z = (img_z as i32 - 64) * scale + center_z;

                let top_y = world.get_top_block(Vector2::new(world_x, world_z));
                let block = world.get_block(&BlockPos::new(world_x, top_y, world_z));

                let color_base = block.map_color;

                let mut brightness = 2; // Normal
                if prev_y != -1 {
                    if top_y > prev_y {
                        brightness = 3; // High
                    } else if top_y < prev_y {
                        brightness = 1; // Low
                    }
                }
                prev_y = top_y;

                let color = color_base * 4 + brightness;
                self.set_color(img_x, img_z, color);
            }
        }
    }
}

pub struct MapDecoration {
    pub icon_type: i32,
    pub x: i8,
    pub z: i8,
    pub direction: i8,
    pub display_name: Option<String>,
}
