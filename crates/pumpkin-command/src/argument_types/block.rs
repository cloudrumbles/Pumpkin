use std::collections::HashSet;

use crate::{
    argument_types::argument_type::{ArgumentType, JavaClientArgumentType},
    context::command_context::CommandContext,
    errors::command_syntax_error::CommandSyntaxError,
    errors::error_types::CommandErrorType,
    string_reader::StringReader,
    suggestion::suggestions::{Suggestions, SuggestionsBuilder},
};
use pumpkin_data::{Block, BlockState, Property, translation};
use pumpkin_util::text::TextComponent;

pub const INVALID_BLOCK_ERROR_TYPE: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
    translation::java::ARGUMENT_BLOCK_ID_INVALID,
);

pub const DUPLICATE_BLOCK_PROPERTY_ERROR_TYPE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_DUPLICATE,
);

pub const INVALID_BLOCK_PROPERTY_ERROR_TYPE: CommandErrorType<3> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
    translation::java::ARGUMENT_BLOCK_PROPERTY_INVALID,
);

pub const MISSING_BLOCK_PROPERTY_VALUE_ERROR_TYPE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
);

pub const UNCLOSED_BLOCK_PROPERTIES_ERROR_TYPE: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
);

pub const UNKNOWN_BLOCK_PROPERTY_ERROR_TYPE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNKNOWN,
);

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockArgumentType {
    type Item = &'static Block;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        parse_block(reader)
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

#[derive(Debug, Copy, Clone, PartialEq, Eq)]
pub struct BlockStateArgument {
    pub block: &'static Block,
    pub state: &'static BlockState,
}

#[derive(Debug, Copy, Clone, PartialEq, Eq, Hash)]
pub struct BlockStateArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockStateArgumentType {
    type Item = BlockStateArgument;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        let block = parse_block(reader)?;
        let state = parse_state_properties(reader, block)?;
        Ok(BlockStateArgument { block, state })
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
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

impl BlockStateArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<BlockStateArgument, CommandSyntaxError> {
        context.get_argument::<BlockStateArgument>(name).copied()
    }
}

fn parse_block(reader: &mut StringReader) -> Result<&'static Block, CommandSyntaxError> {
    let start = reader.cursor();
    while let Some(c) = reader.peek() {
        if c.is_ascii_alphanumeric() || matches!(c, '_' | ':' | '/' | '.' | '-') {
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

fn parse_state_properties(
    reader: &mut StringReader,
    block: &'static Block,
) -> Result<&'static BlockState, CommandSyntaxError> {
    if reader.peek() != Some('[') {
        return Ok(block.default_state);
    }
    reader.skip();
    reader.skip_whitespace();

    let block_name = format!("minecraft:{}", block.name);
    let mut properties: Vec<(String, String)> = block
        .properties(block.default_state.id)
        .map(|properties| {
            properties
                .to_props()
                .into_iter()
                .map(|property| {
                    (
                        property.key_str().to_string(),
                        property.value_str().to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    let mut seen = HashSet::<String>::new();
    let mut last_property = None::<(String, String)>;

    while reader.can_read_char() && reader.peek() != Some(']') {
        reader.skip_whitespace();
        let name_start = reader.cursor();
        let name = reader.read_string()?;

        if seen.contains(&name) {
            reader.set_cursor(name_start);
            return Err(DUPLICATE_BLOCK_PROPERTY_ERROR_TYPE.create(
                reader,
                TextComponent::text(name),
                TextComponent::text(block_name),
            ));
        }

        let Some(property) = properties
            .iter_mut()
            .find(|(property_name, _)| property_name == &name)
        else {
            reader.set_cursor(name_start);
            return Err(UNKNOWN_BLOCK_PROPERTY_ERROR_TYPE.create(
                reader,
                TextComponent::text(block_name),
                TextComponent::text(name),
            ));
        };

        reader.skip_whitespace();
        if reader.peek() != Some('=') {
            return Err(MISSING_BLOCK_PROPERTY_VALUE_ERROR_TYPE.create(
                reader,
                TextComponent::text(name),
                TextComponent::text(block_name),
            ));
        }
        reader.skip();
        reader.skip_whitespace();

        let value_start = reader.cursor();
        let value = reader.read_string()?;

        let value_is_valid = block.states.iter().any(|state| {
            block.properties(state.id).is_some_and(|properties| {
                properties
                    .to_props()
                    .iter()
                    .any(|property| property.key_str() == name && property.value_str() == value)
            })
        });
        if !value_is_valid {
            reader.set_cursor(value_start);
            return Err(INVALID_BLOCK_PROPERTY_ERROR_TYPE.create(
                reader,
                TextComponent::text(block_name),
                TextComponent::text(value),
                TextComponent::text(name),
            ));
        }

        property.1.clone_from(&value);
        seen.insert(name.clone());
        last_property = Some((name, value));

        reader.skip_whitespace();
        match reader.peek() {
            Some(',') => reader.skip(),
            Some(']') => {}
            _ => return Err(UNCLOSED_BLOCK_PROPERTIES_ERROR_TYPE.create(reader)),
        }
    }

    if reader.peek() != Some(']') {
        return Err(UNCLOSED_BLOCK_PROPERTIES_ERROR_TYPE.create(reader));
    }
    reader.skip();

    let parsed: Option<Vec<Property>> = properties
        .iter()
        .map(|(name, value)| Property::from_str_pair(name, value))
        .collect();
    if let Some(state) = parsed.and_then(|parsed| block.state_from_properties(&parsed)) {
        return Ok(state);
    }

    let (name, value) = last_property.unwrap_or_default();
    Err(INVALID_BLOCK_PROPERTY_ERROR_TYPE.create(
        reader,
        TextComponent::text(block_name),
        TextComponent::text(value),
        TextComponent::text(name),
    ))
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
            .to_props()
            .into_iter()
            .map(|property| (property.key_str(), property.value_str()))
            .collect::<Vec<_>>();
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
