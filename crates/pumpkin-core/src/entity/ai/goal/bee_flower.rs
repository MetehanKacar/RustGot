use std::collections::HashMap;

use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::world::WorldEvent;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector3::Vector3;
use rand::RngExt;

use super::{Controls, Goal};
use crate::block::blocks::plant::crop::bee_growth_state;
use crate::entity::EntityBase;
use crate::entity::ai::util::{air_and_water_random_pos, hover_random_pos};
use crate::entity::mob::{Mob, neutral::NeutralMob};
use crate::entity::passive::bee::BeeEntity;
use pumpkin_world::world::BlockFlags;

const MAX_TRAVELLING_TICKS: i32 = 2400;
const TICKS_BEFORE_KNOWN_FLOWER_TRIP: i32 = 600;
const MAX_CROPS_GROWN_SINCE_POLLINATION: i32 = 10;
const GROW_CHANCE: i32 = 30;
const FLOWER_SEARCH_RADIUS: i32 = 5;
const MIN_POLLINATION_TICKS: i32 = 400;
const MAX_POLLINATING_TICKS: i32 = 600;
const POSITION_CHANGE_CHANCE: i32 = 25;
const POLLINATE_SPEED_MODIFIER: f64 = 0.35;
const HOVER_HEIGHT_WITHIN_FLOWER: f64 = 0.6;
const HOVER_POS_OFFSET: f32 = 0.333_333_34;
const ARRIVAL_THRESHOLD: f64 = 0.1;
const UNREACHABLE_FLOWER_TICKS: i64 = 600;

fn bee_of(mob: &dyn Mob) -> Option<&BeeEntity> {
    mob.cast_any().downcast_ref::<BeeEntity>()
}

/// Vanilla `Bee.BaseBeeGoal`: bee goals never run while the bee is angry.
fn calm_bee(mob: &dyn Mob) -> Option<&BeeEntity> {
    bee_of(mob).filter(|bee| !NeutralMob::is_angry(*bee))
}

fn distance(a: Vector3<f64>, b: Vector3<f64>) -> f64 {
    a.squared_distance_to_vec(&b).sqrt()
}

/// Vanilla `Vec3.atBottomCenterOf`.
fn at_bottom_center_of(pos: &BlockPos) -> Vector3<f64> {
    Vector3::new(
        f64::from(pos.0.x) + 0.5,
        f64::from(pos.0.y),
        f64::from(pos.0.z) + 0.5,
    )
}

/// Vanilla `Bee.BeeGoToKnownFlowerGoal`.
#[derive(Default)]
pub struct BeeGoToKnownFlowerGoal {
    travelling_ticks: i32,
}

impl BeeGoToKnownFlowerGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        bee.saved_flower_pos().is_some_and(|flower| {
            !bee.get_mob_entity().has_position_target()
                && bee
                    .ticks_without_nectar
                    .load(std::sync::atomic::Ordering::Relaxed)
                    > TICKS_BEFORE_KNOWN_FLOWER_TRIP
                && !bee.closer_than(&flower, 2)
        })
    }
}

impl Goal for BeeGoToKnownFlowerGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn start(&mut self, _mob: &dyn Mob) {
        self.travelling_ticks = 0;
    }

    fn stop(&mut self, mob: &dyn Mob) {
        self.travelling_ticks = 0;
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        navigator.stop();
        navigator.reset_max_visited_nodes_multiplier();
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(flower) = bee.saved_flower_pos() else {
            return;
        };
        self.travelling_ticks += 1;
        if self.travelling_ticks > MAX_TRAVELLING_TICKS {
            bee.drop_flower();
            return;
        }
        if mob.get_mob_entity().steered().navigator().is_in_progress() {
            return;
        }
        if bee.is_too_far_away(&flower) {
            bee.drop_flower();
        } else {
            bee.pathfind_randomly_towards(flower);
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.BeeGrowCropGoal`.
#[derive(Default)]
pub struct BeeGrowCropGoal;

impl BeeGrowCropGoal {
    fn can_bee_use(bee: &BeeEntity) -> bool {
        if bee
            .crops_grown_since_pollination
            .load(std::sync::atomic::Ordering::Relaxed)
            >= MAX_CROPS_GROWN_SINCE_POLLINATION
        {
            return false;
        }
        if bee.get_random().random::<f32>() < 0.3 {
            return false;
        }
        bee.has_nectar() && bee.is_hive_valid()
    }
}

impl Goal for BeeGrowCropGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        calm_bee(mob).is_some_and(Self::can_bee_use)
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if bee.get_random().random_range(0..GROW_CHANCE) != 0 {
            return;
        }
        let world = bee.get_entity().world.load_full();
        let bee_pos = bee.get_entity().block_pos.load();
        for i in 1..=2 {
            let below = BlockPos::new(bee_pos.0.x, bee_pos.0.y - i, bee_pos.0.z);
            let (block, state_id) = world.get_block_and_state_id(&below);
            let Some(grown) = bee_growth_state(block, state_id) else {
                continue;
            };
            world.sync_world_event(WorldEvent::ParticlesAndSoundPlantGrowth, below, 15);
            world.set_block_state(&below, grown, BlockFlags::NOTIFY_ALL);
            bee.crops_grown_since_pollination
                .fetch_add(1, std::sync::atomic::Ordering::Relaxed);
        }
    }
}

/// Vanilla `Bee.BeePollinateGoal`.
#[derive(Default)]
pub struct BeePollinateGoal {
    successful_pollinating_ticks: i32,
    last_sound_played_tick: i32,
    hover_pos: Option<Vector3<f64>>,
    pollinating_ticks: i32,
    unreachable_flower_cache: HashMap<BlockPos, i64>,
}

impl BeePollinateGoal {
    const fn has_pollinated_long_enough(&self) -> bool {
        self.successful_pollinating_ticks > MIN_POLLINATION_TICKS
    }

    fn set_wanted_pos(&self, mob: &dyn Mob) {
        if let Some(hover) = self.hover_pos {
            mob.get_mob_entity()
                .steered()
                .move_control()
                .set_wanted_position(hover.x, hover.y, hover.z, POLLINATE_SPEED_MODIFIER);
        }
    }

    fn offset(mob: &dyn Mob) -> f64 {
        f64::from((mob.get_random().random::<f32>() * 2.0 - 1.0) * HOVER_POS_OFFSET)
    }

    /// Vanilla `findNearbyFlower`: the closest flower the bee can path to, remembering the ones
    /// it could not reach for a while.
    fn find_nearby_flower(&mut self, bee: &BeeEntity) -> Option<BlockPos> {
        let world = bee.get_entity().world.load_full();
        let now = world.get_world_age();
        let center = bee.get_entity().block_pos.load();
        let mut candidates = Vec::new();
        for dx in -FLOWER_SEARCH_RADIUS..=FLOWER_SEARCH_RADIUS {
            for dy in -FLOWER_SEARCH_RADIUS..=FLOWER_SEARCH_RADIUS {
                for dz in -FLOWER_SEARCH_RADIUS..=FLOWER_SEARCH_RADIUS {
                    let manhattan = dx.abs() + dy.abs() + dz.abs();
                    if manhattan <= FLOWER_SEARCH_RADIUS {
                        candidates.push((manhattan, dx, dy, dz));
                    }
                }
            }
        }
        candidates.sort_unstable();
        let mut temp_cache = HashMap::new();
        let mut result = None;
        for (_, dx, dy, dz) in candidates {
            let pos = BlockPos::new(center.0.x + dx, center.0.y + dy, center.0.z + dz);
            let (block, state_id) = world.get_block_and_state_id(&pos);
            if !BeeEntity::attracts_bees(block, state_id) {
                continue;
            }
            if let Some(&until) = self.unreachable_flower_cache.get(&pos)
                && now < until
            {
                temp_cache.insert(pos, until);
                continue;
            }
            let steered = bee.get_mob_entity().steered();
            let path = steered.navigator().create_path(
                &steered.mob_entity().living_entity,
                pos.to_f64(),
                1,
            );
            if path.is_some_and(|path| path.can_reach()) {
                result = Some(pos);
                break;
            }
            temp_cache.insert(pos, now + UNREACHABLE_FLOWER_TICKS);
        }
        self.unreachable_flower_cache = temp_cache;
        result
    }
}

impl Goal for BeePollinateGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        let Some(bee) = calm_bee(mob) else {
            return false;
        };
        if bee.remaining_cooldown_before_locating_new_flower() > 0
            || bee.has_nectar()
            || bee.get_entity().world.load().is_raining()
        {
            return false;
        }
        let Some(flower) = self.find_nearby_flower(bee) else {
            bee.set_remaining_cooldown_before_locating_new_flower(
                bee.get_random().random_range(20..=60),
            );
            return false;
        };
        bee.set_saved_flower_pos(flower);
        let steered = mob.get_mob_entity().steered();
        steered.navigator().move_to_coords(
            f64::from(flower.0.x) + 0.5,
            f64::from(flower.0.y) + 0.5,
            f64::from(flower.0.z) + 0.5,
            1.2,
            &steered.mob_entity().living_entity,
        );
        true
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        let Some(bee) = calm_bee(mob) else {
            return false;
        };
        if !bee.is_pollinating()
            || !bee.has_saved_flower_pos()
            || bee.get_entity().world.load().is_raining()
        {
            return false;
        }
        !self.has_pollinated_long_enough() || bee.get_random().random::<f32>() < 0.2
    }

    fn start(&mut self, mob: &dyn Mob) {
        self.successful_pollinating_ticks = 0;
        self.pollinating_ticks = 0;
        self.last_sound_played_tick = 0;
        if let Some(bee) = bee_of(mob) {
            bee.set_pollinating(true);
            bee.ticks_without_nectar
                .store(0, std::sync::atomic::Ordering::Relaxed);
        }
    }

    fn stop(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        if self.has_pollinated_long_enough() {
            bee.set_has_nectar(true);
        }
        bee.set_pollinating(false);
        mob.get_mob_entity().steered().navigator().stop();
        bee.set_remaining_cooldown_before_locating_new_flower(200);
    }

    fn should_run_every_tick(&self) -> bool {
        true
    }

    fn tick(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(saved_flower) = bee.saved_flower_pos() else {
            return;
        };
        self.pollinating_ticks += 1;
        if self.pollinating_ticks > MAX_POLLINATING_TICKS {
            bee.drop_flower();
            bee.set_pollinating(false);
            bee.set_remaining_cooldown_before_locating_new_flower(200);
            return;
        }
        let mut flower_pos = at_bottom_center_of(&saved_flower);
        flower_pos.y += HOVER_HEIGHT_WITHIN_FLOWER;
        let bee_pos = bee.get_entity().pos.load();
        if distance(flower_pos, bee_pos) > 1.0 {
            self.hover_pos = Some(flower_pos);
            self.set_wanted_pos(mob);
            return;
        }
        let hover_pos = *self.hover_pos.get_or_insert(flower_pos);
        let arrived_at_hover_pos = distance(bee_pos, hover_pos) <= ARRIVAL_THRESHOLD;
        let mut should_set_wanted_pos = true;
        if arrived_at_hover_pos {
            if bee.get_random().random_range(0..POSITION_CHANGE_CHANCE) == 0 {
                self.hover_pos = Some(Vector3::new(
                    flower_pos.x + Self::offset(mob),
                    flower_pos.y,
                    flower_pos.z + Self::offset(mob),
                ));
                mob.get_mob_entity().steered().navigator().stop();
            } else {
                should_set_wanted_pos = false;
            }
            mob.get_mob_entity()
                .look_control
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .look_at(mob, flower_pos.x, flower_pos.y, flower_pos.z);
        }
        if should_set_wanted_pos {
            self.set_wanted_pos(mob);
        }
        self.successful_pollinating_ticks += 1;
        if bee.get_random().random::<f32>() < 0.05
            && self.successful_pollinating_ticks > self.last_sound_played_tick + 60
        {
            self.last_sound_played_tick = self.successful_pollinating_ticks;
            bee.get_entity().world.load().play_sound(
                Sound::EntityBeePollinate,
                SoundCategory::Neutral,
                &bee_pos,
            );
        }
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.BeeWanderGoal`.
#[derive(Default)]
pub struct BeeWanderGoal;

impl BeeWanderGoal {
    fn wander_threshold(bee: &BeeEntity) -> i32 {
        let distance_reduction = if bee.hive_pos.load().is_none() && !bee.has_saved_flower_pos() {
            16
        } else {
            24
        };
        48 - distance_reduction
    }

    fn find_pos(bee: &BeeEntity) -> Option<Vector3<f64>> {
        let wander_direction = match bee.hive_pos.load() {
            Some(hive_pos)
                if bee.is_hive_valid()
                    && !bee.closer_than(&hive_pos, Self::wander_threshold(bee)) =>
            {
                let to_hive = hive_pos.to_centered_f64() - bee.get_entity().pos.load();
                let length = to_hive
                    .squared_distance_to_vec(&Vector3::new(0.0, 0.0, 0.0))
                    .sqrt();
                if length < 1.0e-5 {
                    Vector3::new(0.0, 0.0, 0.0)
                } else {
                    Vector3::new(to_hive.x / length, to_hive.y / length, to_hive.z / length)
                }
            }
            _ => bee.get_looking_vector(),
        };
        hover_random_pos::get_pos(
            bee,
            8,
            7,
            wander_direction.x,
            wander_direction.z,
            std::f64::consts::FRAC_PI_2,
            3,
            1,
        )
        .or_else(|| {
            air_and_water_random_pos::get_pos(
                bee,
                8,
                4,
                -2,
                wander_direction.x,
                wander_direction.z,
                std::f64::consts::FRAC_PI_2,
            )
        })
    }
}

impl Goal for BeeWanderGoal {
    fn can_start(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().steered().navigator().is_done()
            && mob.get_random().random_range(0..10) == 0
    }

    fn should_continue(&mut self, mob: &dyn Mob) -> bool {
        mob.get_mob_entity().steered().navigator().is_in_progress()
    }

    fn start(&mut self, mob: &dyn Mob) {
        let Some(bee) = bee_of(mob) else {
            return;
        };
        let Some(target) = Self::find_pos(bee) else {
            return;
        };
        let steered = mob.get_mob_entity().steered();
        let mut navigator = steered.navigator();
        let living = &steered.mob_entity().living_entity;
        let path = navigator.create_path(living, target, 1);
        navigator.move_to_path(path, 1.0, living);
    }

    fn controls(&self) -> Controls {
        Controls::MOVE
    }
}

/// Vanilla `Bee.ValidateFlowerGoal`.
pub struct ValidateFlowerGoal {
    validate_cooldown: i64,
    last_validate_tick: i64,
}

impl ValidateFlowerGoal {
    #[must_use]
    pub fn new(mob: &dyn Mob) -> Self {
        Self {
            validate_cooldown: mob.get_random().random_range(20..=40),
            last_validate_tick: -1,
        }
    }
}

impl Goal for ValidateFlowerGoal {
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
        if let Some(flower) = bee.saved_flower_pos()
            && world.is_loaded(&flower)
        {
            let (block, state_id) = world.get_block_and_state_id(&flower);
            if !BeeEntity::attracts_bees(block, state_id) {
                bee.drop_flower();
            }
        }
        self.last_validate_tick = world.get_world_age();
    }
}
