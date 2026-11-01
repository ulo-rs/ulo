//! Modules: Rust types with an identity that declare imports, bindings, controllers and exports
//! (§3.6, §8).

pub(crate) mod def;
pub(crate) mod dynamic;
pub(crate) mod handle;
pub(crate) mod keyed;
pub(crate) mod meta;

use std::any::{Any, TypeId, type_name};
use std::collections::{HashMap, HashSet};
use std::fmt;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::sync::Arc;

use crate::binding::Qualifier;
use crate::module::def::ModuleDef;
use crate::module::keyed::Keyed;
use crate::type_name::short_type_name;

/// A module: an identity and a synchronous, I/O-free registration.
///
/// `register` returns `()` and never returns early. A value that may fail to build goes through
/// [`ModuleDef::try_value`], which records the `Err` for `wire()` to report beside every other
/// wiring error.
pub trait Module: Send + Sync + 'static {
    /// Identity for deduplication and diagnostics.
    fn identity(&self) -> ModuleIdentity;

    /// Declare everything. Synchronous and free of I/O.
    fn register(&self, m: &mut ModuleDef<'_>);

    /// What this module is configured with outside its identity. A second import of an equal
    /// identity registers nothing, so it must carry the same settings: `wire()` refuses one that
    /// differs as `WiringError::DifferingSettings`, naming both import sites and each setting
    /// that differs. A module whose identity holds all of its configuration has none, the
    /// default.
    fn settings(&self) -> ModuleSettings {
        ModuleSettings::new()
    }

    /// This module as a keyed instance: inside it injection points stay unqualified, and at its
    /// export boundary every unqualified export, re-exports included, is requalified as `T @ Q`.
    /// `DbModule::for_root(url).keyed::<Primary>()`.
    fn keyed<Q: 'static>(self) -> Keyed<Q, Self>
    where
        Self: Sized,
    {
        Keyed::new(self)
    }
}

/// Who a module is: its type, plus its configuration value if it has one, plus a qualifier if
/// it is keyed. Two configurations of one type are two modules; the same configuration imported
/// twice is one, its [`settings`](Module::settings) equal in both imports.
///
/// Equality and hashing ignore the label. The configuration's `Debug` output is never printed,
/// because it may hold credentials.
#[derive(Clone)]
pub struct ModuleIdentity {
    ty: TypeId,
    ty_name: &'static str,
    config: Option<ConfigKey>,
    qualifier: Option<Qualifier>,
    label: Option<&'static str>,
}

impl ModuleIdentity {
    /// A unit module: its type alone.
    pub fn of_type<M: 'static>() -> Self {
        ModuleIdentity { ty: TypeId::of::<M>(), ty_name: type_name::<M>(), config: None, qualifier: None, label: None }
    }

    /// A configured module: its type and its value.
    pub fn of_value<M: Eq + Hash + Clone + Send + Sync + 'static>(m: &M) -> Self {
        ModuleIdentity::of_owner::<M, M>(m)
    }

    /// The name diagnostics print in place of the type name.
    pub fn label(self, name: &'static str) -> Self {
        ModuleIdentity { label: Some(name), ..self }
    }

    /// A module of type `O` configured by the value `config`, found by `app.module::<O>()` and
    /// `app.module_keyed::<O, Q>()` and printed as `O`. A generic module whose configuration is
    /// its own type's field, as a `DynamicModule` is, names itself `O` and keeps `config` for
    /// equality.
    pub fn of_owner<O: 'static, C: Eq + Hash + Clone + Send + Sync + 'static>(config: &C) -> Self {
        // `DefaultHasher::new` is unkeyed: equal configurations hash alike in every map holding
        // the identity. The configuration's type is hashed too, which keeps two configurations
        // of different types apart before the value comparison runs.
        let mut hasher = DefaultHasher::new();
        TypeId::of::<C>().hash(&mut hasher);
        config.hash(&mut hasher);
        ModuleIdentity {
            ty: TypeId::of::<O>(),
            ty_name: type_name::<O>(),
            config: Some(ConfigKey { hash: hasher.finish(), value: Arc::new(config.clone()) }),
            qualifier: None,
            label: None,
        }
    }

    pub(crate) fn keyed_by(self, qualifier: Qualifier) -> Self {
        ModuleIdentity { qualifier: Some(qualifier), ..self }
    }

    pub(crate) fn type_id(&self) -> TypeId {
        self.ty
    }

    pub(crate) fn type_name(&self) -> &'static str {
        self.ty_name
    }

    pub(crate) fn qualifier(&self) -> Option<Qualifier> {
        self.qualifier
    }
}

impl PartialEq for ModuleIdentity {
    fn eq(&self, other: &Self) -> bool {
        self.ty == other.ty && self.qualifier == other.qualifier && self.config == other.config
    }
}

impl Eq for ModuleIdentity {}

impl Hash for ModuleIdentity {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ty.hash(state);
        self.qualifier.hash(state);
        self.config.as_ref().map(|c| c.hash).hash(state);
    }
}

