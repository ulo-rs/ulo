//! A synchronous type-keyed map.
//!
//! Storage for things built once and read many times — a handler's declared [`Metadata`], an RPC
//! call's descriptor. Per-execution state that enhancers write and handlers read is
//! [`Extensions`](super::Extensions) instead, which is shared by handle and mutable through it.
//!
//! One user: [`Metadata`], which is built once per declaration site and
//! read on every call through it.
//!
//! # Implementation Note
//!
//! This implementation is based on the `http` crate's Extensions type
//! (<https://docs.rs/http/1.3.1/http/struct.Extensions.html>).
//!

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::fmt;
use std::hash::{BuildHasherDefault, Hasher};

type AnyMap = HashMap<TypeId, Box<dyn AnyClone + Send + Sync>, BuildHasherDefault<IdHasher>>;

// With TypeIds as keys, there's no need to hash them. They are already hashes
// themselves, coming from the compiler. The IdHasher just holds the u64 of
// the TypeId, and then returns it, instead of doing any bit fiddling.
#[derive(Default)]
struct IdHasher(u64);

impl Hasher for IdHasher {
    fn write(&mut self, _: &[u8]) {
        unreachable!("TypeId calls write_u64");
    }

    #[inline]
    fn write_u64(&mut self, id: u64) {
        self.0 = id;
    }

    #[inline]
    fn finish(&self) -> u64 {
        self.0
    }
}

/// A type map for storing execution-scoped data.
///
/// This allows middleware to pass typed data to controllers and services
/// without coupling them to HTTP types.
///
/// Values stored in `Extensions` must implement `Clone + Send + Sync + 'static`.
#[derive(Clone, Default)]
pub(crate) struct TypeMap {
    map: AnyMap,
}

impl TypeMap {
    /// Insert a value into the map.
    ///
    /// If a value of this type already existed, it will be returned.
    ///
    pub(crate) fn insert<T: Clone + Send + Sync + 'static>(&mut self, val: T) -> Option<T> {
        self.map
            .insert(TypeId::of::<T>(), Box::new(val))
            .and_then(|boxed| boxed.into_any().downcast().ok().map(|boxed| *boxed))
    }

    /// Get a reference to a value previously inserted.
    ///
    pub(crate) fn get<T: Send + Sync + 'static>(&self) -> Option<&T> {
        self.map
            .get(&TypeId::of::<T>())
            .and_then(|boxed| (**boxed).as_any().downcast_ref())
    }

    /// Get a mutable reference to a value previously inserted.
    ///
    pub(crate) fn get_mut<T: Send + Sync + 'static>(&mut self) -> Option<&mut T> {
        self.map
            .get_mut(&TypeId::of::<T>())
            .and_then(|boxed| (**boxed).as_any_mut().downcast_mut())
    }

    /// Check if the map is empty.
    ///
    #[inline]
    pub(crate) fn is_empty(&self) -> bool {
        self.map.is_empty()
    }
}

impl fmt::Debug for TypeMap {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TypeMap").finish()
    }
}

// Internal trait to enable cloning of trait objects
trait AnyClone: Any {
    fn clone_box(&self) -> Box<dyn AnyClone + Send + Sync>;
    fn as_any(&self) -> &dyn Any;
    fn as_any_mut(&mut self) -> &mut dyn Any;
    fn into_any(self: Box<Self>) -> Box<dyn Any>;
}

impl<T: Clone + Send + Sync + 'static> AnyClone for T {
    fn clone_box(&self) -> Box<dyn AnyClone + Send + Sync> {
        Box::new(self.clone())
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn as_any_mut(&mut self) -> &mut dyn Any {
        self
    }

    fn into_any(self: Box<Self>) -> Box<dyn Any> {
        self
    }
}

impl Clone for Box<dyn AnyClone + Send + Sync> {
    fn clone(&self) -> Self {
        (**self).clone_box()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Debug, PartialEq)]
    struct MyType(i32);

    #[test]
    fn test_extensions() {
        let mut extensions = TypeMap::default();

        extensions.insert(5i32);
        extensions.insert(MyType(10));

        assert_eq!(extensions.get(), Some(&5i32));
        assert_eq!(extensions.get_mut(), Some(&mut 5i32));

        // A clone carries the values, rather than starting empty.
        let ext2 = extensions.clone();
        assert_eq!(ext2.get(), Some(&5i32));
        assert_eq!(ext2.get(), Some(&MyType(10)));

        assert_eq!(extensions.get::<bool>(), None);
        assert!(!extensions.is_empty());
    }

    #[test]
    fn insert_answers_the_value_it_replaced() {
        let mut ext = TypeMap::default();
        assert!(ext.insert(MyType(1)).is_none());
        assert_eq!(ext.insert(MyType(2)), Some(MyType(1)));
        assert_eq!(ext.get(), Some(&MyType(2)));
    }

    #[test]
    fn get_mut_writes_through_to_the_stored_value() {
        let mut ext = TypeMap::default();
        ext.insert(MyType(5));
        ext.get_mut::<MyType>().unwrap().0 += 10;
        assert_eq!(ext.get(), Some(&MyType(15)));
    }

    #[test]
    fn a_fresh_map_is_empty() {
        assert!(TypeMap::default().is_empty());
    }
}
