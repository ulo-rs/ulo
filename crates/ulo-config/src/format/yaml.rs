//! YAML 1.2 through `saphyr`'s `MarkedYaml`.
//!
//! `saphyr`'s loader clones the anchored node at each alias with no bound, so [`prescan`] reads
//! `saphyr-parser`'s event stream first and refuses the file once the nodes the loader would build
//! pass [`ALIAS_LIMIT`]. The events carry each anchor's ID and each event's position (read in
//! `saphyr-parser` 0.2.2), so the count needs no fallback that refuses every alias.
//!
//! The same scan finds a merge key. `MarkedYaml` resolves a scalar before the reader sees it and
//! keeps no style, so `<<` as a plain scalar in key position is only distinguishable in the events,
//! where a quoted `"<<"` is a legitimate string key.

use std::collections::HashMap;
use std::sync::Arc;

use saphyr::{LoadableYamlNode, MarkedYaml, Scalar, YamlData};
use saphyr_parser::{Event, Marker, Parser, ScalarStyle, Span, SpannedEventReceiver};

use super::to_u32;
use crate::problems::Problems;
use crate::tree::{Node, Origin, Tree, Value};

/// The most nodes a file may expand to. Fixed, with no setting: far beyond any configuration file
/// and far below a memory problem.
pub(crate) const ALIAS_LIMIT: usize = 100_000;

pub(crate) fn read(text: &str, file: &Arc<str>) -> Result<Tree, Problems> {
    prescan(text, file)?;

    let documents = match MarkedYaml::load_from_str(text) {
        Ok(documents) => documents,
        Err(error) => {
            let mut problems = Problems::new();
            problems.at_origin("", origin_at(file, error.marker()), error.info().to_owned());
            return Err(problems);
        }
    };
    let mut reader = Reader { file, problems: Problems::new() };
    let root = match documents.as_slice() {
        [] => return Ok(Tree::empty()),
        [only] => reader.node(only),
        [_, second, ..] => {
            let origin = origin_at(file, &second.span.start);
            reader.problems.at_origin("", origin, "the file holds more than one YAML document");
            return Err(reader.problems);
        }
    };
    reader.problems.into_result()?;
    match root.value {
        Value::Null => Ok(Tree::empty()),
        Value::Table(_) => Ok(Tree { root: Some(root) }),
        _ => {
            let mut problems = Problems::new();
            problems.at_origin("", root.origin, "the top level must be a mapping");
            Err(problems)
        }
    }
}

/// Counts the nodes inside each anchored node as its events arrive, adds an anchor's count at
/// every alias, and records a problem naming the line of the alias that passes [`ALIAS_LIMIT`].
pub(crate) fn prescan(text: &str, file: &Arc<str>) -> Result<(), Problems> {
    let mut scan = Scan { file, problems: Problems::new(), frames: Vec::new(), sizes: HashMap::new(), built: 0, refused: false };
    let mut parser = Parser::new_from_str(text);
    if let Err(error) = parser.load(&mut scan, true) {
        scan.problems.at_origin("", origin_at(file, error.marker()), error.info().to_owned());
    }
    scan.problems.into_result()
}

fn origin_at(file: &Arc<str>, marker: &Marker) -> Origin {
    // `saphyr-parser` counts lines from 1 and columns from 0.
    Origin::File { path: file.clone(), line: to_u32(marker.line()), column: to_u32(marker.col() + 1) }
}

struct Frame {
    anchor: usize,
    size: usize,
    mapping: bool,
    /// Whether the next child of a mapping is a key.
    key: bool,
}

struct Scan<'f> {
    file: &'f Arc<str>,
    problems: Problems,
    frames: Vec<Frame>,
    /// The size of each completed anchored node, by anchor ID, aliases inside it counted at their
    /// own anchor's size.
    sizes: HashMap<usize, usize>,
    built: usize,
    refused: bool,
}

impl Scan<'_> {
    fn in_key(&self) -> bool {
        matches!(self.frames.last(), Some(frame) if frame.mapping && frame.key)
    }

    fn finish(&mut self, size: usize, anchor: usize) {
        if anchor != 0 {
            self.sizes.insert(anchor, size);
        }
        if let Some(parent) = self.frames.last_mut() {
            parent.size = parent.size.saturating_add(size);
            if parent.mapping {
                parent.key = !parent.key;
            }
        }
    }
}

