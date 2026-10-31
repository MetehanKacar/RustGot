use pumpkin_util::{math::position::BlockPos, random::RandomGenerator};

use crate::generation::{
    feature::placed_features::PlacedFeature, proto_chunk::GenerationCache,
};
use crate::world::WorldPortalExt;

/// Vanilla `SequenceFeature` (26.x): places each feature at the same origin in order and
/// stops at the first one that fails. Used by sulfur pools and springs and sculk patches.
pub struct SequenceFeature {
    pub features: Vec<PlacedFeature>,
}

impl SequenceFeature {
    #[allow(clippy::too_many_arguments)]
    pub fn generate<T: GenerationCache>(
        &self,
        chunk: &mut T,
        block_registry: &dyn WorldPortalExt,
        min_y: i8,
        height: u16,
        feature_name: pumpkin_data::placed_feature::PlacedFeature,
        random: &mut RandomGenerator,
        pos: BlockPos,
    ) -> bool {
        self.features.iter().all(|feature| {
            feature.generate(
                chunk,
                block_registry,
                min_y,
                height,
                feature_name,
                random,
                pos,
            )
        })
    }
}
