use pumpkin_data::{BlockDirection, BlockState, fluid::Fluid};
use pumpkin_util::{
    math::{position::BlockPos, vector3::Vector3},
    random::RandomGenerator,
};

use crate::generation::proto_chunk::GenerationCache;
use crate::world::WorldPortalExt;

pub struct SpringFeatureFeature {
    pub state: &'static BlockState,
    pub requires_block_below: bool,
    pub rock_count: i32,
    pub hole_count: i32,
    pub valid_blocks: BlockWrapper,
}

pub enum BlockWrapper {
    Single(String),
    Multi(Vec<String>),
}

impl BlockWrapper {
    fn contains(&self, name: &str) -> bool {
        let name_stripped = name.strip_prefix("minecraft:").unwrap_or(name);
        match self {
            Self::Single(s) => s.strip_prefix("minecraft:").unwrap_or(s) == name_stripped,
            Self::Multi(h) => h
                .iter()
                .any(|s| s.strip_prefix("minecraft:").unwrap_or(s) == name_stripped),
        }
    }
}

impl SpringFeatureFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        _block_registry: &dyn WorldPortalExt,
        chunk: &mut T,
        _random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        let valid_blocks = &self.valid_blocks;

        if !valid_blocks.contains(
            GenerationCache::get_block_state(chunk, &pos.up().0)
                .to_block()
                .name,
        ) {
            return false;
        }
        if self.requires_block_below
            && !valid_blocks.contains(
                GenerationCache::get_block_state(
                    chunk,
                    &pos.offset(BlockDirection::Down.to_offset()).0,
                )
                .to_block()
                .name,
            )
        {
            return false;
        }
        let state = GenerationCache::get_block_state(chunk, &pos.0);
        if !state.to_state().is_air() && !valid_blocks.contains(state.to_block().name) {
            return false;
        }

        let mut valid = 0;
        if valid_blocks.contains(
            GenerationCache::get_block_state(
                chunk,
                &pos.offset(BlockDirection::West.to_offset()).0,
            )
            .to_block()
            .name,
        ) {
            valid += 1;
        }
        if valid_blocks.contains(
            GenerationCache::get_block_state(
                chunk,
                &pos.offset(BlockDirection::East.to_offset()).0,
            )
            .to_block()
            .name,
        ) {
            valid += 1;
        }
        if valid_blocks.contains(
            GenerationCache::get_block_state(
                chunk,
                &pos.offset(BlockDirection::North.to_offset()).0,
            )
            .to_block()
            .name,
        ) {
            valid += 1;
        }
        if valid_blocks.contains(
            GenerationCache::get_block_state(
                chunk,
                &pos.offset(BlockDirection::South.to_offset()).0,
            )
            .to_block()
            .name,
        ) {
            valid += 1;
        }
        if valid_blocks.contains(
            GenerationCache::get_block_state(
                chunk,
                &pos.offset(BlockDirection::Down.to_offset()).0,
            )
            .to_block()
            .name,
        ) {
            valid += 1;
        }
        let mut air = 0;
        if chunk.is_air(&pos.offset(BlockDirection::West.to_offset()).0) {
            air += 1;
        }
        if chunk.is_air(&pos.offset(BlockDirection::East.to_offset()).0) {
            air += 1;
        }
        if chunk.is_air(&pos.offset(BlockDirection::North.to_offset()).0) {
            air += 1;
        }
        if chunk.is_air(&pos.offset(BlockDirection::South.to_offset()).0) {
            air += 1;
        }
        if chunk.is_air(&pos.offset(BlockDirection::Down.to_offset()).0) {
            air += 1;
        }
        if valid == self.rock_count && air == self.hole_count {
            self.place(chunk, &pos.0);
            return true;
        }
        false
    }

    fn place<T: GenerationCache>(&self, chunk: &mut T, pos: &Vector3<i32>) {
        chunk.set_block_state(pos, self.state);
        if let Some(fluid) = Fluid::from_state_id(self.state.id)
            && *fluid != Fluid::EMPTY
        {
            chunk.schedule_fluid_tick(pos, fluid);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::generation::get_world_gen;
    use crate::generation::proto_chunk::ProtoChunk;
    use crate::world::BlockAccessor;
    use pumpkin_data::dimension::Dimension;
    use pumpkin_data::{Block, BlockStateId, Mirror, Rotation};
    use pumpkin_util::random::legacy_rand::LegacyRand;
    use pumpkin_util::world_seed::Seed;

    struct NoPortal;
    impl WorldPortalExt for NoPortal {
        fn can_place_at(
            &self,
            _block: &Block,
            _state: &BlockState,
            _block_accessor: &dyn BlockAccessor,
            _block_pos: &BlockPos,
        ) -> bool {
            true
        }

        fn mirror(
            &self,
            _block: &Block,
            state_id: BlockStateId,
            mirror: Mirror,
        ) -> &'static BlockState {
            BlockState::from_id(state_id).mirror(mirror)
        }

        fn rotate(
            &self,
            _block: &Block,
            state_id: BlockStateId,
            rotation: Rotation,
        ) -> &'static BlockState {
            BlockState::from_id(state_id).rotate(rotation)
        }

        fn spawn_mobs_for_chunk_generation(
            &self,
            _cache: &mut dyn GenerationCache,
            _biome: &'static pumpkin_data::biome::Biome,
            _chunk_x: i32,
            _chunk_z: i32,
        ) {
        }
    }

    #[test]
    fn placed_spring_schedules_a_fluid_tick() {
        let world_gen = get_world_gen(
            Seed(0),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let mut chunk = ProtoChunk::new(0, 0, &world_gen);
        for (x, y, z) in [(8, 41, 8), (8, 39, 8), (7, 40, 8), (9, 40, 8), (8, 40, 7)] {
            chunk.set_block_state(x, y, z, Block::STONE.default_state);
        }
        let spring = SpringFeatureFeature {
            state: Block::WATER.default_state,
            requires_block_below: true,
            rock_count: 4,
            hole_count: 1,
            valid_blocks: BlockWrapper::Single("minecraft:stone".to_string()),
        };
        let mut random = RandomGenerator::Legacy(LegacyRand::from_seed(0));

        assert!(spring.generate(&NoPortal, &mut chunk, &mut random, BlockPos::new(8, 40, 8)));
        assert_eq!(chunk.fluid_ticks.len(), 1);
        assert_eq!(chunk.fluid_ticks[0].position, BlockPos::new(8, 40, 8));
        assert_eq!(chunk.fluid_ticks[0].value.id, Fluid::WATER.id);
        assert_eq!(chunk.fluid_ticks[0].delay, 0);
    }
}
