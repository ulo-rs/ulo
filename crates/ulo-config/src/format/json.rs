//! Strict JSON through `jsonc-parser`'s AST, with comments, trailing commas and loose property
//! names refused. The AST gives a byte range per value; line and column come from counting
//! newlines.

use std::sync::Arc;

use jsonc_parser::ast::{ObjectPropName, Value as Json};
use jsonc_parser::common::Ranged;
use jsonc_parser::{CollectOptions, ParseOptions, parse_to_ast};

use super::{Lines, to_u32, whole_file};
use crate::problems::Problems;
use crate::tree::{Node, Origin, Tree, Value};

/// RFC 8259 and nothing beyond it. The crate's defaults accept comments, single quotes, hex and
/// `NaN`, none of which a `.json` file may hold.
const STRICT: ParseOptions = ParseOptions {
    allow_comments: false,
    allow_loose_object_property_names: false,
    allow_trailing_commas: false,
    allow_missing_commas: false,
    allow_single_quoted_strings: false,
    allow_hexadecimal_numbers: false,
    allow_unary_plus_numbers: false,
    allow_bare_decimal_point_numbers: false,
    allow_non_finite_numbers: false,
    allow_extended_string_escapes: false,
};

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    let mut problems = Problems::new();
    let parsed = match parse_to_ast(text, &CollectOptions::default(), &STRICT) {
        Ok(parsed) => parsed,
        Err(error) => {
            let origin = Origin::File {
                path: file.clone(),
                line: to_u32(error.line_display()),
                column: to_u32(error.column_display()),
            };
            problems.at_origin("", origin, error.kind().to_string());
            return Err(problems);
        }
    };
    let Some(document) = parsed.value else {
        problems.at_origin("", whole_file(file), "the file holds no JSON value");
        return Err(problems);
    };

    let lines = Lines::new(text);
    let mut reader = Reader { lines: &lines, file, problems };
    let root = reader.node(&document);
    if !matches!(root.value, Value::Table(_)) {
        reader.problems.at_origin("", root.origin.clone(), "the top level must be an object");
    }
    match reader.problems.into_result() {
        Ok(()) => Ok(Tree { root: Some(root) }),
        Err(problems) => Err(problems),
    }
}

struct Reader<'a> {
    lines: &'a Lines<'a>,
    file: &'a Arc<str>,
    problems: Problems,
}

impl Reader<'_> {
    fn node(&mut self, json: &Json<'_>) -> Node {
        let origin = self.lines.origin(self.file, json.range().start);
        let value = match json {
            Json::StringLit(text) => Value::String(text.value.to_string()),
            Json::NumberLit(number) => match number_value(number.value) {
                Some(value) => value,
                None => {
                    self.problems.at_origin("", origin.clone(), "number cannot be read");
                    Value::Null
                }
            },
            Json::BooleanLit(flag) => Value::Bool(flag.value),
            Json::NullKeyword(_) => Value::Null,
            Json::Array(array) => Value::Array(array.elements.iter().map(|element| self.node(element)).collect()),
            Json::Object(object) => {
                let mut entries: Vec<(String, Node)> = Vec::with_capacity(object.properties.len());
                for property in &object.properties {
                    let name = match &property.name {
                        ObjectPropName::String(text) => text.value.to_string(),
                        ObjectPropName::Word(word) => word.value.to_string(),
                    };
                    let node = self.node(&property.value);
                    match entries.iter().find(|(existing, _)| *existing == name) {
                        // A key written twice is almost always a mistake, and RFC 8259 leaves
                        // which one wins to the reader.
                        Some((_, first)) => {
                            let message = format!("key `{name}` is written twice, first at {}", first.origin);
                            self.problems.at_origin("", node.origin.clone(), message);
                        }
                        None => entries.push((name, node)),
                    }
                }
                Value::Table(entries)
            }
        };
        Node { value, origin }
    }
}

fn number_value(text: &str) -> Option<Value> {
    if !text.contains(['.', 'e', 'E'])
        && let Ok(number) = text.parse::<i128>()
    {
        return Some(Value::Integer(number));
    }
    text.parse::<f64>().ok().map(Value::Float)
}
