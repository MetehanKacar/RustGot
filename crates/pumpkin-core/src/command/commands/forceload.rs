use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::coordinates::Coordinates;
use crate::command::argument_types::coordinates::column_pos::ColumnPosArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::world::World;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

const DESCRIPTION: &str = "Constantly load chunks in the world.";
const PERMISSION: &str = "minecraft:command.forceload";

static ERROR_FAILED_ADD: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_ADDED_FAILURE,
    "No world in source",
);
static ERROR_FAILED_REMOVE: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_REMOVED_FAILURE,
    "No world in source",
);
static ERROR_ALL_ADDED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_ADDED_FAILURE,
    "No chunks were marked for force loading",
);
static ERROR_NONE_REMOVED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_REMOVED_FAILURE,
    "No chunks were removed from force loading",
);
static ERROR_FAILED_QUERY: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_QUERY_FAILURE,
    "No world in source",
);
static ERROR_NOT_TICKING: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_QUERY_FAILURE,
    "Chunk is not marked for force loading",
);
static ERROR_TOO_MANY: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FORCELOAD_TOOBIG,
    "Too many chunks",
);

/// Returns how many chunks changed and the first changed chunk.
fn change_force_load(
    world: &World,
    min_x: i32,
    max_x: i32,
    min_z: i32,
    max_z: i32,
    forced: bool,
) -> (i32, Vector2<i32>) {
    let mut changed = 0;
    let mut first_changed = Vector2::new(min_x, min_z);
    for x in min_x..=max_x {
        for z in min_z..=max_z {
            let pos = Vector2::new(x, z);
            if world.set_chunk_forced(pos, forced) {
                if changed == 0 {
                    first_changed = pos;
                }
                changed += 1;
            }
        }
    }
    world.update_active_chunks();
    (changed, first_changed)
}

struct ForceloadAddExecutor;

impl CommandExecutor for ForceloadAddExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let from_pos = ColumnPosArgumentType::get_column_pos(context, "from")?;

        let to_pos = if context.get_argument::<Coordinates>("to").is_ok() {
            ColumnPosArgumentType::get_column_pos(context, "to")?
        } else {
            from_pos
        };

        let world = context
            .source
            .world
            .as_ref()
            .ok_or_else(|| ERROR_FAILED_ADD.create_without_context())?;

        let chunk_x_start = from_pos.0.x >> 4;
        let chunk_z_start = from_pos.0.y >> 4;

        let chunk_x_end = to_pos.0.x >> 4;
        let chunk_z_end = to_pos.0.y >> 4;

        let min_x = chunk_x_start.min(chunk_x_end);
        let max_x = chunk_x_start.max(chunk_x_end);
        let min_z = chunk_z_start.min(chunk_z_end);
        let max_z = chunk_z_start.max(chunk_z_end);

        let count_x = max_x - min_x + 1;
        let count_z = max_z - min_z + 1;
        let total_chunks = count_x * count_z;

        if total_chunks > 256 {
            return Err(ERROR_TOO_MANY.create_without_context(
                TextComponent::text("256"),
                TextComponent::text(total_chunks.to_string()),
            ));
        }

        let (changed, first_changed) = change_force_load(world, min_x, max_x, min_z, max_z, true);
        if changed == 0 {
            return Err(ERROR_ALL_ADDED.create_without_context());
        }

        let dimension_name = world.dimension.minecraft_name.to_string();

        let text = if changed == 1 {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_ADDED_SINGLE,
                translation::java::COMMANDS_FORCELOAD_ADDED_SINGLE,
                [
                    TextComponent::text(format!("[{}, {}]", first_changed.x, first_changed.y)),
                    TextComponent::text(dimension_name),
                ],
            )
        } else {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_ADDED_MULTIPLE,
                translation::java::COMMANDS_FORCELOAD_ADDED_MULTIPLE,
                [
                    TextComponent::text(changed.to_string()),
                    TextComponent::text(dimension_name),
                    TextComponent::text(format!("[{min_x}, {min_z}]")),
                    TextComponent::text(format!("[{max_x}, {max_z}]")),
                ],
            )
        };
        context.source.send_feedback(text, true);

        Ok(changed)
    }
}

struct ForceloadRemoveExecutor;

impl CommandExecutor for ForceloadRemoveExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let from_pos = ColumnPosArgumentType::get_column_pos(context, "from")?;

        let to_pos = if context.get_argument::<Coordinates>("to").is_ok() {
            ColumnPosArgumentType::get_column_pos(context, "to")?
        } else {
            from_pos
        };

        let world = context
            .source
            .world
            .as_ref()
            .ok_or_else(|| ERROR_FAILED_REMOVE.create_without_context())?;

        let chunk_x_start = from_pos.0.x >> 4;
        let chunk_z_start = from_pos.0.y >> 4;

        let chunk_x_end = to_pos.0.x >> 4;
        let chunk_z_end = to_pos.0.y >> 4;

        let min_x = chunk_x_start.min(chunk_x_end);
        let max_x = chunk_x_start.max(chunk_x_end);
        let min_z = chunk_z_start.min(chunk_z_end);
        let max_z = chunk_z_start.max(chunk_z_end);

        let count_x = max_x - min_x + 1;
        let count_z = max_z - min_z + 1;
        let total_chunks = count_x * count_z;

        if total_chunks > 256 {
            return Err(ERROR_TOO_MANY.create_without_context(
                TextComponent::text("256"),
                TextComponent::text(total_chunks.to_string()),
            ));
        }

        let (changed, first_changed) = change_force_load(world, min_x, max_x, min_z, max_z, false);
        if changed == 0 {
            return Err(ERROR_NONE_REMOVED.create_without_context());
        }

        let dimension_name = world.dimension.minecraft_name.to_string();

        let text = if changed == 1 {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_REMOVED_SINGLE,
                translation::java::COMMANDS_FORCELOAD_REMOVED_SINGLE,
                [
                    TextComponent::text(format!("[{}, {}]", first_changed.x, first_changed.y)),
                    TextComponent::text(dimension_name),
                ],
            )
        } else {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_REMOVED_MULTIPLE,
                translation::java::COMMANDS_FORCELOAD_REMOVED_MULTIPLE,
                [
                    TextComponent::text(changed.to_string()),
                    TextComponent::text(dimension_name),
                    TextComponent::text(format!("[{min_x}, {min_z}]")),
                    TextComponent::text(format!("[{max_x}, {max_z}]")),
                ],
            )
        };
        context.source.send_feedback(text, true);

        Ok(changed)
    }
}

