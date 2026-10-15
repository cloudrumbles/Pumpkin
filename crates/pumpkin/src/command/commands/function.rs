use pumpkin_data::translation;
use pumpkin_protocol::java::client::play::ArgumentType as JavaClientArgumentType;
use pumpkin_util::PermissionLvl;
use pumpkin_util::identifier::Identifier;
use pumpkin_util::permission::{Permission, PermissionDefault, PermissionRegistry};
use pumpkin_util::text::TextComponent;

use crate::command::argument_builder::{ArgumentBuilder, argument, command};
use crate::command::argument_types::FromStringReader;
use crate::command::argument_types::argument_type::ArgumentType;
use crate::command::context::command_context::CommandContext;
use crate::command::errors::error_types::CommandErrorType;
use crate::command::node::dispatcher::CommandDispatcher;
use crate::command::node::{CommandExecutor, CommandExecutorResult};
use crate::command::string_reader::StringReader;
use crate::command::suggestion::provider::{SuggestionProvider, SuggestionProviderResult};
use crate::command::suggestion::suggestions::SuggestionsBuilder;

const DESCRIPTION: &str = "Runs commands found in the corresponding function files.";
const PERMISSION: &str = "minecraft:command.function";

static ERROR_UNKNOWN_FUNCTION: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
    translation::java::ARGUMENTS_FUNCTION_UNKNOWN,
);

struct FunctionSuggestionProvider;

struct FunctionArgumentType;

impl<S: pumpkin_command::source::CommandSource> ArgumentType<S> for FunctionArgumentType {
    type Item = String;

    fn parse(
        &self,
        reader: &mut StringReader,
    ) -> Result<Self::Item, crate::command::errors::command_syntax_error::CommandSyntaxError> {
        let is_tag = reader.peek() == Some('#');
        if is_tag {
            reader.skip();
        }
        let identifier = Identifier::from_reader(reader)?;
        Ok(if is_tag {
            format!("#{identifier}")
        } else {
            identifier.to_string()
        })
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::Function
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "foo".to_string(),
            "foo:bar/baz".to_string(),
            "#minecraft:load".to_string(),
        ]
    }
}

impl FunctionArgumentType {
    fn get<'a>(
        context: &'a CommandContext,
        name: &str,
    ) -> Result<&'a str, crate::command::errors::command_syntax_error::CommandSyntaxError> {
        Ok(context.get_argument::<String>(name)?.as_str())
    }
}

impl SuggestionProvider for FunctionSuggestionProvider {
    fn suggest(
        &self,
        context: &CommandContext,
        mut builder: SuggestionsBuilder,
    ) -> SuggestionProviderResult {
        let server = context.server();
        let function_names = server.datapack_manager.get_function_names();
        for name in function_names {
            builder = builder.suggest(name);
        }
        builder.build()
    }
}

struct FunctionExecutor;

impl CommandExecutor for FunctionExecutor {
    fn execute(&self, context: &CommandContext) -> CommandExecutorResult {
        let name_str = FunctionArgumentType::get(context, "name")?;
        let server = context.server();

        let Ok(executed_count) =
            server
                .datapack_manager
                .execute_function(server, &context.source, name_str)
        else {
            return Err(ERROR_UNKNOWN_FUNCTION
                .create_without_context(TextComponent::text(name_str.to_string())));
        };

        if name_str.starts_with('#') {
            context.source.send_feedback(
                TextComponent::translate_cross(
                    translation::java::COMMANDS_FUNCTION_SUCCESS_MULTIPLE,
                    translation::java::COMMANDS_FUNCTION_SUCCESS_MULTIPLE,
                    [
                        TextComponent::text(executed_count.to_string()),
                        TextComponent::text(name_str.to_string()),
                    ],
                ),
                true,
            );
        } else {
            context.source.send_feedback(
                TextComponent::translate_cross(
                    translation::java::COMMANDS_FUNCTION_SUCCESS_SINGLE,
                    translation::java::COMMANDS_FUNCTION_SUCCESS_SINGLE,
                    [
                        TextComponent::text(executed_count.to_string()),
                        TextComponent::text(name_str.to_string()),
                    ],
                ),
                true,
            );
        }

        Ok(executed_count as i32)
    }
}

pub fn register(dispatcher: &mut CommandDispatcher, registry: &PermissionRegistry) {
    registry.register_permission_or_panic(Permission::new(
        PERMISSION,
        DESCRIPTION,
        PermissionDefault::Op(PermissionLvl::Two),
    ));

    dispatcher.register(
        command("function", DESCRIPTION).requires(PERMISSION).then(
            argument("name", FunctionArgumentType)
                .suggests(FunctionSuggestionProvider)
                .executes(FunctionExecutor),
        ),
    );
}

#[cfg(test)]
mod tests {
    use crate::command::argument_types::argument_type::ArgumentType;
    use crate::command::source::DummySource;
    use crate::command::string_reader::StringReader;

    use super::FunctionArgumentType;

    #[test]
    fn parses_namespaced_function_id() {
        let input = "ironore:bootstrap/main";
        let mut reader = StringReader::new(input);

        let parsed = ArgumentType::<DummySource>::parse(&FunctionArgumentType, &mut reader)
            .expect("valid namespaced function");

        assert_eq!(parsed, input);
        assert_eq!(reader.cursor(), input.len());
    }

    #[test]
    fn adds_default_namespace_to_function_id() {
        let mut reader = StringReader::new("load/start");

        let parsed = ArgumentType::<DummySource>::parse(&FunctionArgumentType, &mut reader)
            .expect("valid default-namespace function");

        assert_eq!(parsed, "minecraft:load/start");
    }

    #[test]
    fn preserves_function_tag_marker_while_normalizing_id() {
        let mut reader = StringReader::new("#load");

        let parsed = ArgumentType::<DummySource>::parse(&FunctionArgumentType, &mut reader)
            .expect("valid function tag");

        assert_eq!(parsed, "#minecraft:load");
    }
}
