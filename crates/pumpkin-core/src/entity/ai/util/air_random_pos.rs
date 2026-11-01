use pumpkin_util::math::vector3::Vector3;

use super::air_and_water_random_pos;
use crate::entity::mob::Mob;

/// Vanilla `AirRandomPos.getPosTowards`: a spot in open air, leaning towards `towards_pos`.
#[must_use]
pub fn get_pos_towards(
    mob: &dyn Mob,
    horizontal_dist: i32,
    vertical_dist: i32,
    flying_height: i32,
    towards_pos: Vector3<f64>,
    max_xz_radians_from_center: f64,
) -> Option<Vector3<f64>> {
    let dir = towards_pos - mob.get_entity().pos.load();
    air_and_water_random_pos::get_pos_in_air_or_water(
        mob,
        horizontal_dist,
        vertical_dist,
        flying_height,
        dir.x,
        dir.z,
        max_xz_radians_from_center,
        true,
    )
}
