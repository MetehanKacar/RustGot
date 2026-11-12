//! The read access mob spawn rules need, shared by the live world and world generation.
//!
//! Vanilla runs `checkSpawnRules` against a `LevelAccessor`: the `ServerLevel` for natural
//! spawning and a `WorldGenRegion` while a chunk is being generated. Reading the live world
//! during generation sees a chunk that is not loaded yet, so every rule failed and new chunks
//! got no animals.

use pumpkin_data::chunk::Biome;
use pumpkin_data::dimension::Dimension;
use pumpkin_data::fluid::Fluid;
use pumpkin_data::{Block, BlockState};
use pumpkin_util::Difficulty;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::ChunkHeightmapType;
use pumpkin_world::generation::proto_chunk::GenerationCache;

use super::World;

/// Light level at which a position counts as open to the sky.
const MAX_LIGHT_LEVEL: u8 = 15;

pub trait SpawnLevel {
    fn get_block(&self, pos: &BlockPos) -> &'static Block;
    fn get_block_state(&self, pos: &BlockPos) -> &'static BlockState;
    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid;
    fn get_biome(&self, pos: &BlockPos) -> &'static Biome;
    fn get_sky_light_level(&self, pos: &BlockPos) -> u8;
    fn get_block_light_level(&self, pos: &BlockPos) -> Option<u8>;
    fn get_sky_darken(&self) -> i32;
    fn sea_level(&self) -> i32;
    fn dimension(&self) -> &Dimension;
    fn difficulty(&self) -> Difficulty;
    fn seed(&self) -> u64;
    /// Height of the `WORLD_SURFACE` heightmap (first free block) at the column.
    fn world_surface_height(&self, x: i32, z: i32) -> i32;
    fn is_player_within(&self, pos: Vector3<f64>, radius: f64) -> bool;

    fn get_raw_brightness(&self, pos: &BlockPos, sky_darken: u8) -> u8 {
        let sky_light = self.get_sky_light_level(pos).saturating_sub(sky_darken);
        let block_light = self.get_block_light_level(pos).unwrap_or(0);
        sky_light.max(block_light)
    }

    fn get_max_local_raw_brightness(&self, pos: &BlockPos) -> u8 {
        self.get_raw_brightness(pos, u8::try_from(self.get_sky_darken()).unwrap_or(0))
    }

    fn can_see_sky(&self, pos: &BlockPos) -> bool {
        let dimension = self.dimension();
        pos.0.y >= dimension.min_y
            && pos.0.y < dimension.min_y + dimension.height
            && self.get_sky_light_level(pos) >= MAX_LIGHT_LEVEL
    }
}

impl SpawnLevel for World {
    fn get_block(&self, pos: &BlockPos) -> &'static Block {
        Self::get_block(self, pos)
    }
    fn get_block_state(&self, pos: &BlockPos) -> &'static BlockState {
        Self::get_block_state(self, pos)
    }
    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid {
        Self::get_fluid(self, pos)
    }
    fn get_biome(&self, pos: &BlockPos) -> &'static Biome {
        Self::get_biome(self, pos)
    }
    fn get_sky_light_level(&self, pos: &BlockPos) -> u8 {
        Self::get_sky_light_level(self, pos)
    }
    fn get_block_light_level(&self, pos: &BlockPos) -> Option<u8> {
        Self::get_block_light_level(self, pos)
    }
    fn get_sky_darken(&self) -> i32 {
        Self::get_sky_darken(self)
    }
    fn sea_level(&self) -> i32 {
        self.sea_level
    }
    fn dimension(&self) -> &Dimension {
        &self.dimension
    }
    fn difficulty(&self) -> Difficulty {
        self.level_info.load().difficulty
    }
    fn seed(&self) -> u64 {
        self.level.seed.0
    }
    fn world_surface_height(&self, x: i32, z: i32) -> i32 {
        self.get_heightmap_height(ChunkHeightmapType::WorldSurface, x, z)
    }
    // Vanilla `getNearestPlayer(..., EntitySelector.NO_SPECTATORS)`: spectators do not count.
    fn is_player_within(&self, pos: Vector3<f64>, radius: f64) -> bool {
        self.get_nearest_player(pos, radius, |player| !player.is_spectator())
            .is_some()
    }
}

