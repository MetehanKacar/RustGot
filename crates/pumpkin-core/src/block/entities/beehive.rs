use super::BlockEntity;
use crate::block::blocks::campfire::is_smokey_pos;
use crate::entity::EntityBase;
use crate::entity::ageable::AgeableMob;
use crate::entity::passive::bee::BeeEntity;
use crate::entity::player::Player;
use crate::entity::r#type::load_entity_from_nbt;
use crate::plugin::api::events::entity::entity_target::TargetReason;
use crate::world::{BlockFlags, World};
use pumpkin_data::Enchantment;
use pumpkin_data::block_properties::BeeNestLikeProperties;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::tag::{self, Taggable};
use pumpkin_data::{Block, HorizontalFacingExt};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_nbt::tag::NbtTag;
use pumpkin_util::math::boundingbox::BoundingBox;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_world::chunk::io::Dirtiable;
use rand::{RngExt, rng};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::Ordering;

/// Vanilla `BeehiveBlockEntity.IGNORED_BEE_TAGS`: state that is not kept while a bee is inside.
const IGNORED_BEE_TAGS: [&str; 25] = [
    "Air",
    "drop_chances",
    "equipment",
    "Brain",
    "CanPickUpLoot",
    "DeathTime",
    "fall_distance",
    "FallFlying",
    "Fire",
    "HurtTime",
    "LeftHanded",
    "Motion",
    "NoGravity",
    "OnGround",
    "PortalCooldown",
    "Pos",
    "Rotation",
    "sleeping_pos",
    "CannotEnterHiveTicks",
    "TicksSincePollination",
    "CropsGrownSincePollination",
    "hive_pos",
    "Passengers",
    "leash",
    "UUID",
];

pub const MAX_OCCUPANTS: usize = 3;
pub const MIN_OCCUPATION_TICKS_NECTARLESS: i32 = 600;
pub const MIN_OCCUPATION_TICKS_NECTAR: i32 = 2400;
/// Ticks a bee stays out of the hive after a smoked hive was emptied.
const STAY_OUT_OF_HIVE_WHEN_SEDATED: i32 = 400;
const MAX_HONEY_LEVEL: u8 = 5;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum BeeReleaseStatus {
    HoneyDelivered,
    BeeReleased,
    Emergency,
}

/// Vanilla `BeehiveBlockEntity.Occupant`: a bee waiting inside, as saved in NBT and item data.
#[derive(Clone)]
pub struct Occupant {
    pub entity_data: NbtCompound,
    pub ticks_in_hive: i32,
    pub min_ticks_in_hive: i32,
}

impl Occupant {
    fn from_nbt(nbt: &NbtCompound) -> Option<Self> {
        // The keys older Pumpkin versions wrote are still read.
        let entity_data = nbt
            .get_compound("entity_data")
            .or_else(|| nbt.get_compound("EntityData"))?
            .clone();
        Some(Self {
            entity_data,
            ticks_in_hive: nbt
                .get_int("ticks_in_hive")
                .or_else(|| nbt.get_int("TicksInHive"))
                .unwrap_or(0),
            min_ticks_in_hive: nbt
                .get_int("min_ticks_in_hive")
                .or_else(|| nbt.get_int("MinOccupationTicks"))
                .unwrap_or(MIN_OCCUPATION_TICKS_NECTARLESS),
        })
    }

    /// Vanilla `Occupant.of`: the bee as it is saved while it waits inside.
    fn of(bee: &BeeEntity) -> Self {
        let mut entity_data = NbtCompound::new();
        EntityBase::write_nbt(bee, &mut entity_data);
        for tag in IGNORED_BEE_TAGS {
            entity_data.child_tags.remove(tag);
        }
        let min_ticks_in_hive = if entity_data.get_bool("HasNectar").unwrap_or(false) {
            MIN_OCCUPATION_TICKS_NECTAR
        } else {
            MIN_OCCUPATION_TICKS_NECTARLESS
        };
        Self {
            entity_data,
            ticks_in_hive: 0,
            min_ticks_in_hive,
        }
    }

