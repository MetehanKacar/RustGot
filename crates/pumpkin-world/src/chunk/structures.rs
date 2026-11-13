//! Structure starts and references of a chunk (vanilla `ChunkAccess.structureStarts` and
//! `structuresRefences`), saved in the chunk's `structures` tag like
//! `SerializableChunkData.packStructureData` does.

use std::io::Cursor;
use std::sync::{Arc, OnceLock};

use pumpkin_data::structures::{Structure, StructureKeys, TerrainAdaptation};
use pumpkin_nbt::{Nbt, compound::NbtCompound, deserializer::NbtReadHelperJava, tag::NbtTag};
use pumpkin_util::math::{block_box::BlockBox, position::BlockPos, vector2::Vector2};
use tracing::{error, warn};

/// `StructureStart.INVALID_START_ID`.
const INVALID_START_ID: &str = "INVALID";
/// A reference further away than this many chunks is dropped on load
/// (`SerializableChunkData.unpackStructureReferences`).
const MAX_REFERENCE_DISTANCE: i32 = 8;
/// `Structure.adjustBoundingBox` grows terrain-adapting structures by this much.
const TERRAIN_ADAPTATION_INFLATE: i32 = 12;

/// The piece boxes of a structure start, which is all `StructureManager` needs to answer
/// whether a position is in the structure.
#[derive(Debug, PartialEq, Eq)]
pub struct StructureStartShape {
    bounding_box: BlockBox,
    pieces: Box<[BlockBox]>,
}

impl StructureStartShape {
    /// The shape of a start with these pieces, or `None` without pieces (vanilla's invalid start).
    #[must_use]
    pub fn new(structure: StructureKeys, pieces: Box<[BlockBox]>) -> Option<Self> {
        let union = BlockBox::encompass_all(pieces.iter().copied())?;
        // Vanilla `Structure.adjustBoundingBox`.
        let bounding_box =
            if Structure::get(&structure).terrain_adaptation == TerrainAdaptation::None {
                union
            } else {
                union.expand(
                    TERRAIN_ADAPTATION_INFLATE,
                    TERRAIN_ADAPTATION_INFLATE,
                    TERRAIN_ADAPTATION_INFLATE,
                )
            };
        Some(Self {
            bounding_box,
            pieces,
        })
    }

    /// Vanilla `StructureStart.getBoundingBox`.
    #[must_use]
    pub const fn bounding_box(&self) -> &BlockBox {
        &self.bounding_box
    }

    #[must_use]
    pub fn pieces(&self) -> &[BlockBox] {
        &self.pieces
    }

    /// Vanilla `StructureManager.structureHasPieceAt`.
    #[must_use]
    pub fn has_piece_at(&self, pos: &BlockPos) -> bool {
        self.pieces.iter().any(|piece| piece.contains_pos(&pos.0))
    }
}

/// A structure start, kept by the chunk it starts in.
#[derive(Clone)]
pub struct StructureStart {
    pub structure: StructureKeys,
    /// `None` for vanilla's invalid start, which has no pieces.
    pub shape: Option<Arc<StructureStartShape>>,
    /// The start's tag (`StructureStart.createTag`), serialized so a loaded chunk keeps the
    /// pieces only as bytes.
    tag: Box<[u8]>,
}

impl StructureStart {
    #[must_use]
    pub fn new(
        structure: StructureKeys,
        shape: Option<Arc<StructureStartShape>>,
        tag: NbtCompound,
    ) -> Self {
        Self {
            structure,
            shape,
            tag: Nbt::new(String::new(), tag).write_unnamed().to_vec().into(),
        }
    }

    /// Vanilla `StructureStart.loadStaticStart`, keeping only the piece boxes. `None` where
    /// vanilla drops the start.
    fn load(structure: StructureKeys, tag: NbtCompound) -> Option<Self> {
        let id = tag.get_string("id").unwrap_or_default();
        if id == INVALID_START_ID {
            return Some(Self::new(structure, None, tag));
        }
        let Some(start_structure) = StructureKeys::from_name(id) else {
            error!("Unknown stucture id: {id}");
            return None;
        };
        let pieces: Box<[BlockBox]> = tag
            .get_list("Children")
            .unwrap_or_default()
            .iter()
            .filter_map(|child| match child {
                NbtTag::Compound(piece) => piece.get_int_array("BB").and_then(block_box_from_ints),
                _ => None,
            })
            .collect();
        let shape = StructureStartShape::new(start_structure, pieces).map(Arc::new);
        Some(Self::new(structure, shape, tag))
    }

    /// The start's tag as it is saved in its chunk.
    #[must_use]
    pub fn tag(&self) -> Option<NbtCompound> {
        let mut cursor = Cursor::new(&self.tag[..]);
        let mut reader = NbtReadHelperJava::new(&mut cursor);
        match Nbt::read_unnamed(&mut reader) {
            Ok(nbt) => Some(nbt.root_tag),
            Err(err) => {
                error!(
                    "Failed to read the saved {} structure start: {err}",
                    self.structure.to_name()
                );
                None
            }
        }
    }
}

