use pumpkin_data::{
    structures::{Structure, StructureKeys},
    tag::{RegistryKey, get_tag_ids},
};

use std::cell::RefCell;
use std::sync::{Arc, Mutex};

use pumpkin_data::chunk::Biome;
use pumpkin_util::math::{vector2::Vector2, vector3::Vector3};

use crate::chunk::structures::{
    ChunkStructures, ReferencedStart, StructureReferences, StructureStart,
};
use crate::generation::structure::structures::{
    StructureInstance, StructurePiecesCollector, StructureReference,
};
use crate::{
    ProtoChunk,
    biome::BiomeSupplier,
    generation::{
        biome_coords,
        noise::router::multi_noise_sampler::MultiNoiseSampler,
        structure::structures::{
            StructureGenerator, StructureGeneratorContext, StructurePosition,
            buried_treasure::BuriedTreasureGenerator,
            create_chunk_random,
            desert_pyramid::DesertPyramidGenerator,
            end_city::EndCityGenerator,
            igloo::IglooGenerator,
            jigsaw::JigsawGenerator,
            jungle_temple::JungleTempleGenerator,
            mansion::MansionGenerator,
            mineshaft::MineshaftGenerator,
            nether_fortress::NetherFortressGenerator,
            nether_fossil::NetherFossilGenerator,
            ocean_monument::{OceanMonumentGenerator, has_valid_biomes},
            ocean_ruin::OceanRuinGenerator,
            ruined_portal::RuinedPortalGenerator,
            shipwreck::ShipwreckGenerator,
            stronghold::StrongholdGenerator,
            swamp_hut::SwampHutGenerator,
        },
    },
};

pub(crate) mod height_sampler;
pub mod piece;
pub mod placement;
pub mod shiftable_piece;
pub mod structures;
pub mod template;

/// Creates a structure position by dispatching to the appropriate generator.
/// Does NOT perform biome validation — callers that require it should add
/// their own check after calling this function.
#[must_use]
#[allow(clippy::too_many_lines)]
pub fn generate_structure_position(
    key: &StructureKeys,
    structure: &Structure,
    context: StructureGeneratorContext<'_>,
) -> Option<StructurePosition> {
    generate_structure_position_checked(*key, structure, context, None, |_| true)
}

