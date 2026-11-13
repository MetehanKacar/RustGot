use pumpkin_data::damage::DamageType;

use crate::entity::effect::MobEffect;
use crate::entity::living::LivingEntity;

pub struct PoisonMobEffect;

impl MobEffect for PoisonMobEffect {
    fn should_apply_effect_tick(&self, duration: i32, amplifier: u8) -> bool {
        if duration <= 0 {
            return false;
        }
        let tick_rate = 25 >> amplifier.min(4);
        if tick_rate > 0 {
            (duration as u32).is_multiple_of(tick_rate as u32)
        } else {
            true
        }
    }

    fn apply_effect_tick(&self, living: &LivingEntity, _amplifier: u8) {
        let world = living.entity.world.load();
        let mob_effects = &world.purpur().gameplay_mechanics.mob_effects;
        if living.health.load() > mob_effects.minimal_health_poison_amount
            && let Some(dyn_self) = world.get_entity_by_id(living.entity.entity_id)
        {
            dyn_self.damage(
                &*dyn_self,
                mob_effects.poison_degeneration_amount,
                DamageType::MAGIC,
            );
        }
    }
}
