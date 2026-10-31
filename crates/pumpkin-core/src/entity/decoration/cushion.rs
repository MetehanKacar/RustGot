use std::sync::{
    Arc,
    atomic::{AtomicU8, Ordering},
};

use pumpkin_data::damage::DamageType;
use pumpkin_data::dye_color::DyeColor;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_nbt::{compound::NbtCompound, tag::NbtTag};
use pumpkin_protocol::codec::var_int::VarInt;
use pumpkin_protocol::java::client::play::Metadata;
use pumpkin_util::math::vector3::Vector3;
use pumpkin_util::version::JavaMinecraftVersion;

use crate::entity::player::Player;
use crate::entity::{Entity, EntityBase, living::LivingEntity};
use crate::server::Server;

pub struct CushionEntity {
    pub entity: Entity,
    color: AtomicU8,
}

impl CushionEntity {
    pub const fn new(entity: Entity, color: u8) -> Self {
        Self {
            entity,
            color: AtomicU8::new(color),
        }
    }

    pub fn color(&self) -> u8 {
        self.color.load(Ordering::Relaxed)
    }

    pub fn set_color(&self, color: u8) {
        self.color.store(color, Ordering::Relaxed);
        self.sync_color();
    }

    fn sync_color(&self) {
        self.entity.set_synced_data(
            pumpkin_data::tracked_data::cushion::COLOR,
            VarInt(i32::from(self.color())),
        );
    }

    #[must_use]
    pub const fn item_for_color(color: u8) -> &'static Item {
        match color {
            0 => &Item::WHITE_CUSHION,
            1 => &Item::ORANGE_CUSHION,
            2 => &Item::MAGENTA_CUSHION,
            3 => &Item::LIGHT_BLUE_CUSHION,
            4 => &Item::YELLOW_CUSHION,
            5 => &Item::LIME_CUSHION,
            6 => &Item::PINK_CUSHION,
            7 => &Item::GRAY_CUSHION,
            8 => &Item::LIGHT_GRAY_CUSHION,
            9 => &Item::CYAN_CUSHION,
            10 => &Item::PURPLE_CUSHION,
            11 => &Item::BLUE_CUSHION,
            12 => &Item::BROWN_CUSHION,
            13 => &Item::GREEN_CUSHION,
            14 => &Item::RED_CUSHION,
            _ => &Item::BLACK_CUSHION,
        }
    }

    #[must_use]
    pub fn color_from_item(item: &Item) -> u8 {
        for (id, comp) in item.components {
            if *id == pumpkin_data::data_component::DataComponent::CushionColor {
                return pumpkin_data::data_component_impl::get::<
                    pumpkin_data::data_component_impl::CushionColorImpl,
                >(*comp)
                .color
                .id();
            }
        }
        0
    }

    #[must_use]
    pub fn color_from_stack(stack: &ItemStack) -> u8 {
        stack
            .get_data_component::<pumpkin_data::data_component_impl::CushionColorImpl>()
            .map_or_else(|| Self::color_from_item(stack.get_item()), |c| c.color.id())
    }

    fn drop_and_remove(&self) {
        let entity = &self.entity;
        let world = entity.world.load();
        world.play_sound(
            Sound::EntityCushionBreak,
            SoundCategory::Blocks,
            &entity.pos.load(),
        );
        world.drop_stack(
            &entity.block_pos.load(),
            ItemStack::new(1, Self::item_for_color(self.color())),
        );
        entity.remove();
    }
}

impl EntityBase for CushionEntity {
    fn get_entity(&self) -> &Entity {
        &self.entity
    }

    fn get_living_entity(&self) -> Option<&LivingEntity> {
        None
    }

    fn init_data_tracker(&self) {
        self.sync_color();
    }

