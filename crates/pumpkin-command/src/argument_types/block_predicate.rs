use std::collections::HashMap;

use pumpkin_data::tag::{RegistryKey, get_tag_ids};
use pumpkin_data::{Block, BlockState, translation};
use pumpkin_nbt::compound::NbtCompound;
use pumpkin_util::text::TextComponent;

use crate::argument_types::argument_type::{ArgumentType, JavaClientArgumentType};
use crate::argument_types::block::{BlockStateArgumentType, DUPLICATE_BLOCK_PROPERTY_ERROR_TYPE};
use crate::argument_types::nbt::NbtCompoundArgumentType;
use crate::argument_types::nbt_path::compare_nbt;
use crate::context::command_context::CommandContext;
use crate::errors::command_syntax_error::CommandSyntaxError;
use crate::errors::error_types::CommandErrorType;
use crate::string_reader::StringReader;
use crate::suggestion::suggestions::{Suggestions, SuggestionsBuilder};

pub const ERROR_UNKNOWN_TAG: CommandErrorType<1> = CommandErrorType::new(
    translation::java::ARGUMENTS_BLOCK_TAG_UNKNOWN,
    translation::java::ARGUMENTS_BLOCK_TAG_UNKNOWN,
);

pub const ERROR_NO_VALUE: CommandErrorType<2> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
    translation::java::ARGUMENT_BLOCK_PROPERTY_NOVALUE,
);

pub const ERROR_UNCLOSED_PROPERTIES: CommandErrorType<0> = CommandErrorType::new(
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
    translation::java::ARGUMENT_BLOCK_PROPERTY_UNCLOSED,
);

#[derive(Clone, Debug)]
pub enum BlockPredicate {
    Block {
        block: &'static Block,
        properties: HashMap<String, String>,
        nbt: Option<NbtCompound>,
    },
    Tag {
        tag_name: String,
        block_ids: Vec<u16>,
        properties: HashMap<String, String>,
        nbt: Option<NbtCompound>,
    },
}

impl BlockPredicate {
    #[must_use]
    pub fn test(&self, state: &BlockState, nbt: Option<&NbtCompound>) -> bool {
        let block = Block::from_state_id(state.id);
        let (matches_block, properties, expected_nbt) = match self {
            Self::Block {
                block: expected,
                properties,
                nbt,
            } => (block.id == expected.id, properties, nbt),
            Self::Tag {
                block_ids,
                properties,
                nbt,
                ..
            } => (block_ids.contains(&block.id.as_u16()), properties, nbt),
        };
        if !matches_block {
            return false;
        }
        if !properties.is_empty() {
            let actual = block
                .properties(state.id)
                .map(|properties| properties.to_props())
                .unwrap_or_default();
            if !properties
                .iter()
                .all(|(name, value)| actual.contains(&(name.as_str(), value.as_str())))
            {
                return false;
            }
        }
        expected_nbt.as_ref().is_none_or(|pattern| {
            nbt.is_some_and(|actual| {
                pattern.child_tags.iter().all(|(name, value)| {
                    actual
                        .child_tags
                        .get(name)
                        .is_some_and(|actual| compare_nbt(value, actual))
                })
            })
        })
    }

    #[must_use]
    pub const fn requires_nbt(&self) -> bool {
        match self {
            Self::Block { nbt, .. } | Self::Tag { nbt, .. } => nbt.is_some(),
        }
    }
}

fn parse_properties(
    reader: &mut StringReader,
    tag_name: &str,
) -> Result<HashMap<String, String>, CommandSyntaxError> {
    let mut properties = HashMap::new();
    if reader.peek() != Some('[') {
        return Ok(properties);
    }
    reader.skip();
    reader.skip_whitespace();
    while reader.can_read_char() && reader.peek() != Some(']') {
        let start = reader.cursor();
        let key = reader.read_string()?;
        if properties.contains_key(&key) {
            reader.set_cursor(start);
            return Err(DUPLICATE_BLOCK_PROPERTY_ERROR_TYPE.create(
                reader,
                TextComponent::text(key),
                TextComponent::text(tag_name.to_string()),
            ));
        }
        reader.skip_whitespace();
        if reader.peek() != Some('=') {
            return Err(ERROR_NO_VALUE.create(
                reader,
                TextComponent::text(key),
                TextComponent::text(tag_name.to_string()),
            ));
        }
        reader.skip();
        reader.skip_whitespace();
        let value = reader.read_string()?;
        properties.insert(key, value);
        reader.skip_whitespace();
        match reader.peek() {
            Some(',') => {
                reader.skip();
                reader.skip_whitespace();
            }
            Some(']') => {}
            _ => return Err(ERROR_UNCLOSED_PROPERTIES.create(reader)),
        }
    }
    if reader.peek() != Some(']') {
        return Err(ERROR_UNCLOSED_PROPERTIES.create(reader));
    }
    reader.skip();
    Ok(properties)
}

fn parse_nbt(reader: &mut StringReader) -> Result<Option<NbtCompound>, CommandSyntaxError> {
    if reader.peek() == Some('{') {
        let tag =
            ArgumentType::<crate::source::DummySource>::parse(&NbtCompoundArgumentType, reader)?;
        Ok(Some(tag))
    } else {
        Ok(None)
    }
}

pub struct BlockPredicateArgumentType;

impl<S: crate::source::CommandSource> ArgumentType<S> for BlockPredicateArgumentType {
    type Item = BlockPredicate;