impl<'input> SpannedEventReceiver<'input> for Scan<'_> {
    fn on_event(&mut self, event: Event<'input>, span: Span) {
        // The parser runs on to the end of the input once a file is refused; the events are ignored.
        if self.refused {
            return;
        }
        match event {
            Event::DocumentStart(_) => {
                self.frames.clear();
                self.sizes.clear();
                self.built = 0;
            }
            Event::Scalar(value, style, anchor, _) => {
                if self.in_key() && style == ScalarStyle::Plain && &*value == "<<" {
                    let origin = origin_at(self.file, &span.start);
                    self.problems.at_origin("", origin, "merge key `<<` is YAML 1.1 and is not supported");
                }
                self.built = self.built.saturating_add(1);
                self.finish(1, anchor);
            }
            Event::Alias(anchor) => {
                let size = self.sizes.get(&anchor).copied().unwrap_or(1);
                self.built = self.built.saturating_add(size);
                if self.built > ALIAS_LIMIT {
                    let origin = origin_at(self.file, &span.start);
                    let message = format!("alias expands the document past {ALIAS_LIMIT} nodes");
                    self.problems.at_origin("", origin, message);
                    self.refused = true;
                    return;
                }
                self.finish(size, 0);
            }
            Event::SequenceStart(anchor, _) => {
                self.built = self.built.saturating_add(1);
                self.frames.push(Frame { anchor, size: 1, mapping: false, key: false });
            }
            Event::MappingStart(anchor, _) => {
                self.built = self.built.saturating_add(1);
                self.frames.push(Frame { anchor, size: 1, mapping: true, key: true });
            }
            Event::SequenceEnd | Event::MappingEnd => {
                if let Some(frame) = self.frames.pop() {
                    self.finish(frame.size, frame.anchor);
                }
            }
            Event::Nothing | Event::StreamStart | Event::StreamEnd | Event::DocumentEnd => {}
        }
    }
}

struct Reader<'f> {
    file: &'f Arc<str>,
    problems: Problems,
}

impl Reader<'_> {
    fn node(&mut self, yaml: &MarkedYaml<'_>) -> Node {
        let origin = origin_at(self.file, &yaml.span.start);
        let value = match &yaml.data {
            YamlData::Value(Scalar::Null) => Value::Null,
            YamlData::Value(Scalar::Boolean(flag)) => Value::Bool(*flag),
            YamlData::Value(Scalar::Integer(number)) => Value::Integer(i128::from(*number)),
            YamlData::Value(Scalar::FloatingPoint(number)) => Value::Float(number.0),
            YamlData::Value(Scalar::String(text)) => Value::String(text.to_string()),
            YamlData::Representation(text, _, _) => Value::String(text.to_string()),
            YamlData::Sequence(items) => Value::Array(items.iter().map(|item| self.node(item)).collect()),
            YamlData::Mapping(map) => {
                let mut entries = Vec::with_capacity(map.len());
                for (key, value) in map.iter() {
                    match key_text(key) {
                        Some(name) => {
                            let node = self.node(value);
                            entries.push((name, node));
                        }
                        None => {
                            let at = origin_at(self.file, &key.span.start);
                            self.problems.at_origin("", at, "a key must be a scalar");
                        }
                    }
                }
                Value::Table(entries)
            }
            // A user tag names a type the configuration does not use; the node is read as if untagged.
            YamlData::Tagged(_, inner) => return self.node(inner),
            YamlData::Alias(_) | YamlData::BadValue => {
                self.problems.at_origin("", origin.clone(), "the value cannot be read");
                Value::Null
            }
        };
        Node { value, origin }
    }
}

fn key_text(key: &MarkedYaml<'_>) -> Option<String> {
    match &key.data {
        YamlData::Value(Scalar::String(text)) => Some(text.to_string()),
        YamlData::Value(Scalar::Integer(number)) => Some(number.to_string()),
        YamlData::Value(Scalar::Boolean(flag)) => Some(flag.to_string()),
        YamlData::Value(Scalar::FloatingPoint(number)) => Some(number.0.to_string()),
        YamlData::Representation(text, _, _) => Some(text.to_string()),
        YamlData::Tagged(_, inner) => key_text(inner),
        _ => None,
    }
}