    /// The colour has to ride along with the spawn packet; a client seeing the
    /// cushion for the first time never got the tracked data update.
    fn java_spawn_metadata(&self, version: JavaMinecraftVersion) -> Option<Box<[u8]>> {
        let mut metadata = Vec::new();
        Metadata::new(
            pumpkin_data::tracked_data::cushion::COLOR,
            VarInt(i32::from(self.color())),
        )
        .write(&mut metadata, &version)
        .ok()?;
        metadata.push(255);
        Some(metadata.into_boxed_slice())
    }

    fn tick(&self, _caller: &dyn EntityBase, _server: &Server) {}

    fn can_hit(&self) -> bool {
        self.entity.is_alive()
    }

    fn is_collidable(&self, _entity: Option<Box<dyn EntityBase>>) -> bool {
        true
    }

    // Vanilla `Cushion` (a `BlockAttachedEntity`): `color` is the dye name and `block_pos`
    // the block it occupies. Without `block_pos`, vanilla logs the cushion as being at an
    // invalid position; reading only `Color` dropped the colour of cushions from structures.
    fn write_custom_nbt(&self, nbt: &mut NbtCompound) {
        let color = DyeColor::by_id(self.color()).unwrap_or(DyeColor::White);
        nbt.put_string("color", color.name().to_string());
        let block_pos = self.entity.block_pos.load().0;
        nbt.put(
            "block_pos",
            NbtTag::IntArray(vec![block_pos.x, block_pos.y, block_pos.z]),
        );
    }

    fn read_custom_nbt(&self, nbt: &NbtCompound) {
        let color = nbt
            .get_string("color")
            .and_then(DyeColor::by_name)
            .map(|color| color.id())
            // Worlds saved by older RustGot builds.
            .or_else(|| nbt.get_byte("Color").map(|color| color as u8));
        if let Some(color) = color {
            self.set_color(color);
        }
    }

    fn interact(&self, player: &Arc<Player>, _item_stack: &mut ItemStack) -> bool {
        if player.get_entity().is_sneaking() {
            return false;
        }
        if player.get_entity().has_vehicle() {
            return false;
        }
        if !self
            .entity
            .passengers
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .is_empty()
        {
            return false;
        }

        let world = self.entity.world.load();
        let Some(vehicle) = world.get_entity_by_id(self.entity.entity_id) else {
            return false;
        };
        let Some(passenger) = world.get_player_by_id(player.entity_id()) else {
            return false;
        };

        world.play_sound(
            Sound::EntityCushionSit,
            SoundCategory::Blocks,
            &self.entity.pos.load(),
        );
        self.entity
            .add_passenger(vehicle, passenger as Arc<dyn EntityBase>);
        true
    }

    fn damage_with_context(
        &self,
        _caller: &dyn EntityBase,
        _amount: f32,
        _damage_type: DamageType,
        _position: Option<Vector3<f64>>,
        _source: Option<&dyn EntityBase>,
        _cause: Option<&dyn EntityBase>,
    ) -> bool {
        if !self.entity.is_alive() {
            return false;
        }
        self.drop_and_remove();
        true
    }

    fn cast_any(&self) -> &dyn std::any::Any {
        self
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::entity::ai::test_support::world;
    use pumpkin_data::entity::EntityType;

    #[tokio::test]
    async fn cushion_nbt_matches_vanilla() {
        let folder = tempfile::tempdir().unwrap();
        let world = world(&folder);
        let cushion = CushionEntity::new(
            Entity::new(
                world.clone(),
                Vector3::new(-140.5, 63.9375, -337.5),
                &EntityType::CUSHION,
            ),
            DyeColor::White.id(),
        );

        // As written by vanilla / structure templates.
        let mut vanilla = NbtCompound::new();
        vanilla.put_string("color", "magenta".to_string());
        cushion.read_custom_nbt(&vanilla);
        assert_eq!(cushion.color(), DyeColor::Magenta.id());

        let mut saved = NbtCompound::new();
        cushion.write_custom_nbt(&mut saved);
        assert_eq!(saved.get_string("color"), Some("magenta"));
        assert_eq!(saved.get_int_array("block_pos"), Some(&[-141, 63, -338][..]));
        world.level.shutdown().await;
    }
}
