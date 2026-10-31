//! Speleothem features (vanilla `SpeleothemUtils` and friends).
//!
//! 26.x generalised dripstone: the same features build sulfur spikes in sulfur caves,
//! configured by `base_block`, `pointed_block` and `replaceable_blocks`.

use crate::generation::proto_chunk::GenerationCache;
use pumpkin_data::block_properties::{
    PointedDripstoneLikeProperties, SpeleothemThickness, VerticalDirection,
};
use pumpkin_data::tag;
use pumpkin_data::tag::Taggable;
use pumpkin_data::{Block, BlockDirection, BlockId, BlockState};
use pumpkin_util::math::position::BlockPos;

pub mod cluster;
pub mod large;
pub mod small;

/// The blocks a speleothem feature builds with.
pub struct SpeleothemBlocks {
    pub base: &'static Block,
    pub pointed: &'static Block,
    pub replaceable: tag::Tag,
}

impl SpeleothemBlocks {
    pub const DRIPSTONE: Self = Self {
        base: &Block::DRIPSTONE_BLOCK,
        pointed: &Block::POINTED_DRIPSTONE,
        replaceable: tag::Block::MINECRAFT_DRIPSTONE_REPLACEABLE_BLOCKS,
    };
}

/// Vanilla `SpeleothemUtils.isBase`.
pub(super) fn is_base(blocks: &SpeleothemBlocks, id: BlockId) -> bool {
    id == blocks.base.id || id.has_tag(blocks.replaceable)
}

/// Vanilla `SpeleothemUtils.placeBaseBlockIfPossible`.
pub(super) fn place_base_block_if_possible<T: GenerationCache>(
    chunk: &mut T,
    pos: BlockPos,
    blocks: &SpeleothemBlocks,
) -> bool {
    let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    if block.has_tag(blocks.replaceable) {
        chunk.set_block_state(&pos.0, blocks.base.default_state);
        return true;
    }
    false
}

/// Vanilla `SpeleothemUtils.isEmptyOrWater`.
pub(super) fn is_empty_or_water<T: GenerationCache>(chunk: &T, pos: BlockPos) -> bool {
    let block = GenerationCache::get_block_state(chunk, &pos.0).to_block_id();
    block == BlockId::AIR
        || block == BlockId::CAVE_AIR
        || block == BlockId::VOID_AIR
        || block == BlockId::WATER
}

/// Vanilla `SpeleothemUtils.growSpeleothem`: builds base-to-tip from `start_pos` towards
/// `tip_direction`, but only if the block behind the start can hold it.
pub(super) fn grow_speleothem<T: GenerationCache>(
    chunk: &mut T,
    start_pos: BlockPos,
    tip_direction: BlockDirection,
    height: i32,
    merged_tip: bool,
    blocks: &SpeleothemBlocks,
) {
    let behind = start_pos.offset(tip_direction.opposite().to_offset());
    if !is_base(
        blocks,
        GenerationCache::get_block_state(chunk, &behind.0).to_block_id(),
    ) {
        return;
    }

    let vertical_direction = match tip_direction {
        BlockDirection::Down => VerticalDirection::Down,
        _ => VerticalDirection::Up,
    };
    let mut thicknesses = Vec::with_capacity(usize::try_from(height).unwrap_or(0));
    if height >= 3 {
        thicknesses.push(SpeleothemThickness::Base);
        for _ in 0..(height - 3) {
            thicknesses.push(SpeleothemThickness::Middle);
        }
    }
    if height >= 2 {
        thicknesses.push(SpeleothemThickness::Frustum);
    }
    if height >= 1 {
        thicknesses.push(if merged_tip {
            SpeleothemThickness::TipMerge
        } else {
            SpeleothemThickness::Tip
        });
    }

    let mut pos = start_pos;
    for thickness in thicknesses {
        let (fluid, _) = GenerationCache::get_fluid_and_fluid_state(chunk, &pos.0);
        let mut props = PointedDripstoneLikeProperties::default(blocks.pointed);
        props.thickness = thickness;
        props.vertical_direction = vertical_direction;
        // Vanilla `level.isWaterAt(pos)`.
        props.waterlogged = fluid.has_tag(&tag::Fluid::MINECRAFT_WATER);
        chunk.set_block_state(
            &pos.0,
            BlockState::from_id(props.to_state_id(blocks.pointed)),
        );
        pos = pos.offset(tip_direction.to_offset());
    }
}
