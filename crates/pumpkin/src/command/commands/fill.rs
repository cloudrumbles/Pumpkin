use pumpkin_data::Block;
use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::math::position::BlockPos;
use pumpkin_util::math::vector2::Vector2;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use crate::command::argument_builder::{
    ArgumentBuilder, RequiredArgumentBuilder, argument, command, literal,
};
use crate::command::argument_types::block::{BlockStateArgument, BlockStateArgumentType};
use crate::command::argument_types::block_predicate::{BlockPredicate, BlockPredicateArgumentType};
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::block_input::{matches_block, notify_neighbors, place_block, placement_flags};
use crate::command::context::command_context::CommandContext;
use crate::command::context::command_source::CommandSource;
use crate::command::errors::command_syntax_error::CommandSyntaxError;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};

const DESCRIPTION: &str = "Fills all or parts of a region with a specific block.";
const PERMISSION: &str = "minecraft:command.fill";

const ERROR_AREA_TOO_LARGE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::COMMANDS_FILL_TOOBIG,
    translation::java::COMMANDS_FILL_TOOBIG,
);

const ERROR_NOT_LOADED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_POS_UNLOADED,
    translation::java::ARGUMENT_POS_UNLOADED,
);

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_FILL_FAILED,
    translation::java::COMMANDS_FILL_FAILED,
);

#[derive(Clone, Copy, PartialEq, Eq)]
enum FillMode {
    Replace,
    Outline,
    Hollow,
    Destroy,
    Keep,
}

#[derive(Clone, Copy)]
enum FilterMode {
    None,
    WithFilter,
    KeepAir,
}

fn fill_blocks(
    source: &CommandSource,
    from: BlockPos,
    to: BlockPos,
    input: &BlockStateArgument,
    mode: FillMode,
    filter: Option<&BlockPredicate>,
    strict: bool,
) -> Result<i32, CommandSyntaxError> {
    let min_x = from.0.x.min(to.0.x);
    let min_y = from.0.y.min(to.0.y);
    let min_z = from.0.z.min(to.0.z);
    let max_x = from.0.x.max(to.0.x);
    let max_y = from.0.y.max(to.0.y);
    let max_z = from.0.z.max(to.0.z);
    let area = (i128::from(max_x) - i128::from(min_x) + 1)
        * (i128::from(max_y) - i128::from(min_y) + 1)
        * (i128::from(max_z) - i128::from(min_z) + 1);
    let world = source.world();
    let limit = world
        .level_info
        .load()
        .game_rules
        .max_block_modifications
        .min(i64::from(i32::MAX));
    if area > i128::from(limit) {
        return Err(ERROR_AREA_TOO_LARGE.create_without_context(
            TextComponent::text(limit.to_string()),
            TextComponent::text(area.to_string()),
        ));
    }
    // Check the entire region before mutating any chunk.
    for chunk_x in (min_x >> 4)..=(max_x >> 4) {
        for chunk_z in (min_z >> 4)..=(max_z >> 4) {
            if world
                .level
                .read_chunk_sync(&Vector2::new(chunk_x, chunk_z), |_| ())
                .is_none()
            {
                return Err(ERROR_NOT_LOADED.create_without_context());
            }
        }
    }
    let air = BlockStateArgument {
        block: &Block::AIR,
        state: Block::AIR.default_state,
        properties: std::collections::HashMap::new(),
        nbt: None,
    };
    let flags = placement_flags(strict);
    let mut count = 0;
    let mut changed_positions = Vec::new();
    for z in min_z..=max_z {
        for y in min_y..=max_y {
            for x in min_x..=max_x {
                let pos = BlockPos::new(x, y, z);
                let previous = world.get_block_state_id(&pos);
                if (mode == FillMode::Keep && !previous.to_state().is_air())
                    || filter.is_some_and(|predicate| !matches_block(world, &pos, predicate))
                {
                    continue;
                }
                let is_edge = x == min_x
                    || x == max_x
                    || y == min_y
                    || y == max_y
                    || z == min_z
                    || z == max_z;
                if mode == FillMode::Outline && !is_edge {
                    continue;
                }
                let target = if mode == FillMode::Hollow && !is_edge {
                    &air
                } else {
                    input
                };
                let destroyed = mode == FillMode::Destroy
                    && world
                        .break_block(&pos, None, BlockFlags::NOTIFY_ALL)
                        .is_some();
                let placed = place_block(world, &pos, target, flags);
                if placed {
                    if !strict {
                        changed_positions.push((pos, previous));
                    }
                    count += 1;
                } else if destroyed {
                    count += 1;
                }
            }
        }
    }
    for (position, previous) in changed_positions {
        notify_neighbors(world, &position, previous);
    }
    if count == 0 {
        return Err(ERROR_FAILED.create_without_context());
    }
    world.flush_block_updates();
    source.send_feedback(
        TextComponent::translate_cross(
            translation::java::COMMANDS_FILL_SUCCESS,
            translation::java::COMMANDS_FILL_SUCCESS,
            [TextComponent::text(count.to_string())],
        ),
        true,
    );
    Ok(count)
}

struct FillExecutor {
    mode: FillMode,
    filter_mode: FilterMode,
    strict: bool,
}

impl CommandExecutor for FillExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let from = BlockPosArgumentType::get_loaded_block_pos(context, "from")?;
        let to = BlockPosArgumentType::get_loaded_block_pos(context, "to")?;
        let block_state = BlockStateArgumentType::get(context, "block")?;

        let filter = if matches!(self.filter_mode, FilterMode::WithFilter) {
            Some(BlockPredicateArgumentType::get(context, "filter")?)
        } else {
            None
        };

        fill_blocks(
            &context.source,
            from,
            to,
            &block_state,
            self.mode,
            filter.as_ref(),
            self.strict,
        )
    }
}

fn wrap_with_mode(builder: RequiredArgumentBuilder, has_filter: bool) -> RequiredArgumentBuilder {
    let filter_mode = if has_filter {
        FilterMode::WithFilter
    } else {
        FilterMode::None
    };

    builder
        .executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode,
            strict: false,
        })
        .then(literal("outline").executes(FillExecutor {
            mode: FillMode::Outline,
            filter_mode,
            strict: false,
        }))
        .then(literal("hollow").executes(FillExecutor {
            mode: FillMode::Hollow,
            filter_mode,
            strict: false,
        }))
        .then(literal("destroy").executes(FillExecutor {
            mode: FillMode::Destroy,
            filter_mode,
            strict: false,
        }))
        .then(literal("strict").executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode,
            strict: true,
        }))
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    let filter_arg = wrap_with_mode(argument("filter", BlockPredicateArgumentType), true);

    let replace_literal = literal("replace")
        .executes(FillExecutor {
            mode: FillMode::Replace,
            filter_mode: FilterMode::None,
            strict: false,
        })
        .then(filter_arg);

    let keep_literal = literal("keep").executes(FillExecutor {
        mode: FillMode::Keep,
        filter_mode: FilterMode::KeepAir,
        strict: false,
    });

    let block_arg = wrap_with_mode(argument("block", BlockStateArgumentType), false)
        .then(replace_literal)
        .then(keep_literal);

    dispatcher.register(
        command("fill", DESCRIPTION).requires(PERMISSION).then(
            argument("from", BlockPosArgumentType)
                .then(argument("to", BlockPosArgumentType).then(block_arg)),
        ),
    );
}
