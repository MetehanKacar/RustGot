//! Looking up the structure starts loaded chunks reference, like vanilla `StructureManager`.

use std::sync::{
    Arc, Weak,
    atomic::{AtomicUsize, Ordering},
};

use dashmap::DashMap;
use pumpkin_data::chunk::ChunkStatus;
use pumpkin_data::structures::StructureKeys;
use pumpkin_util::math::{position::BlockPos, vector2::Vector2};
use rustc_hash::{FxBuildHasher, FxHashSet};
use tracing::warn;

use crate::chunk::io::{FileIO, LoadedData, run_blocking};
use crate::chunk::structures::{ChunkStructures, StructureStartShape};
use crate::chunk::{ChunkData, io::Dirtiable};
use crate::chunk_system::StagedChunkEnum;
use crate::level::{Level, SyncChunk};

/// How many shapes are added between sweeps of the entries no chunk uses any more.
const SWEEP_INTERVAL: usize = 1024;

type StartKey = (StructureKeys, Vector2<i32>);

/// Shapes of structure starts by structure and start chunk, held weakly: the chunks that
/// reference a start keep it alive.
#[derive(Default)]
pub(crate) struct StructureStartShapes {
    shapes: DashMap<StartKey, Weak<StructureStartShape>, FxBuildHasher>,
    inserts: AtomicUsize,
}

impl StructureStartShapes {
    fn get(&self, key: &StartKey) -> Option<Arc<StructureStartShape>> {
        self.shapes.get(key).and_then(|shape| shape.upgrade())
    }

    fn insert(&self, key: StartKey, shape: &Arc<StructureStartShape>) {
        self.shapes.insert(key, Arc::downgrade(shape));
        if self.inserts.fetch_add(1, Ordering::Relaxed) % SWEEP_INTERVAL == SWEEP_INTERVAL - 1 {
            self.shapes.retain(|_, shape| shape.strong_count() > 0);
        }
    }

    fn insert_starts(&self, chunk: Vector2<i32>, structures: &ChunkStructures) {
        for start in &structures.starts {
            if let Some(shape) = &start.shape {
                self.insert((start.structure, chunk), shape);
            }
        }
    }
}

/// Whether the structure stages ran for a chunk saved with this status. Vanilla makes starts
/// before the biomes, this server after them.
fn has_structure_starts(chunk: &ChunkData) -> bool {
    match chunk.status {
        ChunkStatus::Empty => false,
        ChunkStatus::StructureStarts | ChunkStatus::StructureReferences => true,
        status => StagedChunkEnum::from(status) >= StagedChunkEnum::StructureStart,
    }
}

impl Level {
    fn loaded_chunk(&self, pos: Vector2<i32>) -> Option<SyncChunk> {
        self.loaded_chunks
            .get(&pos)
            .map(|chunk| chunk.value().clone())
    }

    /// Calls `f` with the structure data of the loaded chunk at `pos`.
    pub fn with_chunk_structures<R>(
        &self,
        pos: &Vector2<i32>,
        f: impl FnOnce(&ChunkStructures) -> R,
    ) -> Option<R> {
        let chunk = self.loaded_chunk(*pos)?;
        chunk.structures.get().map(f)
    }

    /// Whether `matches` accepts one of the valid starts referenced in the chunk at `pos`
    /// (vanilla `StructureManager.startsForStructure`).
    pub fn any_structure_start_at(
        &self,
        pos: &BlockPos,
        mut matches: impl FnMut(StructureKeys, &StructureStartShape) -> bool,
    ) -> bool {
        let chunk = Vector2::new(pos.0.x >> 4, pos.0.z >> 4);
        self.with_chunk_structures(&chunk, |structures| {
            structures.references.iter().any(|reference| {
                reference.starts.iter().any(|start| {
                    start
                        .shape()
                        .is_some_and(|shape| matches(reference.structure, shape))
                })
            })
        })
        .unwrap_or(false)
    }

    /// Vanilla `StructureManager.getStructureWithPieceAt(pos, predicate).isValid()`.
    pub fn has_structure_piece_at(
        &self,
        pos: &BlockPos,
        wanted: impl Fn(StructureKeys) -> bool,
    ) -> bool {
        self.any_structure_start_at(pos, |structure, shape| {
            wanted(structure) && shape.has_piece_at(pos)
        })
    }

    /// Vanilla `StructureManager.getStructureAt(pos, structure).isValid()`.
    pub fn is_inside_structure(&self, pos: &BlockPos, structure: StructureKeys) -> bool {
        self.any_structure_start_at(pos, |found, shape| {
            found == structure && shape.bounding_box().contains_pos(&pos.0)
        })
    }