/// A chunk holding a start that reaches into this chunk, and the start's shape once it is known.
#[derive(Clone)]
pub struct ReferencedStart {
    pub chunk: Vector2<i32>,
    shape: OnceLock<Arc<StructureStartShape>>,
}

impl ReferencedStart {
    #[must_use]
    pub const fn new(chunk: Vector2<i32>) -> Self {
        Self {
            chunk,
            shape: OnceLock::new(),
        }
    }

    #[must_use]
    pub fn resolved(chunk: Vector2<i32>, shape: Arc<StructureStartShape>) -> Self {
        let reference = Self::new(chunk);
        let _ = reference.shape.set(shape);
        reference
    }

    /// The referenced start's shape, `None` while it is unknown or the start is missing.
    #[must_use]
    pub fn shape(&self) -> Option<&Arc<StructureStartShape>> {
        self.shape.get()
    }

    pub fn set_shape(&self, shape: Arc<StructureStartShape>) {
        let _ = self.shape.set(shape);
    }
}

/// The starts of one structure that reach into a chunk.
#[derive(Clone)]
pub struct StructureReferences {
    pub structure: StructureKeys,
    pub starts: Box<[ReferencedStart]>,
}

/// The structure data of a chunk.
#[derive(Clone, Default)]
pub struct ChunkStructures {
    /// Starts made in this chunk.
    pub starts: Box<[StructureStart]>,
    /// Starts, in this or other chunks, whose bounding box reaches into this chunk.
    pub references: Box<[StructureReferences]>,
}

impl ChunkStructures {
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.starts.is_empty() && self.references.is_empty()
    }

    #[must_use]
    pub fn start(&self, structure: StructureKeys) -> Option<&StructureStart> {
        self.starts
            .iter()
            .find(|start| start.structure == structure)
    }

    /// Vanilla `SerializableChunkData.unpackStructureStart` and `unpackStructureReferences`.
    #[must_use]
    pub fn from_nbt(mut tag: NbtCompound, chunk: Vector2<i32>) -> Self {
        let mut starts = Vec::new();
        if let Some(NbtTag::Compound(starts_tag)) = tag.child_tags.remove("starts") {
            for (key, start_tag) in starts_tag.child_tags {
                let Some(structure) = StructureKeys::from_name(&key) else {
                    error!("Unknown structure start: {key}");
                    continue;
                };
                let NbtTag::Compound(start_tag) = start_tag else {
                    continue;
                };
                if let Some(start) = StructureStart::load(structure, start_tag) {
                    starts.push(start);
                }
            }
        }

        let mut references = Vec::new();
        if let Some(references_tag) = tag.get_compound("References") {
            for (key, value) in &references_tag.child_tags {
                let Some(structure) = StructureKeys::from_name(key) else {
                    warn!(
                        "Found reference to unknown structure '{key}' in chunk [{}, {}], discarding",
                        chunk.x, chunk.y
                    );
                    continue;
                };
                let NbtTag::LongArray(packed) = value else {
                    continue;
                };
                let referenced: Box<[ReferencedStart]> = packed
                    .iter()
                    .map(|&packed| unpack_chunk_pos(packed))
                    .filter(|reference| {
                        let in_range = (reference.x - chunk.x).abs() <= MAX_REFERENCE_DISTANCE
                            && (reference.y - chunk.y).abs() <= MAX_REFERENCE_DISTANCE;
                        if !in_range {
                            warn!(
                                "Found invalid structure reference [ {key} @ [{}, {}] ] for chunk [{}, {}].",
                                reference.x, reference.y, chunk.x, chunk.y
                            );
                        }
                        in_range
                    })
                    .map(ReferencedStart::new)
                    .collect();
                references.push(StructureReferences {
                    structure,
                    starts: referenced,
                });
            }
        }

        Self {
            starts: starts.into_boxed_slice(),
            references: references.into_boxed_slice(),
        }
    }

    /// Vanilla `SerializableChunkData.packStructureData`.
    #[must_use]
    pub fn to_nbt(&self) -> NbtCompound {
        let mut starts = NbtCompound::new();
        for start in &self.starts {
            if let Some(tag) = start.tag() {
                starts.put_compound(&structure_id(start.structure), tag);
            }
        }
        let mut references = NbtCompound::new();
        for reference in &self.references {
            if !reference.starts.is_empty() {
                references.put(
                    &structure_id(reference.structure),
                    NbtTag::LongArray(
                        reference
                            .starts
                            .iter()
                            .map(|start| pack_chunk_pos(start.chunk))
                            .collect(),
                    ),
                );
            }
        }
        let mut tag = NbtCompound::new();
        tag.put_compound("starts", starts);
        tag.put_compound("References", references);
        tag
    }
}