struct ForceloadRemoveAllExecutor;

impl CommandExecutor for ForceloadRemoveAllExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context
            .source
            .world
            .as_ref()
            .ok_or_else(|| ERROR_FAILED_REMOVE.create_without_context())?;

        let forced: Vec<_> = world
            .forced_chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        for pos in forced {
            world.set_chunk_forced(pos, false);
        }
        world.update_active_chunks();

        let dimension_name = world.dimension.minecraft_name.to_string();

        let text = TextComponent::translate_cross(
            translation::java::COMMANDS_FORCELOAD_REMOVED_ALL,
            translation::java::COMMANDS_FORCELOAD_REMOVED_ALL,
            [TextComponent::text(dimension_name)],
        );
        context.source.send_feedback(text, true);

        Ok(0)
    }
}

struct ForceloadQueryExecutor;

impl CommandExecutor for ForceloadQueryExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let world = context
            .source
            .world
            .as_ref()
            .ok_or_else(|| ERROR_FAILED_QUERY.create_without_context())?;

        let dimension_name = world.dimension.minecraft_name.to_string();

        if context.get_argument::<Coordinates>("pos").is_ok() {
            let pos = ColumnPosArgumentType::get_column_pos(context, "pos")?;
            let chunk_pos = Vector2::new(pos.0.x >> 4, pos.0.y >> 4);
            let is_forced = world
                .forced_chunks
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .contains(&chunk_pos);
            if !is_forced {
                return Err(ERROR_NOT_TICKING.create_without_context(
                    TextComponent::text(format!("[{}, {}]", chunk_pos.x, chunk_pos.y)),
                    TextComponent::text(dimension_name),
                ));
            }
            let text = TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_QUERY_SUCCESS,
                translation::java::COMMANDS_FORCELOAD_QUERY_SUCCESS,
                [
                    TextComponent::text(format!("[{}, {}]", chunk_pos.x, chunk_pos.y)),
                    TextComponent::text(dimension_name),
                ],
            );
            context.source.send_feedback(text, false);
            return Ok(1);
        }

        let mut all_forced: Vec<Vector2<i32>> = world
            .forced_chunks
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .iter()
            .copied()
            .collect();
        // Vanilla sorts by the packed `ChunkPos` long.
        all_forced.sort_unstable_by_key(|pos| {
            (i64::from(pos.x) & 0xFFFF_FFFF) | ((i64::from(pos.y) & 0xFFFF_FFFF) << 32)
        });

        if all_forced.is_empty() {
            let text = TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_ADDED_NONE,
                translation::java::COMMANDS_FORCELOAD_ADDED_NONE,
                [TextComponent::text(dimension_name)],
            );
            context.source.send_error(text);
            return Ok(0);
        }

        let list_str = all_forced
            .iter()
            .map(|pos| format!("[{}, {}]", pos.x, pos.y))
            .collect::<Vec<_>>()
            .join(", ");
        let text = if all_forced.len() == 1 {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_LIST_SINGLE,
                translation::java::COMMANDS_FORCELOAD_LIST_SINGLE,
                [
                    TextComponent::text(dimension_name),
                    TextComponent::text(list_str),
                ],
            )
        } else {
            TextComponent::translate_cross(
                translation::java::COMMANDS_FORCELOAD_LIST_MULTIPLE,
                translation::java::COMMANDS_FORCELOAD_LIST_MULTIPLE,
                [
                    TextComponent::text(all_forced.len().to_string()),
                    TextComponent::text(dimension_name),
                    TextComponent::text(list_str),
                ],
            )
        };
        context.source.send_feedback(text, false);

        Ok(all_forced.len() as i32)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let builder = command("forceload", DESCRIPTION)
        .requires(PERMISSION)
        .then(
            literal("add").then(
                argument("from", ColumnPosArgumentType)
                    .executes(ForceloadAddExecutor)
                    .then(argument("to", ColumnPosArgumentType).executes(ForceloadAddExecutor)),
            ),
        )
        .then(
            literal("remove")
                .then(literal("all").executes(ForceloadRemoveAllExecutor))
                .then(
                    argument("from", ColumnPosArgumentType)
                        .executes(ForceloadRemoveExecutor)
                        .then(
                            argument("to", ColumnPosArgumentType).executes(ForceloadRemoveExecutor),
                        ),
                ),
        )
        .then(
            literal("query")
                .executes(ForceloadQueryExecutor)
                .then(argument("pos", ColumnPosArgumentType).executes(ForceloadQueryExecutor)),
        );

    dispatcher.register(builder);
}
