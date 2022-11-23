mod filter;
mod find_unique;
mod replicate;

use common_base::BitVec;

use crate::error::Result;
// use crate::types::PrimitiveElement;
use crate::vectors::{
    VectorRef, Vector, BinaryVector, BooleanVector
};

/// Vector compute operations.
pub trait VectorOp {
    /// Copies each element according `offsets` parameter.
    /// - `i-th` element should be copied `offsets[i] - offsets[i - 1]` times
    /// - `0-th` element would be copied `offsets[0]` times
    ///
    /// # Panics
    /// Panics if `offsets.len() != self.len()`.
    fn replicate(&self, offsets: &[usize]) -> VectorRef;

    /// Mark `i-th` bit of `selected` to `true` if the `i-th` element of `self` is unique, which
    /// means there is no elements behind it have same value as it.
    ///
    /// The caller should ensure
    /// 1. the length of `selected` bitmap is equal to `vector.len()`.
    /// 2. `vector` and `prev_vector` are sorted.
    ///
    /// If there are multiple duplicate elements, this function retains the **first** element.
    /// The first element is considered as unique if the first element of `self` is different
    /// from its previous element, that is the last element of `prev_vector`.
    ///
    /// # Panics
    /// Panics if
    /// - `selected.len() < self.len()`.
    /// - `prev_vector` and `self` have different data types.
    fn find_unique(&self, selected: &mut BitVec, prev_vector: Option<&dyn Vector>);

    /// Filters the vector, returns elements matching the `filter` (i.e. where the values are true).
    ///
    /// Note that the nulls of `filter` are interpreted as `false` will lead to these elements being masked out.
    fn filter(&self, filter: &BooleanVector) -> Result<VectorRef>;
}

macro_rules! impl_scalar_vector_op {
    ($( { $VectorType: ident, $replicate: ident } ),+) => {$(
        impl VectorOp for $VectorType {
            fn replicate(&self, offsets: &[usize]) -> VectorRef {
                replicate::$replicate(self, offsets)
            }

            fn find_unique(&self, selected: &mut BitVec, prev_vector: Option<&dyn Vector>) {
                let prev_vector = prev_vector.map(|pv| pv.as_any().downcast_ref::<$VectorType>().unwrap());
                find_unique::find_unique_scalar(self, selected, prev_vector);
            }

            fn filter(&self, filter: &BooleanVector) -> Result<VectorRef> {
                filter::filter_non_constant!(self, $VectorType, filter)
            }
        }
    )+};
}

impl_scalar_vector_op!(
    { BinaryVector, replicate_scalar },
    { BooleanVector, replicate_scalar }
    // { ListVector, replicate_scalar },
    // { StringVector, replicate_scalar },
    // { DateVector, replicate_date },
    // { DateTimeVector, replicate_datetime },
    // { TimestampVector, replicate_timestamp }
);