/// The settings a module answers from [`Module::settings`]: each a name and its value as `Debug`
/// writes it, in the order written. Two imports compare by that text, so a value whose `Debug`
/// text differs between equal values, a `HashMap` iterating in another order or a pointer, makes
/// two equal registrations differ.
///
/// A wiring report prints the values. A credential belongs in the identity, whose configuration
/// is never printed.
///
/// ```
/// # use std::time::Duration;
/// # use ulo::ModuleSettings;
/// let settings = ModuleSettings::new().with("connect_within", Duration::from_secs(5)).with("lazy", false);
/// assert_ne!(settings, ModuleSettings::new().with("connect_within", Duration::from_secs(10)).with("lazy", false));
/// ```
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ModuleSettings {
    entries: Vec<(&'static str, String)>,
}

impl ModuleSettings {
    pub fn new() -> Self {
        ModuleSettings::default()
    }

    /// Records the setting `name` with `value` written by `Debug`.
    pub fn with(mut self, name: &'static str, value: impl fmt::Debug) -> Self {
        self.entries.push((name, format!("{value:?}")));
        self
    }

    /// Each setting whose value differs between `self` and `other`, or that one of them lacks, as
    /// `(name, self's value, other's value)`; a missing value is `None`. In `self`'s order, then
    /// the names only `other` has.
    pub(crate) fn differences(&self, other: &ModuleSettings) -> Vec<(&'static str, Option<String>, Option<String>)> {
        let value = |settings: &ModuleSettings, name: &str| {
            settings.entries.iter().find(|(written, _)| *written == name).map(|(_, value)| value.clone())
        };
        let mut names: Vec<&'static str> = self.entries.iter().map(|(name, _)| *name).collect();
        names.extend(other.entries.iter().map(|(name, _)| *name).filter(|name| !self.entries.iter().any(|(own, _)| own == name)));
        names
            .into_iter()
            .filter_map(|name| {
                let (first, second) = (value(self, name), value(other, name));
                (first != second).then_some((name, first, second))
            })
            .collect()
    }
}

/// A configuration value erased for identity: its precomputed hash and a comparison by value.
#[derive(Clone)]
pub(crate) struct ConfigKey {
    pub(crate) hash: u64,
    pub(crate) value: Arc<dyn DynKey>,
}

impl PartialEq for ConfigKey {
    fn eq(&self, other: &Self) -> bool {
        self.hash == other.hash && self.value.eq_dyn(&*other.value)
    }
}

pub(crate) trait DynKey: Any + Send + Sync {
    fn eq_dyn(&self, other: &dyn DynKey) -> bool;
    fn as_any(&self) -> &dyn Any;
}

impl<C: Eq + Send + Sync + 'static> DynKey for C {
    fn eq_dyn(&self, other: &dyn DynKey) -> bool {
        other.as_any().downcast_ref::<C>() == Some(self)
    }

    fn as_any(&self) -> &dyn Any {
        self
    }
}

/// A module as the errors name it: the type name or its label, the qualifier, and a position for
/// a second configuration of one type, as in `DbModule @ Replica` or `DbModule #2`.
///
/// Type names are cut to their last path segment, and `{:#}` writes them with their full paths:
/// `my_app::db::DbModule @ my_app::Replica #2`. A label is not a path and prints in both forms;
/// the alternate form follows it with the module type's full path, `redis (my_app::Redis)`, so two
/// module types sharing a label print apart too. `Debug` passes the flag along, so `{:#?}` writes
/// full paths as well.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct ModuleName {
    text: Arc<str>,
    full: Arc<str>,
}

impl ModuleName {
    /// Names `identity`. `instance` is its zero-based position, in collection order, among the
    /// registered modules of its type and qualifier; from the second on it is printed as `#n`,
    /// labelled or not, since several configurations commonly share one label.
    pub(crate) fn of(identity: &ModuleIdentity, instance: usize) -> Self {
        ModuleName {
            text: Arc::from(ModuleName::render(identity, instance, false)),
            full: Arc::from(ModuleName::render(identity, instance, true)),
        }
    }

    fn render(identity: &ModuleIdentity, instance: usize, full: bool) -> String {
        let path = |name: &'static str| if full { name.to_owned() } else { short_type_name(name) };
        let mut text = match identity.label {
            Some(label) if full => format!("{label} ({})", identity.ty_name),
            Some(label) => label.to_owned(),
            None => path(identity.ty_name),
        };
        if let Some(qualifier) = identity.qualifier {
            text.push_str(" @ ");
            text.push_str(&path(qualifier.name));
        }
        if instance > 0 {
            text.push_str(&format!(" #{}", instance + 1));
        }
        text
    }

    /// The short form, as `{}` writes it.
    pub fn as_str(&self) -> &str {
        &self.text
    }
}

impl fmt::Display for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(if f.alternate() { &self.full } else { &self.text })
    }
}

impl fmt::Debug for ModuleName {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        fmt::Display::fmt(self, f)
    }
}

/// The names among `names` whose short form a different module's name shares, as
/// `billing::Module` beside `users::Module`: a report prints these with `{:#}`.
pub(crate) fn colliding_names<'a>(names: impl IntoIterator<Item = &'a ModuleName>) -> HashSet<ModuleName> {
    let mut by_text: HashMap<&'a str, Vec<&'a ModuleName>> = HashMap::new();
    for name in names {
        let group = by_text.entry(name.as_str()).or_default();
        if !group.contains(&name) {
            group.push(name);
        }
    }
    by_text.into_values().filter(|group| group.len() > 1).flatten().cloned().collect()
}
