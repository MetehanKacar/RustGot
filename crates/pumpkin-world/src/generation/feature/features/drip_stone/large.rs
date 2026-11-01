//! Vanilla `LargeDripstoneFeature`: a big stalactite/stalagmite pair (a column when they meet),
//! shaped by a curve of height over radius and slanted by an optional wind.

use pumpkin_data::tag;
use pumpkin_util::math::float_provider::FloatProvider;
use pumpkin_util::math::int_provider::IntProvider;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::{cos, sin};
use pumpkin_util::random::{RandomGenerator, RandomImpl};

use super::SpeleothemBlocks;
use crate::generation::proto_chunk::GenerationCache;

pub struct LargeDripstoneFeature {
    /// Only `base` (dripstone) and `replaceable` are used, as in vanilla.
    pub blocks: SpeleothemBlocks,
    pub floor_to_ceiling_search_range: i32,
    pub column_radius: IntProvider,
    pub height_scale: FloatProvider,
    pub max_column_radius_to_cave_height_ratio: f32,
    pub stalactite_bluntness: FloatProvider,
    pub stalagmite_bluntness: FloatProvider,
    pub wind_speed: FloatProvider,
    pub min_radius_for_wind: i32,
    pub min_bluntness_for_wind: f32,
}

impl LargeDripstoneFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        origin: BlockPos,
    ) -> bool {
        if !super::is_empty_or_water(chunk, origin) {
            return false;
        }

        let Some(column) = super::scan_column(
            chunk,
            origin,
            self.floor_to_ceiling_search_range,
            super::is_empty_or_water_id,
            // Vanilla `isBaseOrLava`.
            |id| super::is_base(&self.blocks, id) || id == pumpkin_data::BlockId::LAVA,
        ) else {
            return false;
        };
        // Only a `Range` column (both ends found) is usable.
        let (Some(floor), Some(ceiling), Some(column_height)) =
            (column.floor, column.ceiling, column.height())
        else {
            return false;
        };
        if column_height < 4 {
            return false;
        }

        let max_radius_for_height =
            (column_height as f32 * self.max_column_radius_to_cave_height_ratio) as i32;
        let min_radius = self.column_radius.get_min();
        let max_column_radius = max_radius_for_height
            .max(min_radius)
            .min(self.column_radius.get_max());
        let radius = random_between_inclusive(random, min_radius, max_column_radius);

        let mut stalactite = LargeDripstone::new(
            BlockPos::new(origin.0.x, ceiling - 1, origin.0.z),
            false,
            radius,
            &self.stalactite_bluntness,
            &self.height_scale,
            random,
        );
        let mut stalagmite = LargeDripstone::new(
            BlockPos::new(origin.0.x, floor + 1, origin.0.z),
            true,
            radius,
            &self.stalagmite_bluntness,
            &self.height_scale,
            random,
        );

        let wind = if stalactite.is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
            && stalagmite
                .is_suitable_for_wind(self.min_radius_for_wind, self.min_bluntness_for_wind)
        {
            WindOffsetter::new(origin.0.y, random, &self.wind_speed, 16 - radius)
        } else {
            WindOffsetter::no_wind()
        };

        let stalactite_embedded = stalactite.move_back_until_base_is_inside_stone(chunk, &wind);
        let stalagmite_embedded = stalagmite.move_back_until_base_is_inside_stone(chunk, &wind);
        if stalactite_embedded {
            stalactite.place_blocks(chunk, random, &wind, &self.blocks);
        }
        if stalagmite_embedded {
            stalagmite.place_blocks(chunk, random, &wind, &self.blocks);
        }
        true
    }
}

struct LargeDripstone {
    root: BlockPos,
    pointing_up: bool,
    radius: i32,
    bluntness: f64,
    scale: f64,
}

impl LargeDripstone {
    fn new(
        root: BlockPos,
        pointing_up: bool,
        radius: i32,
        bluntness: &FloatProvider,
        height_scale: &FloatProvider,
        random: &mut RandomGenerator,
    ) -> Self {
        // Vanilla evaluates the bluntness sample before the height scale.
        let bluntness = f64::from(bluntness.get(random));
        let scale = f64::from(height_scale.get(random));
        Self {
            root,
            pointing_up,
            radius,
            bluntness,
            scale,
        }
    }