#[must_use]
#[allow(clippy::too_many_lines)]
fn generate_structure_position_checked(
    key: StructureKeys,
    structure: &Structure,
    context: StructureGeneratorContext<'_>,
    is_cold: Option<&dyn Fn(Vector3<i32>) -> bool>,
    is_valid_biome: impl FnOnce(pumpkin_util::math::position::BlockPos) -> bool,
) -> Option<StructurePosition> {
    match key {
        StructureKeys::BuriedTreasure => {
            BuriedTreasureGenerator::get_structure_position(&BuriedTreasureGenerator, context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::SwampHut => {
            SwampHutGenerator::get_structure_position(&SwampHutGenerator, context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::Stronghold => {
            StrongholdGenerator::get_structure_position(&StrongholdGenerator, context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::Fortress => {
            NetherFortressGenerator::get_structure_position(&NetherFortressGenerator, context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::NetherFossil => {
            NetherFossilGenerator::get_structure_position(&NetherFossilGenerator, context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::Igloo => IglooGenerator::get_structure_position(&IglooGenerator, context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::DesertPyramid => DesertPyramidGenerator
            .get_structure_position(context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::JunglePyramid => JungleTempleGenerator
            .get_structure_position(context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::VillagePlains
        | StructureKeys::VillageDesert
        | StructureKeys::VillageSavanna
        | StructureKeys::VillageSnowy
        | StructureKeys::VillageTaiga
        | StructureKeys::AbandonedCampBambooJungle
        | StructureKeys::AbandonedCampBirchForest
        | StructureKeys::AbandonedCampCherryGrove
        | StructureKeys::AbandonedCampDappledForest
        | StructureKeys::AbandonedCampFlowerForest
        | StructureKeys::AbandonedCampForest
        | StructureKeys::AbandonedCampMeadow
        | StructureKeys::AbandonedCampOldGrowthBirchForest
        | StructureKeys::AbandonedCampOldGrowthPineTaiga
        | StructureKeys::AbandonedCampOldGrowthSpruceTaiga
        | StructureKeys::AbandonedCampPaleGarden
        | StructureKeys::AbandonedCampSavanna
        | StructureKeys::AbandonedCampSnowyTaiga
        | StructureKeys::AbandonedCampSparseJungle
        | StructureKeys::AbandonedCampSwamp
        | StructureKeys::AbandonedCampTaiga
        | StructureKeys::AbandonedCampWindsweptForest
        | StructureKeys::AbandonedCampWoodedBadlands
        | StructureKeys::AncientCity
        | StructureKeys::BastionRemnant
        | StructureKeys::PillagerOutpost
        | StructureKeys::TrailRuins
        | StructureKeys::TrialChambers => {
            let (Some(start_pool), Some(size)) = (structure.start_pool, structure.size) else {
                return None;
            };
            let mut generator =
                JigsawGenerator::new(start_pool, size).with_pool_aliases(structure.pool_aliases);
            if structure.use_expansion_hack.unwrap_or(false) {
                generator = generator.with_expansion_hack(true);
            }
            if let Some(start_jigsaw_name) = structure.start_jigsaw_name {
                generator = generator.with_start_jigsaw(start_jigsaw_name);
            }
            generator.get_structure_position_checked(context, is_valid_biome)
        }
        StructureKeys::Shipwreck | StructureKeys::ShipwreckBeached => {
            let generator = ShipwreckGenerator {
                is_beached: structure
                    .is_beached
                    .unwrap_or(key == StructureKeys::ShipwreckBeached),
            };
            generator
                .get_structure_position(context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::RuinedPortal
        | StructureKeys::RuinedPortalDesert
        | StructureKeys::RuinedPortalJungle
        | StructureKeys::RuinedPortalSwamp
        | StructureKeys::RuinedPortalMountain
        | StructureKeys::RuinedPortalOcean
        | StructureKeys::RuinedPortalNether => {
            let generator = RuinedPortalGenerator {
                variant: key,
                is_cold,
            };
            generator
                .get_structure_position(context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::OceanRuinCold | StructureKeys::OceanRuinWarm => {
            let generator = OceanRuinGenerator {
                is_warm: structure
                    .biome_temp
                    .map_or(key == StructureKeys::OceanRuinWarm, |t| t == "warm"),
            };
            generator
                .get_structure_position(context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
        StructureKeys::EndCity => EndCityGenerator
            .get_structure_position(context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::Mansion => MansionGenerator
            .get_structure_position(context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::Monument => OceanMonumentGenerator
            .get_structure_position(context)
            .filter(|pos| is_valid_biome(pos.start_pos)),
        StructureKeys::Mineshaft | StructureKeys::MineshaftMesa => {
            let generator = MineshaftGenerator {
                is_mesa: structure
                    .mineshaft_type
                    .map_or(key == StructureKeys::MineshaftMesa, |t| t == "mesa"),
            };
            generator
                .get_structure_position(context)
                .filter(|pos| is_valid_biome(pos.start_pos))
        }
    }
}

#[must_use]
pub fn try_generate_structure(
    key: &StructureKeys,
    structure: &Structure,
    seed: i64,
    chunk: &ProtoChunk,
    sea_level: i32,
    height_sampler: Option<&mut dyn crate::generation::structure::structures::HeightSampler>,
) -> Option<StructurePosition> {
    let biomes = get_tag_ids(
        RegistryKey::WorldgenBiome,
        structure
            .biomes
            .strip_prefix('#')
            .unwrap_or(structure.biomes),
    )?;
    let random = create_chunk_random(seed, chunk.x, chunk.z);
    let context = StructureGeneratorContext {
        seed,
        chunk_x: chunk.x,
        chunk_z: chunk.z,
        random,
        sea_level,
        min_y: (chunk.generation_bottom_y() as i32).max(chunk.bottom_y() as i32),
        height: chunk.generation_height().min(chunk.height()),
        height_sampler,
        structure_key: Some(*key),
    };
    let biome_id_at = |pos: Vector3<i32>| {
        let biome_y = biome_coords::from_block(pos.y);
        let biome_height = (chunk.height() >> 2) as i32;
        let biome_bottom = biome_coords::from_block(chunk.bottom_y() as i32);
        let clamped_biome_y = biome_y.clamp(biome_bottom, biome_bottom + biome_height - 1);

        chunk.get_biome_id(
            biome_coords::from_block(pos.x),
            clamped_biome_y,
            biome_coords::from_block(pos.z),
        )
    };
    let is_cold = |pos: Vector3<i32>| {
        Biome::from_id(biome_id_at(pos)).is_some_and(|biome| {
            !biome
                .weather
                .warm_enough_to_rain(pos.x, pos.y, pos.z, sea_level)
        })
    };
    generate_structure_position_checked(*key, structure, context, Some(&is_cold), |start_pos| {
        biomes.contains(&u16::from(biome_id_at(start_pos.0)))
    })
}

#[must_use]
pub fn lazily_generate_structure(
    key: &StructureKeys,
    structure: &Structure,
    mut context: StructureGeneratorContext, // Replaces 5 separate arguments!
    biome_supplier: &dyn BiomeSupplier,
    multi_noise_sampler: &mut MultiNoiseSampler,
) -> Option<StructurePosition> {
    let biomes = get_tag_ids(
        RegistryKey::WorldgenBiome,
        structure
            .biomes
            .strip_prefix('#')
            .unwrap_or(structure.biomes),
    )?;
    if *key == StructureKeys::Monument {
        let center_x = crate::generation::positions::chunk_pos::get_center_x(context.chunk_x);
        let center_z = crate::generation::positions::chunk_pos::get_center_z(context.chunk_z);
        let start_y = context
            .height_sampler
            .as_deref_mut()
            .map_or(context.sea_level, |sampler| {
                sampler.estimate_ocean_floor_height(center_x, center_z)
            });
        if !has_valid_biomes(
            biome_supplier,
            multi_noise_sampler,
            context.chunk_x,
            context.chunk_z,
            context.sea_level,
            start_y,
        ) {
            return None;
        }
    }

    let sea_level = context.sea_level;
    let multi_noise_sampler = RefCell::new(multi_noise_sampler);
    let biome_at = |pos: Vector3<i32>| {
        biome_supplier.biome(
            biome_coords::from_block(pos.x),
            biome_coords::from_block(pos.y),
            biome_coords::from_block(pos.z),
            &mut multi_noise_sampler.borrow_mut(),
        )
    };
    // Vanilla `RuinedPortalStructure.isCold`.
    let is_cold = |pos: Vector3<i32>| {
        !biome_at(pos)
            .weather
            .warm_enough_to_rain(pos.x, pos.y, pos.z, sea_level)
    };
    generate_structure_position_checked(*key, structure, context, Some(&is_cold), |start_pos| {
        biomes.contains(&(biome_at(start_pos.0).id as u16))
    })
}

/// The chunk data for the structure `starts` made in `chunk` and the `references` reaching
/// into it (vanilla `ChunkAccess.getAllStarts` and `getAllReferences`).
pub(crate) fn build_chunk_structures<'a>(
    chunk: Vector2<i32>,
    starts: impl Iterator<Item = (StructureKeys, &'a Arc<Mutex<StructurePiecesCollector>>)>,
    references: &[StructureReference],
) -> ChunkStructures {
    let starts: Box<[StructureStart]> = starts
        .filter_map(|(structure, collector)| {
            let mut collector = collector
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let shape = collector.start_shape(structure)?;
            Some(StructureStart::new(
                structure,
                Some(shape),
                collector.create_start_tag(structure, chunk),
            ))
        })
        .collect();

    let mut grouped: Vec<(StructureKeys, Vec<ReferencedStart>)> = Vec::new();
    for (structure, start_chunk, collector) in references {
        let Some(shape) = collector
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .start_shape(*structure)
        else {
            continue;
        };
        let reference = ReferencedStart::resolved(*start_chunk, shape);
        match grouped.iter_mut().find(|(key, _)| key == structure) {
            Some((_, list)) => {
                if !list.iter().any(|start| start.chunk == *start_chunk) {
                    list.push(reference);
                }
            }
            None => grouped.push((*structure, vec![reference])),
        }
    }

    ChunkStructures {
        starts,
        references: grouped
            .into_iter()
            .map(|(structure, starts)| StructureReferences {
                structure,
                starts: starts.into_boxed_slice(),
            })
            .collect(),
    }
}

/// The structure data a proto chunk keeps once it becomes a full chunk: what its structure
/// stages made, or what it was loaded with when they did not run again.
pub(crate) fn proto_chunk_structures(chunk: &mut ProtoChunk) -> ChunkStructures {
    if chunk.structure_starts.is_empty()
        && chunk.structure_references.is_empty()
        && let Some(saved) = chunk.saved_structures.take()
    {
        return saved;
    }
    build_chunk_structures(
        Vector2::new(chunk.x, chunk.z),
        chunk
            .structure_starts
            .iter()
            .filter_map(|(structure, instance)| match instance {
                StructureInstance::Start(position) => Some((*structure, &position.collector)),
                StructureInstance::Reference(_) => None,
            }),
        &chunk.structure_references,
    )
}
