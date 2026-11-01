//! The registration walk: calls `Module::register` once per identity, follows imports in the
//! order written, and computes collection order (§3.2, §10.1 step 1).

use std::any::TypeId;
use std::collections::HashMap;
use std::panic::Location;

use crate::binding::Qualifier;
use crate::error::wiring::{DifferingSetting, ModuleSite, WiringError};
use crate::graph::{Graph, ModuleId, boundary_key, order};
use crate::key::Key;
use crate::module::def::{ModuleDef, ModuleNode};
use crate::module::{Module, ModuleIdentity, ModuleName, ModuleSettings};
use crate::testing::{Replacement, TestPlan};

/// Every module reached from a root, before freezing.
pub(crate) struct Registry {
    /// In first-reached order; `post_order` maps them to collection order.
    pub(crate) nodes: Vec<ModuleNode>,
    pub(crate) by_identity: HashMap<ModuleIdentity, usize>,
    /// Per node, its imports as node indices, in the order written. For a lazy registration an
    /// import of an already-frozen identity is `Import::Existing`.
    pub(crate) imports: Vec<Vec<Import>>,
    /// Depth-first post-order over imports from the root: collection order.
    pub(crate) post_order: Vec<usize>,
    /// Each import cycle, as the node path from the first module back to itself.
    pub(crate) import_cycles: Vec<Vec<usize>>,
    /// The modules a test's `replace_module` swapped out, each with what it would have exported.
    pub(crate) replaced: Vec<Replaced>,
    /// Each later registration of an identity already claimed whose settings differ from the
    /// claiming one's.
    pub(crate) differing: Vec<Differing>,
}

#[derive(Clone, Copy, Debug)]
pub(crate) enum Import {
    New(usize),
    Existing(ModuleId),
}

/// Where a registration was reached: the root of a walk, `Root` for the app's and `Load` for a
/// lazily loaded module, or an import written in a module.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Site {
    Root,
    Load,
    Import { importer: Import, at: &'static Location<'static> },
}

/// A second registration of `module` with settings other than the first's.
pub(crate) struct Differing {
    pub(crate) module: Import,
    pub(crate) first: Site,
    pub(crate) second: Site,
    pub(crate) settings: Vec<DifferingSetting>,
}

impl Differing {
    /// `None` when the settings agree. `first` belongs to the registration holding the identity.
    pub(crate) fn of(
        module: Import,
        first: (&ModuleSettings, Site),
        second: (&ModuleSettings, Site),
    ) -> Option<Differing> {
        let settings: Vec<DifferingSetting> = first
            .0
            .differences(second.0)
            .into_iter()
            .map(|(name, first, second)| DifferingSetting { name, first, second })
            .collect();
        (!settings.is_empty()).then_some(Differing { module, first: first.1, second: second.1, settings })
    }

    /// The report entry. `names` are the registry's by node, and `base` holds the modules an
    /// `Import::Existing` names.
    pub(crate) fn into_error(self, names: &[ModuleName], base: &Graph) -> WiringError {
        let name = |import: Import| match import {
            Import::New(node) => names[node].clone(),
            Import::Existing(id) => base.module(id).name.clone(),
        };
        let site = |site: Site| match site {
            Site::Root => ModuleSite::Root,
            Site::Load => ModuleSite::Load,
            Site::Import { importer, at } => ModuleSite::Import { importer: name(importer), at },
        };
        WiringError::DifferingSettings {
            module: name(self.module),
            first: site(self.first),
            second: site(self.second),
            settings: self.settings,
        }
    }
}

pub(crate) struct Replaced {
    pub(crate) original: ModuleIdentity,
    /// The original's export keys as its importers would have seen them.
    pub(crate) exports: Vec<Key>,
    /// The node the replacement registered as.
    pub(crate) replacement: usize,
}

impl Registry {
    /// Every node's diagnostic name. A module's instance number counts the earlier modules of
    /// its type and qualifier in collection order, the frozen graph's first.
    pub(crate) fn names(&self, base: Option<&Graph>) -> Vec<ModuleName> {
        let mut counts: HashMap<(TypeId, Option<Qualifier>), usize> = HashMap::new();
        for module in base.into_iter().flat_map(|base| &base.modules) {
            *counts.entry((module.identity.type_id(), module.identity.qualifier())).or_insert(0) += 1;
        }
        let mut names: Vec<Option<ModuleName>> = vec![None; self.nodes.len()];
        for &node in &self.post_order {
            let identity = &self.nodes[node].identity;
            let count = counts.entry((identity.type_id(), identity.qualifier())).or_insert(0);
            names[node] = Some(ModuleName::of(identity, *count));
            *count += 1;
        }
        names
            .into_iter()
            .zip(&self.nodes)
            .map(|(name, node)| name.unwrap_or_else(|| ModuleName::of(&node.identity, 0)))
            .collect()
    }
}