    fn height(&self) -> i32 {
        self.height_at_radius(0.0)
    }

    fn height_at_radius(&self, check_radius: f32) -> i32 {
        speleothem_height(
            f64::from(check_radius),
            f64::from(self.radius),
            self.scale,
            self.bluntness,
        ) as i32
    }

    fn is_suitable_for_wind(&self, min_radius: i32, min_bluntness: f32) -> bool {
        self.radius >= min_radius && self.bluntness >= f64::from(min_bluntness)
    }

    /// Returns whether the base ended up embedded in stone (and so should be placed).
    fn move_back_until_base_is_inside_stone<T: GenerationCache>(
        &mut self,
        chunk: &T,
        wind: &WindOffsetter,
    ) -> bool {
        while self.radius > 1 {
            let mut new_root = self.root;
            let max_tries = 10.min(self.height());
            for _ in 0..max_tries {
                if GenerationCache::get_block_state(chunk, &new_root.0).to_block_id()
                    == pumpkin_data::BlockId::LAVA
                {
                    return false;
                }
                if is_circle_mostly_embedded_in_stone(chunk, wind.offset(new_root), self.radius) {
                    self.root = new_root;
                    return true;
                }
                new_root = if self.pointing_up {
                    new_root.down()
                } else {
                    new_root.up()
                };
            }
            self.radius /= 2;
        }
        false
    }

    fn place_blocks<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        wind: &WindOffsetter,
        blocks: &SpeleothemBlocks,
    ) {
        for dx in -self.radius..=self.radius {
            for dz in -self.radius..=self.radius {
                let current_radius = f64::from(dx * dx + dz * dz).sqrt() as f32;
                if current_radius > self.radius as f32 {
                    continue;
                }
                let mut height = self.height_at_radius(current_radius);
                if height <= 0 {
                    continue;
                }
                if f64::from(random.next_f32()) < 0.2 {
                    // `Mth.randomBetween(random, 0.8F, 1.0F)`.
                    let scale = random.next_f32() * (1.0 - 0.8) + 0.8;
                    height = (height as f32 * scale) as i32;
                }

                let mut pos = BlockPos::new(self.root.0.x + dx, self.root.0.y, self.root.0.z + dz);
                let mut has_been_out_of_stone = false;
                let max_y = if self.pointing_up {
                    chunk.top_block_wg_height_exclusive(pos.0.x, pos.0.z)
                } else {
                    i32::MAX
                };

                let mut i = 0;
                while i < height && pos.0.y < max_y {
                    let adjusted = wind.offset(pos);
                    if super::is_empty_or_water_or_lava(chunk, adjusted) {
                        has_been_out_of_stone = true;
                        chunk.set_block_state(&adjusted.0, blocks.base.default_state);
                    } else if has_been_out_of_stone
                        && GenerationCache::get_block_state(chunk, &adjusted.0)
                            .to_block_id()
                            .has_tag(tag::Block::MINECRAFT_BASE_STONE_OVERWORLD)
                    {
                        break;
                    }
                    pos = if self.pointing_up { pos.up() } else { pos.down() };
                    i += 1;
                }
            }
        }
    }
}

/// Slants a stalactite/stalagmite pair: the further a block is from the origin's height, the
/// further it is shifted along a random horizontal wind direction.
struct WindOffsetter {
    origin_y: i32,
    /// `(x, z)` shift per block of height difference, or none without wind.
    wind_speed: Option<(f64, f64)>,
    max_offset: i32,
}

impl WindOffsetter {
    fn new(
        origin_y: i32,
        random: &mut RandomGenerator,
        wind_speed: &FloatProvider,
        max_offset: i32,
    ) -> Self {
        let speed = wind_speed.get(random);
        // `Mth.randomBetween(random, 0.0F, (float) Math.PI)`.
        let direction = random.next_f32() * std::f32::consts::PI;
        Self {
            origin_y,
            wind_speed: Some((
                f64::from(cos(direction) * speed),
                f64::from(sin(direction) * speed),
            )),
            max_offset,
        }
    }

    const fn no_wind() -> Self {
        Self {
            origin_y: 0,
            wind_speed: None,
            max_offset: 0,
        }
    }

