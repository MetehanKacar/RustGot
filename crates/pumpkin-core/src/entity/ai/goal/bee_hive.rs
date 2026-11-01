use std::sync::Arc;

use pumpkin_data::tag::{self, Taggable};
use pumpkin_util::math::position::BlockPos;
use rand::RngExt;

use super::{Controls, Goal};
use crate::block::entities::BlockEntity;
use crate::block::entities::beehive::BeehiveBlockEntity;
use crate::entity::EntityBase;
use crate::entity::ai::pathfinder::path::Path;
use crate::entity::mob::{Mob, neutral::NeutralMob};
use crate::entity::passive::bee::{BeeEntity, LOCATE_HIVE_COOLDOWN};
use crate::world::World;

const MAX_TRAVELLING_TICKS: i32 = 2400;
const TICKS_BEFORE_HIVE_DROP: i32 = 60;
const LOCATE_HIVE_RADIUS: i32 = 20;

fn bee_of(mob: &dyn Mob) -> Option<&BeeEntity> {
    mob.cast_any().downcast_ref::<BeeEntity>()
}

/// Vanilla `Bee.BaseBeeGoal`: bee goals never run while the bee is angry.
fn calm_bee(mob: &dyn Mob) -> Option<&BeeEntity> {
    bee_of(mob).filter(|bee| !NeutralMob::is_angry(*bee))
}

fn as_hive(block_entity: &dyn BlockEntity) -> Option<&BeehiveBlockEntity> {
    block_entity.as_any().downcast_ref::<BeehiveBlockEntity>()
}

/// Vanilla `Bee.BeeEnterHiveGoal`.
#[derive(Default)]
pub struct BeeEnterHiveGoal;

impl Goal for BeeEnterHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(bee) = calm_bee(mob) else {
            return false;
        };
        let Some(hive_pos) = bee.hive_pos.load() else {
            return false;
        };
        let pos = bee.get_entity().pos.load();
        if !bee.wants_to_enter_hive() || hive_pos.dist_to_center_sqr(pos.x, pos.y, pos.z) >= 4.0 {
            return false;
        }
        let Some(hive) = bee.get_beehive_block_entity() else {
            return false;
        };
        if as_hive(hive.as_ref()).is_some_and(|hive| !hive.is_full()) {
            return true;
        }
        bee.hive_pos.store(None);
        false
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if let Some(hive) = bee.get_beehive_block_entity().as_deref().and_then(as_hive) {
            hive.add_occupant(&bee.get_entity().world.load_full(), bee);
        }
    }
}

/// Vanilla `Bee.BeeGoToHiveGoal`.
#[derive(Default)]
pub struct BeeGoToHiveGoal {
    travelling_ticks: i32,
    last_path: Option<Path>,
    ticks_stuck: i32,
}

impl BeeGoToHiveGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        let Some(hive_pos) = bee.hive_pos.load() else {
            return false;
        };
        !bee.is_too_far_away(&hive_pos)
            && !bee.get_mob_entity().has_position_target()
            && bee.wants_to_enter_hive()
            && !Self::has_reached_target(bee, &hive_pos)
            && bee
                .get_entity()
                .world
                .load()
                .get_block(&hive_pos)
                .has_tag(&tag::Block::MINECRAFT_BEEHIVES)
    }

    fn has_reached_target(bee: &BeeEntity, target: &BlockPos) -> bool {
        if bee.closer_than(target, 2) {
            return true;
        }
        let steered = bee.get_mob_entity().steered();
        let navigator = steered.navigator();
        navigator
            .get_path()
            .is_some_and(|path| path.get_target() == *target && path.can_reach() && path.is_done())
    }

    /// Vanilla `moveTo(x, y, z, closeEnough, speed)`, which plans the path before following it.
    fn pathfind_directly_towards(bee: &BeeEntity, target: &BlockPos) -> bool {
        let close_enough = if bee.closer_than(target, 3) { 1 } else { 2 };
        let steered = bee.get_mob_entity().steered();
        let living = &steered.mob_entity().living_entity;
        let mut navigator = steered.navigator();
        navigator.set_max_visited_nodes_multiplier(10.0);
        let path = navigator.create_path(living, target.to_f64(), close_enough);
        let can_reach = path.as_ref().is_some_and(Path::can_reach);
        navigator.move_to_path(path, 1.0, living);
        can_reach
    }

    fn drop_and_blacklist_hive(bee: &BeeEntity) {
        if let Some(hive_pos) = bee.hive_pos.load() {
            bee.blacklist_hive(hive_pos);
        }
        bee.drop_hive();
    }
}