    fn parse(&self, reader: &mut StringReader) -> Result<Self::Item, CommandSyntaxError> {
        if reader.peek() == Some('#') {
            reader.skip();
            let start = reader.cursor();
            while let Some(c) = reader.peek() {
                if c.is_alphanumeric() || c == '_' || c == ':' || c == '/' || c == '.' || c == '-' {
                    reader.skip();
                } else {
                    break;
                }
            }
            let tag_str = &reader.string()[start..reader.cursor()];
            let normalized = if tag_str.contains(':') {
                tag_str.to_string()
            } else {
                format!("minecraft:{tag_str}")
            };
            let stripped = normalized.strip_prefix("minecraft:").unwrap_or(&normalized);

            let tag_ids = get_tag_ids(RegistryKey::Block, &normalized)
                .or_else(|| get_tag_ids(RegistryKey::Block, stripped));

            let block_ids = match tag_ids {
                Some(ids) => ids.to_vec(),
                None => {
                    return Err(ERROR_UNKNOWN_TAG.create(reader, TextComponent::text(normalized)));
                }
            };

            let properties = parse_properties(reader, &normalized)?;
            let nbt = parse_nbt(reader)?;

            Ok(BlockPredicate::Tag {
                tag_name: normalized,
                block_ids,
                properties,
                nbt,
            })
        } else {
            let parsed = ArgumentType::<S>::parse(&BlockStateArgumentType, reader)?;
            Ok(BlockPredicate::Block {
                block: parsed.block,
                properties: parsed.properties,
                nbt: parsed.nbt,
            })
        }
    }

    fn client_side_parser(&self) -> JavaClientArgumentType {
        JavaClientArgumentType::BlockPredicate
    }

    fn examples(&self) -> Vec<String> {
        vec![
            "stone".to_string(),
            "minecraft:stone".to_string(),
            "stone[foo=bar]".to_string(),
            "#stone".to_string(),
            "#stone[foo=bar]{baz=nbt}".to_string(),
        ]
    }

    fn list_suggestions(
        &self,
        _context: &CommandContext<S>,
        builder: SuggestionsBuilder,
    ) -> Suggestions {
        builder.build()
    }
}

impl BlockPredicateArgumentType {
    pub fn get<S: crate::source::CommandSource>(
        context: &CommandContext<S>,
        name: &str,
    ) -> Result<BlockPredicate, CommandSyntaxError> {
        Ok(context.get_argument::<BlockPredicate>(name)?.clone())
    }
}

#[cfg(test)]
mod tests {
    use super::{BlockPredicate, BlockPredicateArgumentType};
    use crate::argument_types::argument_type::ArgumentType;
    use crate::argument_types::block::BlockStateArgumentType;
    use crate::argument_types::nbt::NbtCompoundArgumentType;
    use crate::source::DummySource;
    use crate::string_reader::StringReader;

    fn predicate(input: &str) -> BlockPredicate {
        ArgumentType::<DummySource>::parse(
            &BlockPredicateArgumentType,
            &mut StringReader::new(input),
        )
        .expect("valid predicate")
    }

    fn state(input: &str) -> &'static pumpkin_data::BlockState {
        ArgumentType::<DummySource>::parse(&BlockStateArgumentType, &mut StringReader::new(input))
            .expect("valid state")
            .state
    }

    #[test]
    fn matches_only_explicit_properties() {
        let predicate = predicate("oak_stairs[facing=east]");
        assert!(predicate.test(state("oak_stairs[facing=east,half=top]"), None));
        assert!(predicate.test(state("oak_stairs[facing=east,half=bottom]"), None));
        assert!(!predicate.test(state("oak_stairs[facing=west]"), None));
        assert!(!predicate.test(state("stone"), None));
    }

    #[test]
    fn tag_properties_match_across_block_types() {
        let predicate = predicate("#minecraft:logs[axis=x]");
        assert!(predicate.test(state("oak_log[axis=x]"), None));
        assert!(predicate.test(state("birch_log[axis=x]"), None));
        assert!(!predicate.test(state("oak_log[axis=y]"), None));
        assert!(!predicate.test(state("stone"), None));
        assert!(!self::predicate("#minecraft:logs[missing=value]").test(state("oak_log"), None));
    }

    #[test]
    fn nbt_matching_uses_partial_compounds_and_unordered_lists() {
        let actual = ArgumentType::<DummySource>::parse(
            &NbtCompoundArgumentType,
            &mut StringReader::new(
                r#"{Items:[{id:"minecraft:stone",count:2},{id:"minecraft:diamond",count:3}],x:17}"#,
            ),
        )
        .expect("valid NBT");
        let chest = state("chest");
        assert!(predicate(r#"chest{Items:[{id:"minecraft:diamond"}]}"#).test(chest, Some(&actual)));
        assert!(
            !predicate(r#"chest{Items:[{id:"minecraft:emerald"}]}"#).test(chest, Some(&actual))
        );
        assert!(!predicate("chest{Items:[]}").test(chest, Some(&actual)));
        assert!(!predicate("chest{}").test(chest, None));
        assert!(predicate("chest{}").test(chest, Some(&actual)));
    }

    #[test]
    fn rejects_invalid_properties_on_explicit_block_predicates() {
        for input in [
            "stone[facing=east]",
            "oak_stairs[facing=up]",
            "oak_stairs[facing=east,facing=west]",
        ] {
            let mut reader = StringReader::new(input);
            assert!(
                ArgumentType::<DummySource>::parse(&BlockPredicateArgumentType, &mut reader)
                    .is_err(),
                "accepted invalid predicate: {input}"
            );
        }
    }

    #[test]
    fn rejects_duplicate_properties_on_tag_predicates() {
        let mut reader = StringReader::new("#minecraft:logs[axis=x,axis=y]");
        assert!(
            ArgumentType::<DummySource>::parse(&BlockPredicateArgumentType, &mut reader).is_err()
        );
    }
}
