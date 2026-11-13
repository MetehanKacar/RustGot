pub mod bamboo;
pub mod basalt_columns;
pub mod basalt_pillar;
pub mod block_column;
pub mod block_pile;
pub mod blue_ice;
pub mod bonus_chest;
pub mod chorus_plant;
pub mod coral;
pub mod delta_feature;
pub mod desert_well;
pub mod disk;
pub mod drip_stone;
pub mod end_gateway;
pub mod end_island;
pub mod end_platform;
pub mod end_podium;
pub mod end_spike;
pub mod fallen_tree;
pub mod fill_layer;
pub mod forest_rock;
pub mod fossil;
pub mod freeze_top_layer;
pub mod geode;
pub mod glowstone_blob;
pub mod huge_brown_mushroom;
pub mod huge_fungus;
pub mod huge_red_mushroom;
pub mod ice_spike;
pub mod iceberg;
pub mod kelp;
pub mod lake;
pub mod monster_room;
pub mod multiface_growth;
pub mod nether_forest_vegetation;
pub mod netherrack_replace_blobs;
pub mod ore;
pub mod overlay;
pub mod projected_random_patchy_square;
pub mod random_boolean_selector;
pub mod random_neighbor_spread;
pub mod random_patch;
pub mod random_selector;
pub mod replace_single_block;
pub mod root_system;
pub mod scattered_ore;
pub mod sculk;
pub mod sculk_patch;
pub mod sea_pickle;
pub mod seagrass;
pub mod sequence;
pub mod simple_block;
pub mod simple_random_selector;
pub mod single_block_pillar;
pub mod spring_feature;
pub mod stepped_column_cluster;
pub mod template;
pub mod tree;
pub mod twisting_vines;
pub mod underwater_magma;
pub mod vegetation_patch;
pub mod vines;
pub mod void_start_platform;
pub mod waterlogged_vegetation_patch;
pub mod weeping_vines;
pub mod weighted_random_selector;

mod java_hash_set;

use java_hash_set::{JavaHashSet, JavaKey};
use pumpkin_util::math::position::BlockPos;

/// A hash set of [`BlockPos`] that preserves `OpenJDK` `java.util.HashSet<BlockPos>` iteration order.
#[derive(Clone, Debug, Default)]
pub struct JavaBlockPosHashSet(JavaHashSet<JavaBlockPos>);

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct JavaBlockPos(BlockPos);

impl JavaKey for JavaBlockPos {
    /// Vanilla `Vec3i.hashCode`.
    fn java_hash(&self) -> i32 {
        let pos = self.0.0;
        pos.y
            .wrapping_add(pos.z.wrapping_mul(31))
            .wrapping_mul(31)
            .wrapping_add(pos.x)
    }

    // `BlockPos` inherits `Comparable<Vec3i>`, which Java's `HashMap` does not use for it, so
    // equal hashes fall back to identity hashes; that order is arbitrary in vanilla as well.
    fn tie_break(&self, _other: &Self) -> std::cmp::Ordering {
        std::cmp::Ordering::Less
    }
}

impl JavaBlockPosHashSet {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn insert(&mut self, pos: BlockPos) -> bool {
        self.0.insert(JavaBlockPos(pos))
    }

    #[must_use]
    pub fn contains(&self, pos: &BlockPos) -> bool {
        self.0.contains(&JavaBlockPos(*pos))
    }

    #[must_use]
    pub const fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    #[must_use]
    pub const fn len(&self) -> usize {
        self.0.len()
    }

    /// `iterator().next()` followed by `iterator.remove()`.
    pub fn pop_first(&mut self) -> Option<BlockPos> {
        self.0.pop_first().map(|pos| pos.0)
    }

    pub fn iter(&self) -> impl Iterator<Item = &BlockPos> {
        self.0.iter().map(|pos| &pos.0)
    }

    #[must_use]
    pub fn into_vec(self) -> Vec<BlockPos> {
        self.iter().copied().collect()
    }
}

impl FromIterator<BlockPos> for JavaBlockPosHashSet {
    fn from_iter<T: IntoIterator<Item = BlockPos>>(iter: T) -> Self {
        let mut set = Self::new();
        for pos in iter {
            set.insert(pos);
        }
        set
    }
}

impl<'a> IntoIterator for &'a JavaBlockPosHashSet {
    type Item = &'a BlockPos;
    type IntoIter =
        std::iter::Map<java_hash_set::Iter<'a, JavaBlockPos>, fn(&JavaBlockPos) -> &BlockPos>;