    fn offset(&self, pos: BlockPos) -> BlockPos {
        let Some((speed_x, speed_z)) = self.wind_speed else {
            return pos;
        };
        let dy = f64::from(self.origin_y - pos.0.y);
        let dx = ((speed_x * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        let dz = ((speed_z * dy).floor() as i32).clamp(-self.max_offset, self.max_offset);
        BlockPos::new(pos.0.x + dx, pos.0.y, pos.0.z + dz)
    }
}

/// Vanilla `SpeleothemUtils.getSpeleothemHeight`.
fn speleothem_height(
    mut xz_distance_from_center: f64,
    speleothem_radius: f64,
    scale: f64,
    bluntness: f64,
) -> f64 {
    if xz_distance_from_center < bluntness {
        xz_distance_from_center = bluntness;
    }
    let r = xz_distance_from_center / speleothem_radius * 0.384;
    let part1 = 0.75 * r.powf(4.0 / 3.0);
    let part2 = r.powf(2.0 / 3.0);
    let part3 = (1.0 / 3.0) * r.ln();
    let height_relative_to_max_radius = (scale * (part1 - part2 - part3)).max(0.0);
    height_relative_to_max_radius / 0.384 * speleothem_radius
}

/// Vanilla `SpeleothemUtils.isCircleMostlyEmbeddedInStone`.
fn is_circle_mostly_embedded_in_stone<T: GenerationCache>(
    chunk: &T,
    center: BlockPos,
    xz_radius: i32,
) -> bool {
    if super::is_empty_or_water_or_lava(chunk, center) {
        return false;
    }
    let angle_increment = 6.0f32 / xz_radius as f32;
    let mut angle = 0.0f32;
    while angle < std::f32::consts::TAU {
        let dx = (cos(angle) * xz_radius as f32) as i32;
        let dz = (sin(angle) * xz_radius as f32) as i32;
        let pos = BlockPos::new(center.0.x + dx, center.0.y, center.0.z + dz);
        if super::is_empty_or_water_or_lava(chunk, pos) {
            return false;
        }
        angle += angle_increment;
    }
    true
}

/// Vanilla `Mth.randomBetweenInclusive`.
fn random_between_inclusive(random: &mut RandomGenerator, min: i32, max: i32) -> i32 {
    random.next_bounded_i32(max - min + 1) + min
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn height_curve_peaks_at_the_centre_and_falls_to_zero_at_the_edge() {
        let (radius, scale, bluntness) = (8.0, 1.0, 0.5);
        let centre = speleothem_height(0.0, radius, scale, bluntness);
        let middle = speleothem_height(4.0, radius, scale, bluntness);
        let edge = speleothem_height(8.0, radius, scale, bluntness);
        assert!(centre > middle && middle > edge, "{centre} {middle} {edge}");
        assert!(edge.abs() < 1.0, "the edge should be (nearly) flat, got {edge}");
        // Below the bluntness the curve is clamped to its value at the bluntness.
        assert_eq!(
            speleothem_height(0.0, radius, scale, bluntness),
            speleothem_height(bluntness, radius, scale, bluntness)
        );
    }

    #[test]
    fn wind_shifts_blocks_in_proportion_to_their_distance_from_the_origin_height() {
        let wind = WindOffsetter {
            origin_y: 64,
            wind_speed: Some((0.25, -0.5)),
            max_offset: 3,
        };
        assert_eq!(wind.offset(BlockPos::new(10, 64, 10)), BlockPos::new(10, 64, 10));
        // 8 blocks above: dx = floor(0.25 * -8) = -2, dz = floor(-0.5 * -8) = 4 -> clamped to 3.
        assert_eq!(wind.offset(BlockPos::new(10, 72, 10)), BlockPos::new(8, 72, 13));
        // 4 blocks below: dx = floor(1.0) = 1, dz = floor(-2.0) = -2.
        assert_eq!(wind.offset(BlockPos::new(0, 60, 0)), BlockPos::new(1, 60, -2));
        assert_eq!(
            WindOffsetter::no_wind().offset(BlockPos::new(1, 2, 3)),
            BlockPos::new(1, 2, 3)
        );
    }
}