/// Spawn rules are often called with the world handle callers already hold.
impl SpawnLevel for std::sync::Arc<World> {
    fn get_block(&self, pos: &BlockPos) -> &'static Block {
        SpawnLevel::get_block(&**self, pos)
    }
    fn get_block_state(&self, pos: &BlockPos) -> &'static BlockState {
        SpawnLevel::get_block_state(&**self, pos)
    }
    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid {
        SpawnLevel::get_fluid(&**self, pos)
    }
    fn get_biome(&self, pos: &BlockPos) -> &'static Biome {
        SpawnLevel::get_biome(&**self, pos)
    }
    fn get_sky_light_level(&self, pos: &BlockPos) -> u8 {
        SpawnLevel::get_sky_light_level(&**self, pos)
    }
    fn get_block_light_level(&self, pos: &BlockPos) -> Option<u8> {
        SpawnLevel::get_block_light_level(&**self, pos)
    }
    fn get_sky_darken(&self) -> i32 {
        SpawnLevel::get_sky_darken(&**self)
    }
    fn sea_level(&self) -> i32 {
        SpawnLevel::sea_level(&**self)
    }
    fn dimension(&self) -> &Dimension {
        SpawnLevel::dimension(&**self)
    }
    fn difficulty(&self) -> Difficulty {
        SpawnLevel::difficulty(&**self)
    }
    fn seed(&self) -> u64 {
        SpawnLevel::seed(&**self)
    }
    fn world_surface_height(&self, x: i32, z: i32) -> i32 {
        SpawnLevel::world_surface_height(&**self, x, z)
    }
    fn is_player_within(&self, pos: Vector3<f64>, radius: f64) -> bool {
        SpawnLevel::is_player_within(&**self, pos, radius)
    }
}

/// A chunk being generated, like vanilla's `WorldGenRegion`: blocks, biomes and light come
/// from the generation cache, world-wide values from the world.
pub struct GenerationSpawnLevel<'a> {
    pub cache: &'a dyn GenerationCache,
    pub world: &'a World,
}

impl SpawnLevel for GenerationSpawnLevel<'_> {
    fn get_block(&self, pos: &BlockPos) -> &'static Block {
        GenerationCache::get_block_state(self.cache, &pos.0).to_block()
    }
    fn get_block_state(&self, pos: &BlockPos) -> &'static BlockState {
        GenerationCache::get_block_state(self.cache, &pos.0).to_state()
    }
    fn get_fluid(&self, pos: &BlockPos) -> &'static Fluid {
        World::get_fluid_from_state_id(GenerationCache::get_block_state(self.cache, &pos.0))
    }
    fn get_biome(&self, pos: &BlockPos) -> &'static Biome {
        self.cache
            .get_biome_for_terrain_gen(pos.0.x, pos.0.y, pos.0.z)
    }
    fn get_sky_light_level(&self, pos: &BlockPos) -> u8 {
        self.cache.get_sky_light(&pos.0)
    }
    fn get_block_light_level(&self, pos: &BlockPos) -> Option<u8> {
        Some(self.cache.get_block_light(&pos.0))
    }
    // Vanilla `WorldGenRegion.getSkyDarken` is always 0.
    fn get_sky_darken(&self) -> i32 {
        0
    }
    fn sea_level(&self) -> i32 {
        self.cache.get_sea_level()
    }
    fn dimension(&self) -> &Dimension {
        &self.world.dimension
    }
    fn difficulty(&self) -> Difficulty {
        self.world.level_info.load().difficulty
    }
    fn seed(&self) -> u64 {
        self.world.level.seed.0
    }
    fn world_surface_height(&self, x: i32, z: i32) -> i32 {
        self.cache.top_block_height_exclusive(x, z)
    }
    // A `WorldGenRegion` has no players.
    fn is_player_within(&self, _pos: Vector3<f64>, _radius: f64) -> bool {
        false
    }
}
