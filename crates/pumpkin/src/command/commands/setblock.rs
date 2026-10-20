use pumpkin_data::translation;
use pumpkin_util::PermissionLvl;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;
use pumpkin_world::world::BlockFlags;

use crate::command::argument_builder::{ArgumentBuilder, argument, command, literal};
use crate::command::argument_types::block::BlockStateArgumentType;
use crate::command::argument_types::coordinates::block_pos::BlockPosArgumentType;
use crate::command::block_input::{notify_neighbors, place_block, placement_flags};
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};

const DESCRIPTION: &str = "Place a block.";
const PERMISSION: &str = "minecraft:command.setblock";

const ERROR_FAILED: CommandErrorType<0> = CommandErrorType::new(
    translation::java::COMMANDS_SETBLOCK_FAILED,
    translation::java::COMMANDS_SETBLOCK_FAILED,
);

#[derive(Clone, Copy)]
enum Mode {
    /// with particles + item drops
    Destroy,

    /// only replaces air
    Keep,

    /// default; without particles
    Replace,

    /// places block without triggering updates around it
    Strict,
}

struct SetBlockExecutor(Mode);

impl CommandExecutor for SetBlockExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let input = BlockStateArgumentType::get(context, "block")?;
        let mode = self.0;
        let world = context.source.world();
        let pos = BlockPosArgumentType::get_loaded_block_pos(context, "pos")?;

        let previous = world.get_block_state_id(&pos);
        if matches!(mode, Mode::Keep) && !previous.to_state().is_air() {
            return Err(ERROR_FAILED.create_without_context());
        }
        let destroyed = matches!(mode, Mode::Destroy)
            && world
                .break_block(&pos, None, BlockFlags::NOTIFY_ALL)
                .is_some();
        let strict = matches!(mode, Mode::Strict);
        let placed = place_block(world, &pos, &input, placement_flags(strict));
        let success = placed || destroyed;
        if success && !strict {
            notify_neighbors(world, &pos, previous);
        }

        if success {
            world.flush_block_updates();
            context.source.send_feedback(
                TextComponent::translate_cross(
                    pumpkin_data::translation::java::COMMANDS_SETBLOCK_SUCCESS,
                    pumpkin_data::translation::bedrock::COMMANDS_SETBLOCK_SUCCESS,
                    [
                        TextComponent::text(pos.0.x.to_string()),
                        TextComponent::text(pos.0.y.to_string()),
                        TextComponent::text(pos.0.z.to_string()),
                    ],
                ),
                true,
            );
            Ok(1)
        } else {
            Err(ERROR_FAILED.create_without_context())
        }
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("setblock", DESCRIPTION).requires(PERMISSION).then(
            argument("pos", BlockPosArgumentType).then(
                argument("block", BlockStateArgumentType)
                    .executes(SetBlockExecutor(Mode::Replace))
                    .then(literal("destroy").executes(SetBlockExecutor(Mode::Destroy)))
                    .then(literal("keep").executes(SetBlockExecutor(Mode::Keep)))
                    .then(literal("replace").executes(SetBlockExecutor(Mode::Replace)))
                    .then(literal("strict").executes(SetBlockExecutor(Mode::Strict))),
            ),
        ),
    );
}
