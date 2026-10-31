use std::sync::{Arc, OnceLock};

use pumpkin_data::{BlockState, BlockStateId, Rotation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::{
    math::{position::BlockPos, vector3::Vector3},
    random::{RandomGenerator, RandomImpl},
};

use crate::generation::{
    proto_chunk::GenerationCache,
    structure::template::{
        BlockPlacer, get_template, place_template_with_options,
        processor::{StructureProcessor, load_processor_list, parse_processor_list},
    },
};

/// One weighted entry of a template feature.
pub struct TemplateEntry {
    /// Template id without the `minecraft:` prefix (e.g. `spring/sulfur_spring_small_1`).
    pub template: &'static str,
    /// Rotations to pick from (vanilla defaults to all four).
    pub rotations: &'static [Rotation],
    pub weight: i32,
}

pub enum TemplateProcessors {
    None,
    /// A processor list referenced by id.
    Named(&'static str),
    /// An inline processor list, parsed on first use.
    Inline {
        json: &'static str,
        parsed: OnceLock<Arc<[StructureProcessor]>>,
    },
}

impl TemplateProcessors {
    fn get(&self) -> Arc<[StructureProcessor]> {
        match self {
            Self::None => Arc::from([]),
            Self::Named(name) => load_processor_list(name),
            Self::Inline { json, parsed } => parsed
                .get_or_init(|| {
                    parse_processor_list(json).unwrap_or_else(|err| {
                        tracing::warn!("Invalid template feature processors: {err}");
                        Arc::from([])
                    })
                })
                .clone(),
        }
    }
}

/// Vanilla `TemplateFeature` (26.x): places a structure template centred on the origin with a
/// random allowed rotation. Used by desert wells and sulfur springs.
pub struct TemplateFeature {
    pub templates: Vec<TemplateEntry>,
    pub processors: TemplateProcessors,
}

impl TemplateFeature {
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        // `WeightedList.getRandomOrThrow`, then `Util.getRandom(rotations)`.
        let total: i32 = self.templates.iter().map(|entry| entry.weight.max(0)).sum();
        if total <= 0 {
            return false;
        }
        let mut roll = random.next_bounded_i32(total);
        let Some(entry) = self.templates.iter().find(|entry| {
            roll -= entry.weight.max(0);
            roll < 0
        }) else {
            return false;
        };
        let Ok(rotation_count) = i32::try_from(entry.rotations.len()) else {
            return false;
        };
        if rotation_count == 0 {
            return false;
        }
        let rotation = entry.rotations[random.next_bounded_i32(rotation_count) as usize];
        let Some(template) = get_template(entry.template) else {
            tracing::warn!("Unknown template for template feature: {}", entry.template);
            return false;
        };

        let origin = template_origin(pos.0, template.size, rotation);
        let processors = self.processors.get();
        place_template_with_options(
            &mut CachePlacer(chunk),
            &template,
            origin,
            (0, 0),
            rotation,
            false,
            true,
            &processors,
            None,
            false,
        );
        true
    }
}

/// Where `RustGot`'s template placement must start so blocks land where vanilla puts them.
///
/// Vanilla shifts the origin by half the template along the rotated west and north axes and
/// then rotates every block around that point. `transform_pos` instead rotates inside the
/// template's bounds, which is the same rotation moved by the rotated box's minimum corner.
const fn template_origin(
    pos: Vector3<i32>,
    size: Vector3<i32>,
    rotation: Rotation,
) -> Vector3<i32> {
    let (half_x, half_z) = (size.x / 2, size.z / 2);
    let (dx, dz) = match rotation {
        Rotation::None => (-half_x, -half_z),
        Rotation::Clockwise90 => (half_z - (size.z - 1), -half_x),
        Rotation::Rotate180 => (half_x - (size.x - 1), half_z - (size.z - 1)),
        Rotation::CounterClockwise90 => (-half_z, half_x - (size.x - 1)),
    };
    Vector3::new(pos.x + dx, pos.y, pos.z + dz)
}

/// Lets template placement write through a generation cache (every chunk of the region).
struct CachePlacer<'a, T: GenerationCache>(&'a mut T);

impl<T: GenerationCache> BlockPlacer for CachePlacer<'_, T> {
    fn get_block_state(&self, pos: &Vector3<i32>) -> BlockStateId {
        GenerationCache::get_block_state(self.0, pos)
    }

    fn set_block_state(&mut self, pos: &Vector3<i32>, state: &BlockState) {
        GenerationCache::set_block_state(self.0, pos, state);
    }

    fn add_block_entity(&mut self, nbt: NbtCompound) {
        if let (Some(x), Some(y), Some(z)) = (nbt.get_int("x"), nbt.get_int("y"), nbt.get_int("z"))
        {
            GenerationCache::add_block_entity(self.0, &Vector3::new(x, y, z), nbt);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Vanilla: start at `pos + rotate(WEST) * (sx / 2) + rotate(NORTH) * (sz / 2)` and rotate
    /// every template block around that point.
    fn vanilla_world_pos(
        pos: Vector3<i32>,
        size: Vector3<i32>,
        rotation: Rotation,
        block: Vector3<i32>,
    ) -> Vector3<i32> {
        let (hx, hz) = (size.x / 2, size.z / 2);
        let (start_x, start_z) = match rotation {
            Rotation::None => (-hx, -hz),
            Rotation::Clockwise90 => (hz, -hx),
            Rotation::Rotate180 => (hx, hz),
            Rotation::CounterClockwise90 => (-hz, hx),
        };
        let (bx, bz) = rotation.rotate_offset(block.x, block.z);
        Vector3::new(pos.x + start_x + bx, pos.y + block.y, pos.z + start_z + bz)
    }

    #[test]
    fn template_origin_matches_vanilla_rotation_around_the_shifted_origin() {
        let pos = Vector3::new(100, 64, -50);
        let size = Vector3::new(7, 4, 5);
        for rotation in [
            Rotation::None,
            Rotation::Clockwise90,
            Rotation::Rotate180,
            Rotation::CounterClockwise90,
        ] {
            let origin = template_origin(pos, size, rotation);
            for block in [
                Vector3::new(0, 0, 0),
                Vector3::new(6, 3, 4),
                Vector3::new(2, 1, 3),
            ] {
                let local = rotation.transform_pos(block, size);
                let placed =
                    Vector3::new(origin.x + local.x, origin.y + local.y, origin.z + local.z);
                assert_eq!(
                    placed,
                    vanilla_world_pos(pos, size, rotation, block),
                    "{rotation:?} {block:?}"
                );
            }
        }
    }
}
