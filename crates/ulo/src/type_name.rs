use std::any::{TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{Hash, Hasher};

/// A type as an error report names it: `TypeName::of::<my_app::User>()` prints `User`, and with
/// `{:#}` prints `my_app::User`.
///
/// Two names compare on their `TypeId`s, so a type alias or a renamed import names the same type.
/// A report decides which names it writes with `{:#}` by running [`TypeName::colliding`] over
/// every name it holds: the wiring report over each [`Key`](crate::Key)'s type and qualifier and
/// the transports it names, the startup report over each failing transport and the names of each
/// [`PrepareError`](crate::PrepareError), and the shutdown report over its keys and transports.
/// A transport is named by its marker type, `TypeName::of::<Http>()`.
///
/// A report entry stores the names it reports as values, a `TypeName`, a [`Key`](crate::Key) or a
/// [`KeyName`](crate::KeyName), rendered when the report is formatted under the collision rule.
/// Descriptive text, such as what reads a missing key or the steps of a path between services, may
/// be stored already rendered, with the names inside it written short. A name an entry also stores
/// as a value is never written into its text: it is rendered from the value, so the entry prints
/// it one way.
#[derive(Clone, Copy)]
pub struct TypeName {
    id: TypeId,
    name: &'static str,
}

impl TypeName {
    pub fn of<T: ?Sized + 'static>() -> TypeName {
        TypeName { id: TypeId::of::<T>(), name: type_name::<T>() }
    }

    /// The names among `names` whose short form another, different type among them shares,
    /// `a::Config` beside `b::Config`: the ones a report writes with `{:#}`. Run it over every
    /// name one report prints, so a name colliding with one in another entry prints in full too.
    pub fn colliding(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName> {
        let mut by_text: HashMap<String, Vec<TypeName>> = HashMap::new();
        for name in names {
            let alike = by_text.entry(name.to_string()).or_default();
            if !alike.contains(&name) {
                alike.push(name);
            }
        }
        by_text.into_values().filter(|alike| alike.len() > 1).flatten().collect()
    }

    /// A name rebuilt from an id and a `type_name` held as runtime values, as a module's identity
    /// holds its type.
    pub(crate) fn from_parts(id: TypeId, name: &'static str) -> TypeName {
        TypeName { id, name }
    }

    pub(crate) fn id(&self) -> TypeId {
        self.id
    }

    /// The full `type_name`, as `{:#}` writes it.
    pub(crate) fn full(&self) -> &'static str {
        self.name
    }

    /// `{:#}` when `full` holds this name, `{}` otherwise.
    pub(crate) fn written(&self, full: &HashSet<TypeName>) -> String {
        if full.contains(self) { self.name.to_owned() } else { short_type_name(self.name) }
    }
}

impl PartialEq for TypeName {
    fn eq(&self, other: &Self) -> bool {
        self.id == other.id
    }
}

impl Eq for TypeName {}

impl Hash for TypeName {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.id.hash(state);
    }
}

/// `Arc<User>` for `alloc::sync::Arc<my_app::User>`: every path cut to its last segment, generic
/// arguments included. Two types whose last segments match print alike. The alternate form,
/// `{:#}`, writes the `type_name` whole.
impl fmt::Display for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if f.alternate() { f.pad(self.name) } else { f.pad(&short_type_name(self.name)) }
    }
}

impl fmt::Debug for TypeName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// `alloc::sync::Arc<my_app::db::PgPool>` reads `Arc<PgPool>`. [`TypeName`]'s `Display`, and
/// through it `Key`, `KeyName` and `ModuleName`; the reports that hold a bare `type_name` call it
/// directly.
///
/// Only the paths change: `dyn my_app::Repo + core::marker::Send` reads `dyn Repo + Send`, and the
/// punctuation of references, slices, tuples, function pointers and qualified paths is copied as
/// written.
pub(crate) fn short_type_name(full: &'static str) -> String {
    let mut out = String::with_capacity(full.len());
    let mut rest = full;
    while let Some(c) = rest.chars().next() {
        if is_path_char(c) {
            let end = rest.find(|c: char| !is_path_char(c)).unwrap_or(rest.len());
            push_last_segment(&mut out, &rest[..end]);
            rest = &rest[end..];
        } else {
            out.push(c);
            rest = &rest[c.len_utf8()..];
        }
    }
    out
}

/// The characters of one `a::b::{{closure}}` run. `type_name` writes `:` only inside `::`.
fn is_path_char(c: char) -> bool {
    c.is_alphanumeric() || matches!(c, '_' | ':' | '{' | '}')
}

