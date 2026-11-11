use std::sync::Arc;

use crate::block::blocks::campfire::is_smokey_pos;
use crate::block::entities::beehive::{BeeReleaseStatus, BeehiveBlockEntity, anger_nearby_bees};
use crate::block::registry::BlockActionResult;
use crate::block::{
    BlockBehaviour, BlockMetadata, ExplodeArgs, GetComparatorOutputArgs, OnPlaceArgs,
    UseWithItemArgs,
};
use crate::entity::EntityBase;
use crate::entity::player::Player;
use crate::world::World;
use pumpkin_data::block_properties::BeeNestLikeProperties;
use pumpkin_data::game_event::GameEvent;
use pumpkin_data::item::Item;
use pumpkin_data::item_stack::ItemStack;
use pumpkin_data::sound::{Sound, SoundCategory};
use pumpkin_data::{Block, BlockId, BlockStateId};
use pumpkin_util::math::position::BlockPos;
use pumpkin_world::world::BlockFlags;

/// A hive holds this much honey before it can be harvested.
pub const MAX_HONEY_LEVEL: u8 = 5;

/// Vanilla `BeehiveBlock.useItemOn` once honey was taken: smoke keeps the bees inside,
/// otherwise they come out angry. The honey level drops to zero either way.
pub fn harvest_hive(
    world: &Arc<World>,
    block: &Block,
    state_id: BlockStateId,
    pos: &BlockPos,
    player: &Player,
) {
    let mut props = BeeNestLikeProperties::from_state_id(state_id);
    props.honey_level = 0;
    let hive = world.get_block_entity(pos);
    let hive = hive
        .as_deref()
        .and_then(|entity| entity.as_any().downcast_ref::<BeehiveBlockEntity>());
    let sedated = is_smokey_pos(world, pos);
    if !sedated && hive.is_some_and(|hive| !hive.is_empty()) {
        anger_nearby_bees(world, pos);
    }
    world.set_block_state(pos, props.to_state_id(block), BlockFlags::NOTIFY_ALL);
    if !sedated && let Some(hive) = hive {
        let player = world.get_player_by_uuid(player.gameprofile.id);
        hive.empty_all_living_from_hive(
            world,
            player.as_ref(),
            state_id,
            BeeReleaseStatus::Emergency,
        );
    }
}

pub struct BeehiveBlock;

impl BlockMetadata for BeehiveBlock {
    fn ids() -> Box<[BlockId]> {
        [BlockId::BEEHIVE, BlockId::BEE_NEST].into()
    }
}

impl BlockBehaviour for BeehiveBlock {
    fn on_place(&self, args: OnPlaceArgs<'_>) -> BlockStateId {
        let mut props = BeeNestLikeProperties::default(args.block);
        props.facing = args.player.get_entity().get_horizontal_facing().opposite();
        props.to_state_id(args.block)
    }

    fn use_with_item(&self, args: UseWithItemArgs<'_>) -> BlockActionResult {
        if args.item_stack.item != &Item::GLASS_BOTTLE {
            return BlockActionResult::Pass;
        }
        let state_id = args.world.get_block_state_id(args.position);
        if BeeNestLikeProperties::from_state_id(state_id).honey_level < MAX_HONEY_LEVEL {
            return BlockActionResult::Pass;
        }
        let player = args.player;
        let position = player.position();
        args.world
            .play_sound(Sound::ItemBottleFill, SoundCategory::Blocks, &position);
        let mut honey_bottle = ItemStack::new(1, &Item::HONEY_BOTTLE);
        if args.item_stack.item_count == 1
            && player.gamemode.load() != pumpkin_util::GameMode::Creative
        {
            *args.item_stack = honey_bottle;
        } else {
            args.item_stack
                .decrement_unless_creative(player.gamemode.load(), 1);
            let was_added = player.inventory().insert_stack_anywhere(&mut honey_bottle);
            if !was_added && !honey_bottle.is_empty() {
                args.world
                    .drop_stack(&position.to_block_pos(), honey_bottle);
            }
        }
        args.world.emit_game_event(
            GameEvent::FluidPickup.name(),
            args.position.to_centered_f64(),
        );
        harvest_hive(args.world, args.block, state_id, args.position, player);
        BlockActionResult::Success
    }

    /// Vanilla `BeehiveBlock.onExplosionHit`.
    fn explode(&self, args: ExplodeArgs<'_>) {
        anger_nearby_bees(args.world, args.position);
    }

    fn get_comparator_output(&self, args: GetComparatorOutputArgs<'_>) -> Option<u8> {
        let state_id = args.world.get_block_state_id(args.position);
        let props = BeeNestLikeProperties::from_state_id(state_id);
        Some(props.honey_level)
    }
}