    fn to_nbt(&self) -> NbtTag {
        let mut nbt = NbtCompound::new();
        nbt.put_compound("entity_data", self.entity_data.clone());
        nbt.put_int("ticks_in_hive", self.ticks_in_hive);
        nbt.put_int("min_ticks_in_hive", self.min_ticks_in_hive);
        NbtTag::Compound(nbt)
    }

    fn has_nectar(&self) -> bool {
        self.entity_data.get_bool("HasNectar").unwrap_or(false)
    }

    /// Vanilla `Occupant.createEntity`, minus placing it: the bee as it leaves the hive.
    fn create_entity(&self, world: &Arc<World>, hive_pos: BlockPos) -> Option<Arc<dyn EntityBase>> {
        let mut nbt = self.entity_data.clone();
        for tag in IGNORED_BEE_TAGS {
            nbt.child_tags.remove(tag);
        }
        let entity = load_entity_from_nbt(&nbt, hive_pos.to_centered_f64(), world)?;
        if !entity
            .get_entity()
            .entity_type
            .has_tag(&tag::EntityType::MINECRAFT_BEEHIVE_INHABITORS)
        {
            return None;
        }
        entity.get_entity().set_has_no_gravity(true);
        if let Some(bee) = entity
            .get_mob()
            .and_then(|mob| mob.cast_any().downcast_ref::<BeeEntity>())
        {
            bee.set_hive_pos(hive_pos);
            self.set_bee_release_data(bee);
        }
        Some(entity)
    }

    /// Vanilla `Occupant.setBeeReleaseData`: time spent inside ages the bee and cools its love.
    fn set_bee_release_data(&self, bee: &BeeEntity) {
        let ticks_in_hive = self.ticks_in_hive;
        if !bee.ageable_data.age_locked.load(Ordering::Relaxed) {
            let age = bee.get_age();
            if age < 0 {
                bee.set_age(0.min(age.saturating_add(ticks_in_hive)));
            } else if age > 0 {
                bee.set_age(0.max(age.saturating_sub(ticks_in_hive)));
            }
        }
        let love_ticks = &bee.mob_entity.love_ticks;
        love_ticks.store(
            0.max(love_ticks.load(Ordering::Relaxed) - ticks_in_hive),
            Ordering::Relaxed,
        );
    }
}

/// Vanilla `BeehiveBlockEntity.BeeData`: an occupant and how long it has been inside.
struct BeeData {
    occupant: Occupant,
    ticks_in_hive: i32,
}

impl BeeData {
    const fn new(occupant: Occupant) -> Self {
        Self {
            ticks_in_hive: occupant.ticks_in_hive,
            occupant,
        }
    }

    /// Counts one tick inside and reports whether the bee has stayed long enough.
    const fn tick(&mut self) -> bool {
        let done = self.ticks_in_hive > self.occupant.min_ticks_in_hive;
        self.ticks_in_hive += 1;
        done
    }

    fn to_occupant(&self) -> Occupant {
        Occupant {
            entity_data: self.occupant.entity_data.clone(),
            ticks_in_hive: self.ticks_in_hive,
            min_ticks_in_hive: self.occupant.min_ticks_in_hive,
        }
    }
}

pub struct BeehiveBlockEntity {
    pub position: BlockPos,
    stored: Mutex<Vec<BeeData>>,
    pub flower_pos: Mutex<Option<BlockPos>>,
}