impl Goal for BeeGoToHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.travelling_ticks = 0;
        self.ticks_stuck = 0;
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        navigator.stop();
        navigator.reset_max_visited_nodes_multiplier();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(hive_pos) = bee.hive_pos.load() else {
            return;
        };
        self.travelling_ticks += 1;
        if self.travelling_ticks > MAX_TRAVELLING_TICKS {
            Self::drop_and_blacklist_hive(bee);
            return;
        }
        let in_progress = mob.get_mob_entity().steered().navigator().is_in_progress();
        if in_progress {
            return;
        }
        if !bee.closer_than(&hive_pos, 16) {
            if bee.is_too_far_away(&hive_pos) {
                bee.drop_hive();
            } else {
                bee.pathfind_randomly_towards(hive_pos);
            }
            return;
        }
        if !Self::pathfind_directly_towards(bee, &hive_pos) {
            Self::drop_and_blacklist_hive(bee);
            return;
        }
        let steered = mob.get_mob_entity().steered();
        let navigator = steered.navigator();
        let path = navigator.get_path();
        let same_path = self
            .last_path
            .as_ref()
            .is_some_and(|last| path.is_some_and(|path| path.same_as_path(last)));
        if same_path {
            self.ticks_stuck += 1;
            if self.ticks_stuck > TICKS_BEFORE_HIVE_DROP {
                drop(navigator);
                bee.drop_hive();
                self.ticks_stuck = 0;
            }
        } else {
            self.last_path = path.map(Path::copy);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.BeeLocateHiveGoal`.
#[derive(Default)]
pub struct BeeLocateHiveGoal;

impl BeeLocateHiveGoal {
    /// Vanilla `PoiManager.getInRange` over the bee homes, nearest first and with room left.
    fn find_nearby_hives_with_space(world: &World, center: BlockPos) -> Vec<BlockPos> {
        let radius_sqr = LOCATE_HIVE_RADIUS * LOCATE_HIVE_RADIUS;
        let min_chunk_x = (center.0.x - LOCATE_HIVE_RADIUS) >> 4;
        let max_chunk_x = (center.0.x + LOCATE_HIVE_RADIUS) >> 4;
        let min_chunk_z = (center.0.z - LOCATE_HIVE_RADIUS) >> 4;
        let max_chunk_z = (center.0.z + LOCATE_HIVE_RADIUS) >> 4;
        let mut hives = Vec::new();
        for chunk_x in min_chunk_x..=max_chunk_x {
            for chunk_z in min_chunk_z..=max_chunk_z {
                let Some(block_entities) = world
                    .block_entities
                    .get(&pumpkin_util::math::vector2::Vector2::new(chunk_x, chunk_z))
                else {
                    continue;
                };
                hives.extend(block_entities.iter().filter_map(|(pos, block_entity)| {
                    let hive = as_hive(block_entity.as_ref())?;
                    (pos.squared_distance(&center) <= radius_sqr && !hive.is_full()).then_some(*pos)
                }));
            }
        }
        hives.sort_by_key(|pos| pos.squared_distance(&center));
        hives
    }
}

impl Goal for BeeLocateHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(|bee| {
            bee.remaining_cooldown_before_locating_new_hive() == 0
                && bee.hive_pos.load().is_none()
                && bee.wants_to_enter_hive()
        })
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        bee.set_remaining_cooldown_before_locating_new_hive(LOCATE_HIVE_COOLDOWN);
        let world: Arc<World> = bee.get_entity().world.load_full();
        let hives = Self::find_nearby_hives_with_space(&world, bee.get_entity().block_pos.load());
        let Some(first) = hives.first() else {
            return;
        };
        if let Some(pos) = hives.iter().find(|pos| !bee.is_hive_blacklisted(pos)) {
            bee.hive_pos.store(Some(*pos));
            return;
        }
        bee.clear_hive_blacklist();
        bee.hive_pos.store(Some(*first));
    }
}

/// Vanilla `Bee.ValidateHiveGoal`.
pub struct ValidateHiveGoal {
    validate_cooldown: i64,
    last_validate_tick: i64,
}

impl ValidateHiveGoal {
    #[must_use]
    pub fn new(mob: &dyn Mob) -> Self {
        Self {
            validate_cooldown: mob.get_random().random_range(20..=40),
            last_validate_tick: -1,
        }
    }
}

impl Goal for ValidateHiveGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some()
            && mob.get_entity().world.load().get_world_age()
                > self.last_validate_tick + self.validate_cooldown
    }

    fn should_continue(&mut self, _mob: &dyn Mob) -> bool {
        false
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let world = bee.get_entity().world.load_full();
        if bee.hive_pos.load().is_some_and(|pos| world.is_loaded(&pos)) && !bee.is_hive_valid() {
            bee.drop_hive();
        }
        self.last_validate_tick = world.get_world_age();
    }
}
