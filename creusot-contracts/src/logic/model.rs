use crate as creusot_contracts;
use creusot_contracts_proc::*;

pub trait Model {
    type ModelTy;
    #[logic]
    fn model(self) -> Self::ModelTy;
}

impl<T: Model + ?Sized> Model for &T {
    type ModelTy = T::ModelTy;
    #[logic]
    fn model(self) -> Self::ModelTy {
        (*self).model()
    }
}

impl<T: Model + ?Sized> Model for &mut T {
    type ModelTy = T::ModelTy;
    #[logic]
    fn model(self) -> Self::ModelTy {
        (*self).model()
    }
}

impl<T, const N: usize> Model for [T; N] {
    type ModelTy = crate::Seq<T>;

    #[logic]
    #[creusot::builtins = "prelude.Prelude.id"]
    fn model(self) -> Self::ModelTy {
        pearlite! { absurd }
    }
}

impl<A: Model, B: Model> Model for (A, B) {
    type ModelTy = (A::ModelTy, B::ModelTy);

    #[logic]
    fn model(self) -> Self::ModelTy {
        pearlite! {
            (@self.0, @self.1)
        }
    }
}

#[cfg(feature = "num_bigint")]
impl Model for num_bigint::BigInt {
    type ModelTy = crate::Int;
    #[logic]
    #[trusted]
    fn model(self) -> Self::ModelTy {
        pearlite! { absurd }
    }
}
