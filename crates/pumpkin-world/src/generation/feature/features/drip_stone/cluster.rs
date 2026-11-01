//! Vanilla 26.x `SpeleothemClusterFeature`: a patch of stalactites and stalagmites (dripstone,
//! or sulfur spikes in sulfur caves) with base-block layers on the ceiling and floor.

use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, tag};
use pumpkin_util::math::clamped_map;
use pumpkin_util::math::float_provider::FloatProvider;
use pumpkin_util::math::int_provider::IntProvider;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::{Column, SpeleothemBlocks};
use crate::generation::proto_chunk::GenerationCache;

pub struct DripstoneClusterFeature {
    pub blocks: SpeleothemBlocks,
    pub floor_to_ceiling_search_range: i32,
    pub height: IntProvider,
    pub radius: IntProvider,
    pub max_stalagmite_stalactite_height_diff: i32,
    pub height_deviation: i32,
    pub speleothem_block_layer_thickness: IntProvider,
    pub density: FloatProvider,
    pub wetness: FloatProvider,
    pub chance_of_speleothem_at_max_distance_from_center: f32,
    pub max_distance_from_edge_affecting_chance_of_speleothem: i32,
    pub max_distance_from_center_affecting_height_bias: i32,
}

impl DripstoneClusterFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        if !super::is_empty_or_water(chunk, origin) {
            return false;
        }

        let height = self.height.get(random);
        let wetness = self.wetness.get(random);
        let density = self.density.get(random);
        let x_radius = self.radius.get(random);
        let z_radius = self.radius.get(random);

        for dx in -x_radius..=x_radius {
            for dz in -z_radius..=z_radius {
                let chance = self.chance_of_stalagmite_or_stalactite(x_radius, z_radius, dx, dz);
                let pos = BlockPos::new(origin.0.x + dx, origin.0.y, origin.0.z + dz);
                self.place_column(chunk, random, pos, dx, dz, wetness, chance, height, density);
            }
        }
        true
    }

    #[allow(clippy::too_many_arguments, clippy::too_many_lines)]
    fn place_column<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
        dx: i32,
        dz: i32,
        chance_of_water: f32,
        chance_of_stalagmite_or_stalactite: f64,
        cluster_height: i32,
        density: f32,
    ) {
        let Some(base_column) = super::scan_column(
            chunk,
            pos,
            self.floor_to_ceiling_search_range,
            super::is_empty_or_water_id,
            |id| !super::is_empty_or_water_id(id),
        ) else {
            return;
        };
        let ceiling = base_column.ceiling;
        let base_floor = base_column.floor;
        if ceiling.is_none() && base_floor.is_none() {
            return;
        }

        let want_pool = random.next_f32() < chance_of_water;
        let column = match base_floor {
            Some(floor_y) if want_pool && self.can_place_pool(chunk, at_y(pos, floor_y)) => {
                chunk.set_block_state(&at_y(pos, floor_y).0, Block::WATER.default_state);
                Column {
                    floor: Some(floor_y - 1),
                    ceiling,
                }
            }
            _ => base_column,
        };
        let floor = column.floor;

        let want_stalactite = random.next_f64() < chance_of_stalagmite_or_stalactite;
        let stalactite_height = match ceiling {
            Some(ceiling_y) if want_stalactite && !is_lava(chunk, at_y(pos, ceiling_y)) => {
                let thickness = self.speleothem_block_layer_thickness.get(random);
                self.replace_with_base_blocks(
                    chunk,
                    at_y(pos, ceiling_y),
                    thickness,
                    BlockDirection::Up,
                );
                let max_height = floor.map_or(cluster_height, |floor_y| {
                    cluster_height.min(ceiling_y - floor_y)
                });
                self.speleothem_height(random, dx, dz, density, max_height)
            }
            _ => 0,
        };

        let want_stalagmite = random.next_f64() < chance_of_stalagmite_or_stalactite;
        let stalagmite_height = match floor {
            Some(floor_y) if want_stalagmite && !is_lava(chunk, at_y(pos, floor_y)) => {
                let thickness = self.speleothem_block_layer_thickness.get(random);
                self.replace_with_base_blocks(
                    chunk,
                    at_y(pos, floor_y),
                    thickness,
                    BlockDirection::Down,
                );
                if ceiling.is_some() {
                    let diff = self.max_stalagmite_stalactite_height_diff;
                    (stalactite_height + random_between_inclusive(random, -diff, diff)).max(0)
                } else {
                    self.speleothem_height(random, dx, dz, density, cluster_height)
                }
            }
            _ => 0,
        };

        let (stalactite_height, stalagmite_height) = match (ceiling, floor) {
            (Some(ceiling_y), Some(floor_y))
                if ceiling_y - stalactite_height <= floor_y + stalagmite_height =>
            {
                let lowest_stalactite_bottom = (ceiling_y - stalactite_height).max(floor_y + 1);
                let highest_stalagmite_top = (floor_y + stalagmite_height).min(ceiling_y - 1);
                let stalactite_bottom = random_between_inclusive(
                    random,
                    lowest_stalactite_bottom,
                    highest_stalagmite_top + 1,
                );
                (ceiling_y - stalactite_bottom, stalactite_bottom - 1 - floor_y)
            }
            _ => (stalactite_height, stalagmite_height),
        };

        let merge_tips = random.next_bool()
            && stalactite_height > 0
            && stalagmite_height > 0
            && column.height() == Some(stalactite_height + stalagmite_height);
        if let Some(ceiling_y) = ceiling {
            super::grow_speleothem(
                chunk,
                at_y(pos, ceiling_y - 1),
                BlockDirection::Down,
                stalactite_height,
                merge_tips,
                &self.blocks,
            );
        }
        if let Some(floor_y) = floor {
            super::grow_speleothem(
                chunk,
                at_y(pos, floor_y + 1),
                BlockDirection::Up,
                stalagmite_height,
                merge_tips,
                &self.blocks,
            );
        }
    }

    fn speleothem_height(
        &self,
        random: &mut RandomGenerator,
        dx: i32,
        dz: i32,
        density: f32,
        max_height: i32,
    ) -> i32 {
        if random.next_f32() > density {
            return 0;
        }
        let distance_from_center = dx.abs() + dz.abs();
        let height_mean = clamped_map(
            f64::from(distance_from_center),
            0.0,
            f64::from(self.max_distance_from_center_affecting_height_bias),
            f64::from(max_height) / 2.0,
            0.0,
        ) as f32;
        // `ClampedNormalFloat.sample(random, mean, deviation, 0, maxHeight)`.
        let normal = height_mean + random.next_gaussian() as f32 * self.height_deviation as f32;
        normal.clamp(0.0, max_height as f32) as i32
    }

    fn can_place_pool<T: GenerationCache>(&self, chunk: &T, pos: BlockPos) -> bool {
        let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
        if block == Block::WATER.id
            || block == self.blocks.base.id
            || block == self.blocks.pointed.id
        {
            return false;
        }
        if is_water_at(chunk, pos.up()) {
            return false;
        }
        // `Plane.HORIZONTAL` order.
        [
            BlockDirection::North,
            BlockDirection::East,
            BlockDirection::South,
            BlockDirection::West,
        ]
        .into_iter()
        .all(|direction| can_be_adjacent_to_water(chunk, pos.offset(direction.to_offset())))
            && can_be_adjacent_to_water(chunk, pos.down())
    }

    fn replace_with_base_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        first_pos: BlockPos,
        max_count: i32,
        direction: BlockDirection,
    ) {
        let mut pos = first_pos;
        for _ in 0..max_count {
            if !super::place_base_block_if_possible(chunk, pos, &self.blocks) {
                return;
            }
            pos = pos.offset(direction.to_offset());
        }
    }

    fn chance_of_stalagmite_or_stalactite(
        &self,
        x_radius: i32,
        z_radius: i32,
        dx: i32,
        dz: i32,
    ) -> f64 {
        let distance_from_edge = (x_radius - dx.abs()).min(z_radius - dz.abs());
        f64::from(clamped_map(
            distance_from_edge as f32,
            0.0,
            self.max_distance_from_edge_affecting_chance_of_speleothem as f32,
            self.chance_of_speleothem_at_max_distance_from_center,
            1.0,
        ))
    }
}

const fn at_y(pos: BlockPos, y: i32) -> BlockPos {
    BlockPos::new(pos.0.x, y, pos.0.z)
}

fn is_lava<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    GenerationCache::get_block_state(chunk, &pos.0).to_block_id() == Block::LAVA.id
}

/// Vanilla `isWaterAt`: the fluid at the position is water (including waterlogged blocks).
fn is_water_at<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let (fluid, _) = GenerationCache::get_fluid_and_fluid_state(chunk, &pos.0);
    fluid.has_tag(&tag::Fluid::MINECRAFT_WATER)
}

fn can_be_adjacent_to_water<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    GenerationCache::get_block_state(chunk, &pos.0)
        .to_block_id()
        .has_tag(tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
        || is_water_at(chunk, pos)
}

/// Vanilla `Mth.randomBetweenInclusive`.
fn random_between_inclusive(random: &mut RandomGenerator, min: i32, max: i32) -> i32 {
    random.next_bounded_i32(max - min + 1) + min
}
