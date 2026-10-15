use crate::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::command_syntax_error::CommandSyntaxError,
    errors::error_types::CommandErrorType,
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};
use pumpkin_data::{Block, translation};
use pumpkin_util::text::TextComponent;

pub const INVALID_BLOCK_ERROR_TYPE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockArgumentType {
    type Item = &'static Block;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let start = reader.cursor();
        while let Some(c) = reader.peek() {
            if c.is_alphanumeric() || c == '_' || c == ':' || c == '/' || c == '.' || c == '-' {
                reader.skip();
            } else {
                break;
            }
        }
        let block_name = &reader.string()[start..reader.cursor()];
        let normalized = if block_name.contains(':') {
            block_name.to_string()
        } else {
            format!("minecraft:{block_name}")
        };

        Block::from_name(&normalized)
            .ok_or_else(|| INVALID_BLOCK_ERROR_TYPE.create(reader, TextComponent::text(normalized)))
    }

    fn client_side_parser(&'_ self) -> JavaClientArgumentType {
        JavaClientArgumentType::BlockState
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder.build()
    }
}

impl BlockArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<&'static Block, CommandSyntaxError> {
        context.get_argument::<&'static Block>(name).copied()
    }
}

#[cfg(test)]
mod tests {
    use crate::argument_types::argument_type::ArgumentType;
    use crate::argument_types::block::{
        BlockStateArgumentType, INVALID_BLOCK_PROPERTY_ERROR_TYPE,
        UNKNOWN_BLOCK_PROPERTY_ERROR_TYPE,
    };
    use crate::source::DummySource;
    use crate::string_reader::StringReader;

    #[test]
    fn parses_block_state_properties_over_default_state() {
        let input = "oak_stairs[facing=east,half=top]";
        let mut reader = StringReader::new(input);

        let parsed = ArgumentType::<DummySource>::parse(&BlockStateArgumentType, &mut reader)
            .expect("valid block state");

        assert_eq!(reader.cursor(), input.len());
        assert_eq!(parsed.block.name, "oak_stairs");
        let properties = parsed
            .block
            .properties(parsed.state.id)
            .expect("oak stairs properties")
            .to_props();
        assert!(properties.contains(&("facing", "east")));
        assert!(properties.contains(&("half", "top")));
        assert!(properties.contains(&("waterlogged", "false")));
    }

    #[test]
    fn rejects_unknown_block_state_property() {
        let mut reader = StringReader::new("stone[facing=east]");

        let error = ArgumentType::<DummySource>::parse(&BlockStateArgumentType, &mut reader)
            .expect_err("stone has no facing property");

        assert_eq!(error.error_type, &UNKNOWN_BLOCK_PROPERTY_ERROR_TYPE);
    }

    #[test]
    fn rejects_invalid_block_state_property_value() {
        let mut reader = StringReader::new("oak_stairs[facing=up]");

        let error = ArgumentType::<DummySource>::parse(&BlockStateArgumentType, &mut reader)
            .expect_err("stairs cannot face up");

        assert_eq!(error.error_type, &INVALID_BLOCK_PROPERTY_ERROR_TYPE);
    }
}