/// Writes the last segment of `path`. A closure segment such as `{{closure}}` keeps the item
/// that encloses it, which is the part that names it. A leading `::`, as after the `>` of
/// `<A as Tr>::Out`, is kept.
fn push_last_segment(out: &mut String, path: &str) {
    let path = match path.strip_prefix("::") {
        Some(tail) => {
            out.push_str("::");
            tail
        }
        None => path,
    };
    let mut segments = path.rsplit("::");
    let last = segments.next().unwrap_or("");
    if last.starts_with('{') {
        if let Some(enclosing) = segments.next() {
            out.push_str(enclosing);
            out.push_str("::");
        }
    }
    out.push_str(last);
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;
    use std::sync::Arc;

    use super::*;

    mod a {
        pub struct Config;
        pub struct Wrapper<T>(pub T);
    }

    mod b {
        pub struct Config;
    }

    struct Solo;

    trait Repo {}

    type AliasOfConfig = a::Config;

    fn name<T: ?Sized + 'static>() -> TypeName {
        TypeName::of::<T>()
    }

    fn set(names: impl IntoIterator<Item = TypeName>) -> HashSet<TypeName> {
        TypeName::colliding(names)
    }

    #[test]
    fn display_cuts_every_path_segment_and_the_alternate_form_keeps_them() {
        assert_eq!(name::<a::Config>().to_string(), "Config");
        let full = format!("{:#}", name::<a::Config>());
        assert!(full.ends_with("tests::a::Config") && full.len() > "Config".len(), "{full}");
    }

    #[test]
    fn generic_arguments_are_cut_too() {
        assert_eq!(name::<Arc<a::Config>>().to_string(), "Arc<Config>");
        assert_eq!(name::<a::Wrapper<Arc<b::Config>>>().to_string(), "Wrapper<Arc<Config>>");
        assert_eq!(name::<Vec<(a::Config, &'static b::Config)>>().to_string(), "Vec<(Config, &Config)>");
    }

    #[test]
    fn the_punctuation_around_paths_is_copied_as_written() {
        assert_eq!(name::<dyn Repo + Send + Sync>().to_string(), "dyn Repo + Send + Sync");
        assert_eq!(name::<[a::Config; 3]>().to_string(), "[Config; 3]");
        assert_eq!(name::<fn(a::Config) -> b::Config>().to_string(), "fn(Config) -> Config");
    }

    #[test]
    fn a_closure_segment_keeps_the_item_that_encloses_it() {
        fn enclosing() -> TypeName {
            fn name_of<T: 'static>(_: &T) -> TypeName {
                TypeName::of::<T>()
            }
            name_of(&|| ())
        }
        assert_eq!(enclosing().to_string(), "enclosing::{{closure}}");
    }

    #[test]
    fn two_names_are_equal_when_their_types_are_not_when_their_spellings_are() {
        assert_eq!(name::<AliasOfConfig>(), name::<a::Config>());
        assert_ne!(name::<a::Config>(), name::<b::Config>());
        assert_ne!(name::<dyn Repo>(), name::<dyn Repo + Send>());
    }

    #[test]
    fn colliding_finds_equal_short_names_of_different_types() {
        let found = set([name::<a::Config>(), name::<b::Config>()]);
        assert_eq!(found, HashSet::from([name::<a::Config>(), name::<b::Config>()]));
    }

    #[test]
    fn colliding_ignores_the_same_type_listed_twice_and_names_that_differ() {
        assert!(set([name::<a::Config>(), name::<a::Config>(), name::<AliasOfConfig>()]).is_empty());
        assert!(set([name::<a::Config>(), name::<Solo>()]).is_empty());
        assert!(set([]).is_empty());
    }

    #[test]
    fn colliding_returns_only_the_names_that_collide() {
        let found = set([name::<a::Config>(), name::<Solo>(), name::<b::Config>(), name::<Solo>()]);
        assert!(found.contains(&name::<a::Config>()) && found.contains(&name::<b::Config>()));
        assert!(!found.contains(&name::<Solo>()));
        assert_eq!(found.len(), 2);
    }

    #[test]
    fn colliding_compares_the_cut_form_so_generic_arguments_count() {
        // `Arc<a::Config>` and `Arc<b::Config>` both print `Arc<Config>`.
        let found = set([name::<Arc<a::Config>>(), name::<Arc<b::Config>>(), name::<a::Config>()]);
        assert!(found.contains(&name::<Arc<a::Config>>()) && found.contains(&name::<Arc<b::Config>>()));
        assert!(!found.contains(&name::<a::Config>()), "`Config` and `Arc<Config>` print apart");
    }

    #[test]
    fn written_gives_the_full_path_to_the_names_in_the_set_only() {
        let full = set([name::<a::Config>(), name::<b::Config>()]);
        assert_eq!(name::<a::Config>().written(&full), format!("{:#}", name::<a::Config>()));
        assert_eq!(name::<Solo>().written(&full), "Solo");
        assert_eq!(name::<a::Config>().written(&HashSet::new()), "Config");
    }
}