    fn into_iter(self) -> Self::IntoIter {
        const fn unwrap(pos: &JavaBlockPos) -> &BlockPos {
            &pos.0
        }
        self.0.iter().map(unwrap)
    }
}

impl IntoIterator for JavaBlockPosHashSet {
    type Item = BlockPos;
    type IntoIter = std::vec::IntoIter<BlockPos>;

    fn into_iter(self) -> Self::IntoIter {
        self.into_vec().into_iter()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use pumpkin_util::random::{RandomImpl, legacy_rand::LegacyRand};
    use rustc_hash::FxHashSet;

    /// `(seed, FNV-1a of the orders)`, printed by `java.util.HashSet` on `OpenJDK` 25 from a small
    /// program kept outside the repository. It builds the set below from `new Random(seed)`,
    /// records its order, pops half of it through `iterator().remove()` (re-adding a neighbour
    /// every third pop) and records the order again. Seeds from 630 on made tree bins; from 5469
    /// on, a set without tree bins iterates differently.
    const JAVA_ORDERS: [(i64, u64); 15] = [
        (0, 0x8128_1577_9422_c51a),
        (1, 0xe842_ccc5_8782_ae15),
        (2, 0xf9da_6a79_fab6_6767),
        (630, 0xbe40_c366_38ee_744e),
        (1129, 0xf294_3a65_e3e6_c77e),
        (5469, 0x765b_3081_a0c5_700f),
        (15971, 0x6b29_a161_d152_4494),
        (22608, 0x1258_0c52_cad1_c3ba),
        (22947, 0xb2b7_8008_396f_f5b2),
        (25015, 0x82d5_cbd8_9acc_3ab5),
        (26244, 0x3d48_7659_d96c_65d8),
        (29926, 0x0cd4_4855_3f01_9534),
        (33591, 0x4ba3_784a_5b9d_637a),
        (41285, 0x6450_49cc_ad47_2bb1),
        (42678, 0x0bfa_9927_b1cc_69ad),
    ];

    struct Fnv(u64);
    impl Fnv {
        fn mix(&mut self, pos: BlockPos) {
            for v in [pos.0.x, pos.0.y, pos.0.z] {
                for byte in v.to_le_bytes() {
                    self.0 ^= u64::from(byte);
                    self.0 = self.0.wrapping_mul(0x0100_0000_01b3);
                }
            }
        }
    }

    fn add(set: &mut JavaBlockPosHashSet, hashes: &mut FxHashSet<i32>, pos: BlockPos) {
        let hash = JavaBlockPos(pos).java_hash();
        // Equal hashes would order by identity hash in Java; the generator skips them.
        if !set.contains(&pos) && hashes.insert(hash) {
            set.insert(pos);
        }
    }

    fn java_order_hash(seed: i64) -> u64 {
        let mut r = LegacyRand::from_seed(seed as u64);
        let bx = r.next_bounded_i32(60000) - 30000;
        let bz = r.next_bounded_i32(60000) - 30000;
        let by = r.next_bounded_i32(300) - 60;
        let radius = 2 + r.next_bounded_i32(7);
        let layers = 1 + r.next_bounded_i32(3);
        let keep = 50 + r.next_bounded_i32(51);
        let mut set = JavaBlockPosHashSet::new();
        let mut hashes = FxHashSet::default();
        for dy in 0..layers {
            for dx in -radius..=radius {
                for dz in -radius..=radius {
                    if dx * dx + dz * dz <= radius * radius && r.next_bounded_i32(100) < keep {
                        add(
                            &mut set,
                            &mut hashes,
                            BlockPos::new(bx + dx, by + dy, bz + dz),
                        );
                    }
                }
            }
        }
        let mut fnv = Fnv(0xcbf2_9ce4_8422_2325);
        set.iter().for_each(|pos| fnv.mix(*pos));
        for i in 0..set.len() / 2 {
            let pos = set.pop_first().unwrap();
            fnv.mix(pos);
            if i % 3 == 0 {
                let neighbour = BlockPos::new(pos.0.x + 1, pos.0.y, pos.0.z - 1);
                add(&mut set, &mut hashes, neighbour);
            }
        }
        set.iter().for_each(|pos| fnv.mix(*pos));
        fnv.0
    }

    #[test]
    fn iteration_order_matches_java_hash_set() {
        for (seed, expected) in JAVA_ORDERS {
            assert_eq!(java_order_hash(seed), expected, "seed {seed}");
        }
    }
}