    /// The shape of the `structure` start in `start_chunk` when a loaded chunk already has it.
    fn known_start_shape(
        &self,
        structure: StructureKeys,
        start_chunk: Vector2<i32>,
    ) -> Option<Arc<StructureStartShape>> {
        let key = (structure, start_chunk);
        if let Some(shape) = self.structure_start_shapes.get(&key) {
            return Some(shape);
        }
        let shape = self
            .loaded_chunk(start_chunk)?
            .structures
            .get()?
            .start(structure)?
            .shape
            .clone()?;
        self.structure_start_shapes.insert(key, &shape);
        Some(shape)
    }

    /// Fills in the starts a chunk read from disk references, which vanilla reads from the
    /// start chunks when it needs them. A chunk saved without structure data gets it from the
    /// seed again.
    pub(crate) async fn load_chunk_structures(self: Arc<Self>, chunk: SyncChunk) {
        let pos = Vector2::new(chunk.x, chunk.z);
        if chunk.structures.get().is_none() {
            if StagedChunkEnum::from(chunk.status) < StagedChunkEnum::Features {
                // The structure stages run again for this chunk.
                return;
            }
            let level = self.clone();
            let Ok(Some(structures)) =
                run_blocking(move || level.world_gen.load().chunk_structures(pos)).await
            else {
                return;
            };
            if chunk.structures.set(structures).is_ok() {
                chunk.mark_dirty(true);
            }
        }
        let Some(structures) = chunk.structures.get() else {
            return;
        };
        self.structure_start_shapes.insert_starts(pos, structures);

        let mut missing = Vec::new();
        for reference in &structures.references {
            for start in &reference.starts {
                if start.shape().is_some() {
                    continue;
                }
                let shape = if start.chunk == pos {
                    structures
                        .start(reference.structure)
                        .and_then(|own| own.shape.clone())
                } else {
                    self.known_start_shape(reference.structure, start.chunk)
                };
                match shape {
                    Some(shape) => start.set_shape(shape),
                    None if start.chunk != pos => missing.push((reference.structure, start)),
                    None => {}
                }
            }
        }
        if missing.is_empty() {
            return;
        }

        let start_chunks: Vec<Vector2<i32>> = missing
            .iter()
            .map(|(_, start)| start.chunk)
            .collect::<FxHashSet<_>>()
            .into_iter()
            .collect();
        let regenerate = self.read_saved_starts(&start_chunks).await;

        let to_generate: Vec<StartKey> = missing
            .iter()
            .map(|(structure, start)| (*structure, start.chunk))
            .filter(|key| self.structure_start_shapes.get(key).is_none())
            .filter(|(_, start_chunk)| regenerate.contains(start_chunk))
            .collect();
        // Held until the references below take their own copies.
        let generated = if to_generate.is_empty() {
            Vec::new()
        } else {
            let level = self.clone();
            run_blocking(move || {
                let world_gen = level.world_gen.load();
                to_generate
                    .into_iter()
                    .filter_map(|key| {
                        let shape = world_gen.structure_start_shape(key.0, key.1)?;
                        level.structure_start_shapes.insert(key, &shape);
                        Some(shape)
                    })
                    .collect::<Vec<_>>()
            })
            .await
            .unwrap_or_default()
        };
        for (structure, start) in missing {
            if let Some(shape) = self.structure_start_shapes.get(&(structure, start.chunk)) {
                start.set_shape(shape);
            }
        }
        drop(generated);
    }

    /// Reads the starts saved in `start_chunks` into the shared shapes, returning the chunks
    /// whose saved data cannot say which starts they have.
    async fn read_saved_starts(&self, start_chunks: &[Vector2<i32>]) -> FxHashSet<Vector2<i32>> {
        let (send, mut recv) = tokio::sync::mpsc::channel(start_chunks.len());
        self.chunk_saver
            .fetch_chunks(&self.level_folder, start_chunks, send)
            .await;
        let mut regenerate = FxHashSet::default();
        while let Some(loaded) = recv.recv().await {
            match loaded {
                LoadedData::Loaded(start_chunk) => {
                    let start_pos = Vector2::new(start_chunk.x, start_chunk.z);
                    match start_chunk.structures.get() {
                        Some(saved) if has_structure_starts(&start_chunk) => {
                            self.structure_start_shapes.insert_starts(start_pos, saved);
                        }
                        _ => {
                            regenerate.insert(start_pos);
                        }
                    }
                }
                LoadedData::Missing(start_pos) => {
                    regenerate.insert(start_pos);
                }
                LoadedData::Error((start_pos, err)) => {
                    warn!(
                        "Failed to read structure starts of chunk [{}, {}]: {err}",
                        start_pos.x, start_pos.y
                    );
                }
            }
        }
        regenerate
    }
}
