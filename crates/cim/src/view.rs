//! Typed views over stored objects.
//!
//! A view is a borrowed, zero-cost wrapper that gives schema-checked access to one
//! class's attributes. Generated views live in each vintage's `views` module; this
//! module holds the machinery they share.

use std::marker::PhantomData;

use crate::dataset::{Dataset, ObjectId};
use crate::mrid::Mrid;
use crate::object::Object;
use crate::schema::ClassId;

/// A typed view over an [`Object`] of a known class.
pub trait TypedView<'a>: Sized + Copy {
    /// The class this view represents.
    const CLASS: ClassId;

    /// Wrap an object without checking its class.
    fn from_object(object: &'a Object) -> Self;

    /// The underlying object.
    fn object(&self) -> &'a Object;

    /// Wrap an object only if its class matches, including subclasses.
    fn try_from_object(schema: &crate::schema::Schema, object: &'a Object) -> Option<Self> {
        schema
            .is_a(object.class(), Self::CLASS)
            .then(|| Self::from_object(object))
    }

    /// The object's master resource identifier.
    fn mrid(&self) -> &'a Mrid {
        self.object().mrid()
    }
}

/// A reference to an object of a known class, stored as its mRID.
///
/// CIM associations are serialized as identifiers, and exactly one side of each
/// association is written, so references are resolved through the dataset rather than
/// stored as pointers. `TypedRef` keeps the target class in the type so that resolving
/// it yields the right view without a cast.
#[derive(Clone, PartialEq, Eq, Hash)]
pub struct TypedRef<T> {
    mrid: Mrid,
    _marker: PhantomData<fn() -> T>,
}

impl<T> TypedRef<T> {
    pub fn new(mrid: Mrid) -> Self {
        TypedRef {
            mrid,
            _marker: PhantomData,
        }
    }

    /// The referenced identifier.
    pub fn mrid(&self) -> &Mrid {
        &self.mrid
    }

    pub fn into_mrid(self) -> Mrid {
        self.mrid
    }
}

impl<T> std::fmt::Debug for TypedRef<T> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "TypedRef({})", self.mrid)
    }
}

impl<'a, T: TypedView<'a>> TypedRef<T> {
    /// Resolve through `dataset`, returning `None` if the target is absent.
    pub fn get(&self, dataset: &'a Dataset) -> Option<T> {
        dataset.by_mrid(&self.mrid).map(T::from_object)
    }

    /// Resolve, checking that the target really is of the expected class.
    pub fn get_checked(&self, dataset: &'a Dataset) -> Option<T> {
        let o = dataset.by_mrid(&self.mrid)?;
        T::try_from_object(dataset.schema(), o)
    }

    /// Whether the target is present in `dataset`.
    pub fn is_resolvable(&self, dataset: &Dataset) -> bool {
        dataset.find(&self.mrid).is_some()
    }
}

/// Iterate every object of a class as a typed view, subclasses included.
pub fn iter_view<'a, T: TypedView<'a>>(dataset: &'a Dataset) -> impl Iterator<Item = T> + 'a {
    dataset.iter_class(T::CLASS).map(|(_, o)| T::from_object(o))
}

/// Iterate every object of a class with its handle.
pub fn iter_view_ids<'a, T: TypedView<'a>>(
    dataset: &'a Dataset,
) -> impl Iterator<Item = (ObjectId, T)> + 'a {
    dataset
        .iter_class(T::CLASS)
        .map(|(id, o)| (id, T::from_object(o)))
}

/// Look up one object by mRID as a typed view.
pub fn view_by_mrid<'a, T: TypedView<'a>>(dataset: &'a Dataset, mrid: &Mrid) -> Option<T> {
    let o = dataset.by_mrid(mrid)?;
    T::try_from_object(dataset.schema(), o)
}

impl Dataset {
    /// Every object of `T`'s class, including subclasses, as typed views.
    ///
    /// ```
    /// use cim::prelude::*;
    /// use cim::cgmes3::{SCHEMA, views::ACLineSegment};
    ///
    /// let dataset = Dataset::new(SCHEMA);
    /// for line in dataset.view::<ACLineSegment>() {
    ///     println!("{:?} r={:?}", line.name(), line.r());
    /// }
    /// ```
    pub fn view<'a, T: TypedView<'a>>(&'a self) -> impl Iterator<Item = T> + 'a {
        iter_view::<T>(self)
    }

    /// Look up one object by mRID as a typed view, checking its class.
    pub fn view_by_mrid<'a, T: TypedView<'a>>(&'a self, mrid: &Mrid) -> Option<T> {
        view_by_mrid::<T>(self, mrid)
    }

    /// Count objects of `T`'s class.
    pub fn count_view<'a, T: TypedView<'a>>(&'a self) -> usize {
        self.count_class(T::CLASS)
    }
}
