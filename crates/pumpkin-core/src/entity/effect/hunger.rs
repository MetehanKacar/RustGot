use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct HungerMobEffect;

impl MobEffect for HungerMobEffect {
    fn should_apply_effect_tick(&self, duration: i32, _amplifier: u8) -> bool {
        duration > 0
    }

    fn apply_effect_tick(&self, living: &LivingEntity, amplifier: u8) {
        let world = living.entity.world.load();
        if let Some(entity) = world.get_entity_by_id(living.entity.entity_id)
            && let Some(player) = entity.get_player()
        {
            let amount = world
                .purpur()
                .gameplay_mechanics
                .mob_effects
                .hunger_exhaustion_amount;
            player
                .hunger_manager
                .add_exhaustion(amount * (f32::from(amplifier) + 1.0));
        }
    }
}