/// The structure's registry id, as vanilla keys `starts` and `References` with it.
#[must_use]
pub fn structure_id(structure: StructureKeys) -> String {
    format!("minecraft:{}", structure.to_name())
}

/// Vanilla `ChunkPos.pack`.
#[must_use]
pub const fn pack_chunk_pos(pos: Vector2<i32>) -> i64 {
    (pos.x as u32 as i64) | ((pos.y as u32 as i64) << 32)
}

/// Vanilla `ChunkPos.unpack`.
#[must_use]
pub const fn unpack_chunk_pos(packed: i64) -> Vector2<i32> {
    Vector2::new(packed as i32, (packed >> 32) as i32)
}

/// Vanilla `BoundingBox.CODEC`: `[minX, minY, minZ, maxX, maxY, maxZ]`.
const fn block_box_from_ints(ints: &[i32]) -> Option<BlockBox> {
    let &[min_x, min_y, min_z, max_x, max_y, max_z] = ints else {
        return None;
    };
    Some(BlockBox::new(min_x, min_y, min_z, max_x, max_y, max_z))
}

/// Vanilla `BoundingBox.CODEC`.
#[must_use]
pub fn block_box_to_nbt(bounding_box: &BlockBox) -> NbtTag {
    NbtTag::IntArray(vec![
        bounding_box.min.x,
        bounding_box.min.y,
        bounding_box.min.z,
        bounding_box.max.x,
        bounding_box.max.y,
        bounding_box.max.z,
    ])
}

#[cfg(test)]
mod tests {
    use pumpkin_data::dimension::Dimension;
    use pumpkin_util::world_seed::Seed;

    use super::*;
    use crate::chunk_system::{
        StagedChunkEnum, chunk_state::Chunk, generation::generate_single_chunk,
    };
    use crate::generation::get_world_gen;
    use crate::chunk_system::generation::tests::BlockRegistry;

    /// `structures` of chunk [-17, -14] in a Purpur 26.3 world with seed 1792752931139676432.
    fn purpur_buried_treasure_chunk() -> NbtCompound {
        let mut piece = NbtCompound::new();
        piece.put_string("id", "minecraft:btp".to_string());
        piece.put("BB", NbtTag::IntArray(vec![-263, 60, -215, -263, 60, -215]));
        piece.put_int("O", -1);
        piece.put_int("GD", 0);
        let mut start = NbtCompound::new();
        start.put_string("id", "minecraft:buried_treasure".to_string());
        start.put_int("ChunkX", -17);
        start.put_int("ChunkZ", -14);
        start.put_int("references", 0);
        start.put_list("Children", vec![NbtTag::Compound(piece)]);
        let mut starts = NbtCompound::new();
        starts.put_compound("minecraft:buried_treasure", start);
        let mut references = NbtCompound::new();
        references.put(
            "minecraft:buried_treasure",
            NbtTag::LongArray(vec![-55_834_574_865]),
        );
        let mut tag = NbtCompound::new();
        tag.put_compound("starts", starts);
        tag.put_compound("References", references);
        tag
    }

    #[test]
    fn purpur_structures_load_and_save_unchanged() {
        let tag = purpur_buried_treasure_chunk();
        let structures = ChunkStructures::from_nbt(tag.clone(), Vector2::new(-17, -14));

        let reference = &structures.references[0];
        assert_eq!(reference.structure, StructureKeys::BuriedTreasure);
        assert_eq!(reference.starts[0].chunk, Vector2::new(-17, -14));
        assert_eq!(unpack_chunk_pos(-68_719_476_759), Vector2::new(-23, -17));

        let shape = structures
            .start(StructureKeys::BuriedTreasure)
            .and_then(|start| start.shape.as_ref())
            .expect("the start has a piece");
        assert!(shape.has_piece_at(&BlockPos::new(-263, 60, -215)));
        assert!(!shape.has_piece_at(&BlockPos::new(-263, 61, -215)));

        assert_eq!(structures.to_nbt(), tag);
    }

    #[test]
    fn generated_chunk_saves_structures_like_purpur() {
        let world_gen = get_world_gen(
            Seed(1_792_752_931_139_676_432),
            Dimension::OVERWORLD,
            false,
            Vec::new(),
            String::new(),
        );
        let chunk = generate_single_chunk(
            &world_gen,
            &BlockRegistry,
            -17,
            -14,
            StagedChunkEnum::Full,
        );
        let Chunk::Level(chunk) = chunk else {
            panic!("full generation must return a level chunk");
        };
        let structures = chunk.structures.get().expect("generated chunks keep structures");
        assert_eq!(structures.to_nbt(), purpur_buried_treasure_chunk());
    }
}