impl BlockEntity for BeehiveBlockEntity {
    fn resource_location(&self) -> &'static str {
        Self::ID
    }

    fn get_position(&self) -> BlockPos {
        self.position
    }

    fn from_nbt(nbt: &pumpkin_nbt::compound::NbtCompound, position: BlockPos) -> Self
    where
        Self: Sized,
    {
        let bees = nbt
            .get_list("bees")
            .or_else(|| nbt.get_list("Bees"))
            .map(Self::occupants_from_nbt)
            .unwrap_or_default();
        let flower_pos = nbt
            .get_int_array("flower_pos")
            .and_then(|arr| (arr.len() == 3).then(|| BlockPos::new(arr[0], arr[1], arr[2])))
            .or_else(|| {
                nbt.get_compound("flower_pos")
                    .or_else(|| nbt.get_compound("FlowerPos"))
                    .map(|c| {
                        BlockPos::new(
                            c.get_int("X").or_else(|| c.get_int("x")).unwrap_or(0),
                            c.get_int("Y").or_else(|| c.get_int("y")).unwrap_or(0),
                            c.get_int("Z").or_else(|| c.get_int("z")).unwrap_or(0),
                        )
                    })
            });
        let hive = Self::new(position);
        *hive.lock_stored() = bees.into_iter().map(BeeData::new).collect();
        *hive
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = flower_pos;
        hive
    }

    fn write_nbt(&self, nbt: &mut NbtCompound) {
        nbt.put_list("bees", self.bees_nbt());
        self.write_flower_pos(nbt);
    }

    fn chunk_data_nbt(&self) -> Option<NbtCompound> {
        let mut nbt = NbtCompound::new();
        nbt.put_list("bees", self.bees_nbt());
        self.write_flower_pos(&mut nbt);
        Some(nbt)
    }

    fn on_player_destroy(&self, world: &Arc<World>, position: &BlockPos, player: &Arc<Player>) {
        let silk_touch = player
            .inventory()
            .held_item()
            .get_enchantment_level(&Enchantment::SILK_TOUCH)
            > 0;
        if silk_touch {
            return;
        }
        let state_id = world.get_block_state_id(position);
        self.empty_all_living_from_hive(world, Some(player), state_id, BeeReleaseStatus::Emergency);
        anger_nearby_bees(world, position);
    }

    fn tick(&self, world: &Arc<World>) {
        let block = world.get_block(&self.position);
        if block != &Block::BEEHIVE && block != &Block::BEE_NEST {
            return;
        }
        self.tick_occupants(world);
        if !self.is_empty() && rng().random::<f64>() < 0.005 {
            world.play_sound(
                Sound::BlockBeehiveWork,
                SoundCategory::Blocks,
                &Vector3::new(
                    f64::from(self.position.0.x) + 0.5,
                    f64::from(self.position.0.y),
                    f64::from(self.position.0.z) + 0.5,
                ),
            );
        }
    }

    fn collect_item_components(&self, stack: &mut pumpkin_data::item_stack::ItemStack) {
        let bees = self.bees_nbt();
        if !bees.is_empty() {
            stack.set_data_component(pumpkin_data::data_component_impl::BeesImpl { bees });
        }
    }

    fn apply_item_components(&self, stack: &pumpkin_data::item_stack::ItemStack) {
        if let Some(bees) =
            stack.get_data_component::<pumpkin_data::data_component_impl::BeesImpl>()
            && !bees.bees.is_empty()
        {
            *self.lock_stored() = Self::occupants_from_nbt(&bees.bees)
                .into_iter()
                .map(BeeData::new)
                .collect();
        }
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

impl BeehiveBlockEntity {
    pub const ID: &'static str = "minecraft:beehive";
    #[must_use]
    pub const fn new(position: BlockPos) -> Self {
        Self {
            position,
            stored: Mutex::new(Vec::new()),
            flower_pos: Mutex::new(None),
        }
    }

    fn lock_stored(&self) -> std::sync::MutexGuard<'_, Vec<BeeData>> {
        self.stored
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    fn occupants_from_nbt(list: &[NbtTag]) -> Vec<Occupant> {
        list.iter()
            .filter_map(|tag| Occupant::from_nbt(tag.extract_compound()?))
            .collect()
    }

    fn bees_nbt(&self) -> Vec<NbtTag> {
        self.lock_stored()
            .iter()
            .map(|data| data.to_occupant().to_nbt())
            .collect()
    }

    fn write_flower_pos(&self, nbt: &mut NbtCompound) {
        if let Some(pos) = *self
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
        {
            nbt.put(
                "flower_pos",
                NbtTag::IntArray(vec![pos.0.x, pos.0.y, pos.0.z]),
            );
        }
    }

    /// Vanilla `getOccupantCount`.
    #[must_use]
    pub fn get_occupant_count(&self) -> usize {
        self.lock_stored().len()
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.lock_stored().is_empty()
    }

    #[must_use]
    pub fn is_full(&self) -> bool {
        self.lock_stored().len() == MAX_OCCUPANTS
    }

    /// Vanilla `BeehiveBlockEntity.isFireNearby`.
    #[must_use]
    pub fn is_fire_nearby(&self, world: &World) -> bool {
        (-1..=1).any(|dx| {
            (-1..=1).any(|dy| {
                (-1..=1).any(|dz| {
                    let pos = self.position.offset(Vector3::new(dx, dy, dz));
                    world.get_block(&pos).id == Block::FIRE.id
                })
            })
        })
    }

    /// Vanilla `BeehiveBlockEntity.addOccupant`: the bee goes inside and leaves the world.
    pub fn add_occupant(&self, world: &Arc<World>, bee: &BeeEntity) {
        let mut stored = self.lock_stored();
        if stored.len() >= MAX_OCCUPANTS {
            return;
        }
        bee.get_entity().remove_leash();
        stored.push(BeeData::new(Occupant::of(bee)));
        drop(stored);
        if let Some(bee_flower_pos) = bee.saved_flower_pos() {
            let mut flower_pos = self
                .flower_pos
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if flower_pos.is_none() || rng().random::<bool>() {
                *flower_pos = Some(bee_flower_pos);
            }
        }
        world.play_sound(
            Sound::BlockBeehiveEnter,
            SoundCategory::Blocks,
            &self.position.to_centered_f64(),
        );
        world.emit_game_event(
            GameEvent::BlockChange.name(),
            self.position.to_centered_f64(),
        );
        bee.get_entity().remove();
        self.set_changed(world);
    }

    /// Vanilla `BeehiveBlockEntity.tickOccupants`.
    fn tick_occupants(&self, world: &Arc<World>) {
        let state_id = world.get_block_state_id(&self.position);
        let saved_flower_pos = *self
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        // The lock is not held while releasing: placing the bee and raising the honey level
        // update blocks, which can read this block entity again.
        let due: Vec<(usize, Occupant)> = self
            .lock_stored()
            .iter_mut()
            .enumerate()
            .filter_map(|(index, data)| data.tick().then(|| (index, data.to_occupant())))
            .collect();
        let released: Vec<usize> = due
            .iter()
            .filter(|(_, occupant)| {
                let release_status = if occupant.has_nectar() {
                    BeeReleaseStatus::HoneyDelivered
                } else {
                    BeeReleaseStatus::BeeReleased
                };
                release_occupant(
                    world,
                    self.position,
                    state_id,
                    occupant,
                    release_status,
                    saved_flower_pos,
                )
                .is_some()
            })
            .map(|(index, _)| *index)
            .collect();
        self.remove_released(world, &released);
    }

    fn remove_released(&self, world: &Arc<World>, released: &[usize]) {
        if released.is_empty() {
            return;
        }
        let mut index = 0;
        self.lock_stored().retain(|_| {
            let keep = !released.contains(&index);
            index += 1;
            keep
        });
        self.set_changed(world);
    }

    /// Vanilla `isSedated`: smoke from a campfire keeps the bees calm.
    #[must_use]
    pub fn is_sedated(&self, world: &World) -> bool {
        is_smokey_pos(world, &self.position)
    }

    /// Vanilla `releaseAllOccupants`.
    fn release_all_occupants(
        &self,
        world: &Arc<World>,
        state_id: pumpkin_data::BlockStateId,
        release_status: BeeReleaseStatus,
    ) -> Vec<Arc<dyn EntityBase>> {
        let saved_flower_pos = *self
            .flower_pos
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let occupants: Vec<Occupant> = self
            .lock_stored()
            .iter()
            .map(BeeData::to_occupant)
            .collect();
        let mut spawned = Vec::new();
        let mut released = Vec::new();
        for (index, occupant) in occupants.iter().enumerate() {
            if let Some(entity) = release_occupant(
                world,
                self.position,
                state_id,
                occupant,
                release_status,
                saved_flower_pos,
            ) {
                spawned.push(entity);
                released.push(index);
            }
        }
        self.remove_released(world, &released);
        spawned
    }

    /// Vanilla `emptyAllLivingFromHive`: the bees come out, and the ones near `player` go for them
    /// unless smoke keeps them calm.
    pub fn empty_all_living_from_hive(
        &self,
        world: &Arc<World>,
        player: Option<&Arc<Player>>,
        state_id: pumpkin_data::BlockStateId,
        release_status: BeeReleaseStatus,
    ) {
        let released = self.release_all_occupants(world, state_id, release_status);
        let Some(player) = player else {
            return;
        };
        let player_pos = player.get_entity().pos.load();
        for entity in released {
            let Some(mob) = entity.get_mob() else {
                continue;
            };
            let Some(bee) = mob.cast_any().downcast_ref::<BeeEntity>() else {
                continue;
            };
            if player_pos.squared_distance_to_vec(&entity.get_entity().pos.load()) > 16.0 {
                continue;
            }
            if self.is_sedated(world) {
                bee.set_stay_out_of_hive_countdown(STAY_OUT_OF_HIVE_WHEN_SEDATED);
            } else {
                let target: Arc<dyn EntityBase> = player.clone();
                mob.set_mob_target(Some(target), TargetReason::ClosestPlayer);
            }
        }
    }

    /// Vanilla `setChanged`: fire next to the hive drives every bee out.
    fn set_changed(&self, world: &Arc<World>) {
        if self.is_fire_nearby(world) {
            let state_id = world.get_block_state_id(&self.position);
            self.empty_all_living_from_hive(world, None, state_id, BeeReleaseStatus::Emergency);
        }
        self.mark_chunk_dirty(world);
    }

    fn mark_chunk_dirty(&self, world: &Arc<World>) {
        let _ = world
            .level
            .read_chunk_sync(&self.position.chunk_position(), |chunk| {
                chunk.mark_dirty(true);
            });
    }
}

/// Vanilla `BeehiveBlockEntity.releaseOccupant`: lets one bee out of the hive's front, if the
/// weather and the front block allow it.
fn release_occupant(
    world: &Arc<World>,
    hive_pos: BlockPos,
    state_id: pumpkin_data::BlockStateId,
    occupant: &Occupant,
    release_status: BeeReleaseStatus,
    saved_flower_pos: Option<BlockPos>,
) -> Option<Arc<dyn EntityBase>> {
    if world.bees_stay_in_hive(&hive_pos) && release_status != BeeReleaseStatus::Emergency {
        return None;
    }

    let facing = BeeNestLikeProperties::from_state_id(state_id)
        .facing
        .to_block_direction()
        .to_offset();
    let facing_pos = hive_pos.offset(facing);
    let front_blocked = !world
        .get_block_state(&facing_pos)
        .collision_shapes
        .is_empty();
    if front_blocked && release_status != BeeReleaseStatus::Emergency {
        return None;
    }

    let entity = occupant.create_entity(world, hive_pos)?;
    if let Some(bee) = entity
        .get_mob()
        .and_then(|mob| mob.cast_any().downcast_ref::<BeeEntity>())
    {
        let mut random = rng();
        if let Some(flower_pos) = saved_flower_pos
            && !bee.has_saved_flower_pos()
            && random.random::<f32>() < 0.9
        {
            bee.set_saved_flower_pos(flower_pos);
        }

        if release_status == BeeReleaseStatus::HoneyDelivered {
            bee.drop_off_nectar();
            raise_honey_level(world, hive_pos, state_id, &mut random);
        }

        let dimensions = bee.get_entity().entity_dimension.load();
        let delta = if front_blocked {
            0.0
        } else {
            0.55 + f64::from(dimensions.width) / 2.0
        };
        bee.get_entity().set_pos(Vector3::new(
            f64::from(hive_pos.0.x) + 0.5 + delta * f64::from(facing.x),
            f64::from(hive_pos.0.y) + 0.5 - f64::from(dimensions.height) / 2.0,
            f64::from(hive_pos.0.z) + 0.5 + delta * f64::from(facing.z),
        ));
    }

    world.play_sound(
        Sound::BlockBeehiveExit,
        SoundCategory::Blocks,
        &hive_pos.to_centered_f64(),
    );
    world.emit_game_event(GameEvent::BlockChange.name(), hive_pos.to_centered_f64());
    world.spawn_entity(entity.clone()).then_some(entity)
}

/// Vanilla `BeehiveBlock.angerNearbyBees`: calm bees around the hive pick one of the nearby
/// players as their target.
pub fn anger_nearby_bees(world: &Arc<World>, pos: &BlockPos) {
    let area = BoundingBox::from_block(pos).expand(8.0, 6.0, 8.0);
    let bees: Vec<_> = world
        .get_entities_at_box(&area)
        .into_iter()
        .filter(|entity| {
            entity
                .get_mob()
                .is_some_and(|mob| mob.cast_any().is::<BeeEntity>())
        })
        .collect();
    if bees.is_empty() {
        return;
    }
    let players = world.get_players_at_box(&area);
    if players.is_empty() {
        return;
    }
    let mut random = rng();
    for bee in bees {
        let Some(mob) = bee.get_mob() else {
            continue;
        };
        if mob.get_mob_entity().get_target().is_none() {
            let target: Arc<dyn EntityBase> =
                players[random.random_range(0..players.len())].clone();
            mob.set_mob_target(Some(target), TargetReason::ClosestPlayer);
        }
    }
}

/// The honey a bee brings in: one level, or two once in a hundred times, up to the maximum.
fn raise_honey_level(
    world: &Arc<World>,
    hive_pos: BlockPos,
    state_id: pumpkin_data::BlockStateId,
    random: &mut impl rand::Rng,
) {
    let mut props = BeeNestLikeProperties::from_state_id(state_id);
    if props.honey_level >= MAX_HONEY_LEVEL {
        return;
    }
    let mut increase: u8 = if random.random_range(0..100) == 0 {
        2
    } else {
        1
    };
    if props.honey_level + increase > MAX_HONEY_LEVEL {
        increase -= 1;
    }
    props.honey_level += increase;
    let block = Block::from_state_id(state_id);
    world.set_block_state(&hive_pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;

    const DAY: i64 = 6000;
    const MIDNIGHT: i64 = 18000;

    fn bee_with_nectar(ticks_in_hive: i32) -> Occupant {
        let mut entity_data = NbtCompound::new();
        entity_data.put_string("id", "minecraft:bee".to_string());
        entity_data.put_bool("HasNectar", true);
        Occupant {
            entity_data,
            ticks_in_hive,
            min_ticks_in_hive: MIN_OCCUPATION_TICKS_NECTAR,
        }
    }

    /// A loaded chunk with an empty beehive at (8, 64, 8) holding `occupant`.
    fn hive_with(
        world: &Arc<World>,
        occupant: Occupant,
        time_of_day: i64,
    ) -> (BeehiveBlockEntity, BlockPos) {
        world.level.loaded_chunks.insert(
            pumpkin_util::math::vector2::Vector2::new(0, 0),
            pumpkin_world::chunk::ChunkData::empty_sync(0, 0),
        );
        world.set_time_of_day(time_of_day);
        let pos = BlockPos::new(8, 64, 8);
        world.set_block_state(
            &pos,
            Block::BEEHIVE.default_state.id,
            BlockFlags::FORCE_STATE | BlockFlags::SKIP_BLOCK_ADDED_CALLBACK,
        );
        let hive = BeehiveBlockEntity::new(pos);
        *hive.lock_stored() = vec![BeeData::new(occupant)];
        (hive, pos)
    }

    #[tokio::test]
    async fn bee_leaves_after_its_time_and_delivers_honey() -> Result<(), Box<dyn std::error::Error>>
    {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        // Released on the first tick that sees `ticks_in_hive > min_ticks_in_hive`.
        let (hive, pos) = hive_with(
            &world,
            bee_with_nectar(MIN_OCCUPATION_TICKS_NECTAR + 1),
            DAY,
        );

        hive.tick(&world);

        assert!(hive.is_empty());
        let entities = world.entities.load();
        assert_eq!(entities.len(), 1);
        let bee = entities[0]
            .get_mob()
            .and_then(|mob| mob.cast_any().downcast_ref::<BeeEntity>())
            .ok_or("released entity is not a bee")?;
        assert!(!bee.has_nectar(), "the bee drops its nectar off");
        // Spawned in front of the hive (north, since the default state faces north).
        let spawned = bee.get_entity().pos.load();
        assert!(spawned.z < f64::from(pos.0.z), "{spawned:?}");
        assert!((spawned.x - 8.5).abs() < 1e-9, "{spawned:?}");
        assert_eq!(
            BeeNestLikeProperties::from_state_id(world.get_block_state_id(&pos)).honey_level,
            1
        );
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn bee_going_back_in_is_stored_as_it_was_and_leaves_the_world()
    -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let (hive, _) = hive_with(
            &world,
            bee_with_nectar(MIN_OCCUPATION_TICKS_NECTAR + 1),
            DAY,
        );
        hive.tick(&world);
        let entity = world
            .entities
            .load()
            .first()
            .cloned()
            .ok_or("no bee was released")?;
        let bee = entity
            .get_mob()
            .and_then(|mob| mob.cast_any().downcast_ref::<BeeEntity>())
            .ok_or("released entity is not a bee")?;
        bee.set_has_nectar(true);
        bee.set_saved_flower_pos(BlockPos::new(3, 64, 3));

        hive.add_occupant(&world, bee);

        assert_eq!(hive.get_occupant_count(), 1);
        assert!(bee.get_entity().is_removed());
        let (min_ticks_in_hive, has_nectar) = {
            let stored = hive.lock_stored();
            (
                stored[0].occupant.min_ticks_in_hive,
                stored[0].occupant.has_nectar(),
            )
        };
        assert_eq!(min_ticks_in_hive, MIN_OCCUPATION_TICKS_NECTAR);
        assert!(has_nectar);
        assert_eq!(
            *hive.flower_pos.lock().map_err(|_| "poisoned")?,
            Some(BlockPos::new(3, 64, 3))
        );
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn an_emergency_empties_the_hive_even_at_night() -> Result<(), Box<dyn std::error::Error>>
    {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let (hive, pos) = hive_with(&world, bee_with_nectar(0), MIDNIGHT);
        let state_id = world.get_block_state_id(&pos);

        hive.empty_all_living_from_hive(&world, None, state_id, BeeReleaseStatus::Emergency);

        assert!(hive.is_empty());
        assert_eq!(world.entities.load().len(), 1);
        world.level.shutdown().await;
        Ok(())
    }

    #[tokio::test]
    async fn bees_stay_inside_at_night() -> Result<(), Box<dyn std::error::Error>> {
        let folder = tempfile::tempdir()?;
        let world = world(&folder);
        let (hive, _) = hive_with(
            &world,
            bee_with_nectar(MIN_OCCUPATION_TICKS_NECTAR + 1),
            MIDNIGHT,
        );

        hive.tick(&world);

        assert!(!hive.is_empty());
        assert!(world.entities.load().is_empty());
        world.level.shutdown().await;
        Ok(())
    }

    #[test]
    fn occupants_read_the_vanilla_and_the_older_format() {
        let mut vanilla = NbtCompound::new();
        let mut entity_data = NbtCompound::new();
        entity_data.put_string("id", "minecraft:bee".to_string());
        vanilla.put_compound("entity_data", entity_data.clone());
        vanilla.put_int("ticks_in_hive", 44);
        vanilla.put_int("min_ticks_in_hive", 600);
        let occupant = Occupant::from_nbt(&vanilla).expect("vanilla format");
        assert_eq!(
            (occupant.ticks_in_hive, occupant.min_ticks_in_hive),
            (44, 600)
        );
        assert!(occupant.to_nbt().extract_compound().is_some_and(|nbt| {
            nbt.get_int("ticks_in_hive") == Some(44) && nbt.get_compound("entity_data").is_some()
        }));

        let mut older = NbtCompound::new();
        older.put_compound("EntityData", entity_data);
        older.put_int("TicksInHive", 7);
        older.put_int("MinOccupationTicks", 2400);
        let occupant = Occupant::from_nbt(&older).expect("older format");
        assert_eq!(
            (occupant.ticks_in_hive, occupant.min_ticks_in_hive),
            (7, 2400)
        );
    }
}
