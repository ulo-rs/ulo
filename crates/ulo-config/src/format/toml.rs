//! TOML 1.0 through the `toml` crate's spanned parse; a leaf's line and column come from its byte
//! offset.

use std::sync::Arc;

use toml::Spanned;
use toml::de::{DeTable, DeValue};

use super::{Lines, whole_file};
use crate::problems::Problems;
use crate::tree::{Node, Tree, Value};

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    let lines = Lines::new(text);
    let table = match DeTable::parse(text) {
        Ok(table) => table,
        Err(error) => {
            // The error's own `Display` quotes the offending source line, which may hold a secret.
            let origin = match error.span() {
                Some(span) => lines.origin(file, span.start),
                None => whole_file(file),
            };
            let mut problems = Problems::new();
            problems.at_origin("", origin, error.message().to_owned());
            return Err(problems);
        }
    };

    let mut reader = Reader { lines: &lines, file, problems: Problems::new() };
    let root = Node { value: Value::Table(reader.table(table.get_ref())), origin: lines.origin(file, 0) };
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
    fn node(&mut self, spanned: &Spanned<DeValue<'_>>) -> Node {
        let origin = self.lines.origin(self.file, spanned.span().start);
        let value = match spanned.get_ref() {
            DeValue::String(text) => Value::String(text.to_string()),
            DeValue::Integer(integer) => match i128::from_str_radix(integer.as_str(), integer.radix()) {
                Ok(number) => Value::Integer(number),
                Err(_) => {
                    self.problems.at_origin("", origin.clone(), "integer is too large");
                    Value::Null
                }
            },
            DeValue::Float(float) => match float.as_str().parse::<f64>() {
                Ok(number) => Value::Float(number),
                Err(_) => {
                    self.problems.at_origin("", origin.clone(), "float is not a number");
                    Value::Null
                }
            },
            DeValue::Boolean(flag) => Value::Bool(*flag),
            // A date or time is read as the text TOML wrote it, which `String`, `chrono` and `time`
            // parse alike.
            DeValue::Datetime(moment) => Value::String(moment.to_string()),
            DeValue::Array(items) => Value::Array(items.iter().map(|item| self.node(item)).collect()),
            DeValue::Table(table) => Value::Table(self.table(table)),
        };
        Node { value, origin }
    }

    fn table(&mut self, table: &DeTable<'_>) -> Vec<(String, Node)> {
        table.iter().map(|(key, value)| (key.get_ref().to_string(), self.node(value))).collect()
    }
}