/// Registers `root` and everything it imports. Two imports of an equal identity are one module
/// and `register` runs once for it; a later one whose settings differ is recorded in
/// `differing`. A `TestPlan`'s `replace_module` swaps a module by identity before its `register`
/// runs.
pub(crate) fn register_all(root: Box<dyn Module>, plan: Option<&TestPlan>) -> Registry {
    let mut walk = Walk::new(None, plan);
    walk.reach(&*root, Site::Root);
    walk.finish()
}

/// Registers a lazily loaded module against a frozen graph. An import whose identity the graph
/// already holds links to it and is not registered again, its settings compared with the frozen
/// module's.
pub(crate) fn register_lazy(base: &Graph, module: Box<dyn Module>) -> Registry {
    let mut walk = Walk::new(Some(base), None);
    walk.reach(&*module, Site::Load);
    walk.finish()
}

struct Walk<'a> {
    registry: Registry,
    base: Option<&'a Graph>,
    plan: Option<&'a TestPlan>,
    /// The nodes on the import path being walked, root first: an import of one of them closes
    /// a cycle.
    path: Vec<usize>,
}

impl<'a> Walk<'a> {
    fn new(base: Option<&'a Graph>, plan: Option<&'a TestPlan>) -> Self {
        Walk {
            registry: Registry {
                nodes: Vec::new(),
                by_identity: HashMap::new(),
                imports: Vec::new(),
                post_order: Vec::new(),
                import_cycles: Vec::new(),
                replaced: Vec::new(),
                differing: Vec::new(),
            },
            base,
            plan,
            path: Vec::new(),
        }
    }

    fn finish(self) -> Registry {
        let mut registry = self.registry;
        registry.post_order = order::collection_order(&registry);
        registry
    }

    fn reach(&mut self, module: &dyn Module, site: Site) -> Import {
        let identity = module.identity();
        if let Some((base, &existing)) = self.base.and_then(|base| Some((base, base.by_identity.get(&identity)?))) {
            let frozen = base.module(existing);
            let found = Import::Existing(existing);
            self.registry.differing.extend(Differing::of(found, (&frozen.settings, frozen.site), (&module.settings(), site)));
            return found;
        }
        let plan = self.plan;
        if let Some(replacement) = plan.and_then(|plan| plan.replacements.iter().find(|r| r.original == identity)) {
            return self.replace(module, identity, replacement, site);
        }
        self.enter(identity, module, site)
    }

    /// Registers `replacement` where `original` was imported. The original's `register` runs
    /// once, into a node that is then dropped, only to learn what it exported: the replacement
    /// must export a superset (§11).
    fn replace(&mut self, original: &dyn Module, identity: ModuleIdentity, replacement: &'a Replacement, site: Site) -> Import {
        let import = self.enter(replacement.replacement.identity(), &*replacement.replacement, site);
        if let Import::New(node) = import {
            if !self.registry.replaced.iter().any(|r| r.original == identity) {
                let mut scratch = ModuleNode::new(identity.clone());
                original.register(&mut ModuleDef::new(&mut scratch));
                let exports = scratch.exports.iter().map(|export| boundary_key(scratch.keyed, export.key)).collect();
                self.registry.replaced.push(Replaced { original: identity, exports, replacement: node });
            }
        }
        import
    }

    fn enter(&mut self, identity: ModuleIdentity, module: &dyn Module, site: Site) -> Import {
        if let Some(&node) = self.registry.by_identity.get(&identity) {
            if let Some(start) = self.path.iter().position(|&on_path| on_path == node) {
                let mut cycle = self.path[start..].to_vec();
                cycle.push(node);
                self.registry.import_cycles.push(cycle);
            }
            let claimed = &self.registry.nodes[node];
            let found = Import::New(node);
            self.registry.differing.extend(Differing::of(found, (&claimed.settings, claimed.site), (&module.settings(), site)));
            return found;
        }

        // The identity is claimed before `register` runs, so a module importing itself, directly
        // or through others, meets its own node and closes a cycle instead of registering again.
        let index = self.registry.nodes.len();
        let mut node = ModuleNode::new(identity.clone());
        node.settings = module.settings();
        node.site = site;
        // Every module on the path has registered and been pushed already, its `keyed` set, so the
        // nearest keyed one is known here.
        node.inherited_keyed = self.path.iter().rev().find_map(|&ancestor| self.registry.nodes[ancestor].keyed);
        module.register(&mut ModuleDef::new(&mut node));
        let imports = std::mem::take(&mut node.imports);
        self.registry.nodes.push(node);
        self.registry.by_identity.insert(identity, index);
        self.registry.imports.push(Vec::new());

        self.path.push(index);
        for import in imports {
            let reached = self.reach(&*import.module, Site::Import { importer: Import::New(index), at: import.at });
            self.registry.imports[index].push(reached);
        }
        self.path.pop();
        Import::New(index)
    }
}
