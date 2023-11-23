#![warn(rustdoc::broken_intra_doc_links)]
#![warn(missing_docs)]
#![doc = include_str!("../README.md")]

mod box_scope;
pub mod counterexamples;
mod scope;
/// From <https://blog.aloni.org/posts/a-stack-less-rust-coroutine-100-loc/>, originally from
/// [genawaiter](https://lib.rs/crates/genawaiter).
mod waker;

pub use box_scope::BoxScope;

use std::{cell::Cell, future::Future, marker::PhantomData, task::Poll};

/// A type for functions that never return.
///
/// Since this enum has no variant, a value of this type can never actually exist.
/// This type is similar to [`std::convert::Infallible`] and used as a technicality to ensure that
/// functions passed to [`BoxScope::new`] never return.
///
/// ## Future compatibility
///
/// Should the [the `!` “never” type][never] ever be stabilized, this type would become a type alias and
/// eventually be deprecated. See [the relevant section](std::convert::Infallible#future-compatibility)
/// for more information.
pub enum Never {}

/// Describes a family of types containing a lifetime.
///
/// This type is typically implemented on a helper type to describe the lifetime of the borrowed data we want to freeze in time.
/// See [the module documentation](self) for more information.
pub trait Family<'a> {
    /// An instance with lifetime `'a` of the borrowed data.
    type Family: 'a;
}

/// The future resulting from using a time capsule to freeze some scope.
pub struct FrozenFuture<'a, 'b, T>
where
    T: for<'c> Family<'c>,
    'b: 'a,
{
    mut_ref: Cell<Option<&'a mut <T as Family<'b>>::Family>>,
    state: *const State<T>,
}

struct State<T>(Cell<*mut <T as Family<'static>>::Family>)
where
    T: for<'a> Family<'a>;

impl<T> Default for State<T>
where
    T: for<'a> Family<'a>,
{
    fn default() -> Self {
        Self(Cell::new(std::ptr::null_mut()))
    }
}

/// Passed to the closures of a scope so that they can freeze the scope.
pub struct TimeCapsule<T>
where
    T: for<'a> Family<'a>,
{
    state: *const State<T>,
}

impl<T> TimeCapsule<T>
where
    T: for<'a> Family<'a>,
{
    /// Freeze a scope, making the data it has borrowed available to the outside.
    ///
    /// Once a scope is frozen, its borrowed data can be accessed through [`BoxScope::enter`].
    pub fn freeze<'a, 'b>(
        &'a mut self,
        t: &'a mut <T as Family<'b>>::Family,
    ) -> FrozenFuture<'a, 'b, T>
    where
        'b: 'a,
    {
        FrozenFuture {
            mut_ref: Cell::new(Some(t)),
            state: self.state,
        }
    }
}

impl<'a, 'b, T> Future for FrozenFuture<'a, 'b, T>
where
    T: for<'c> Family<'c>,
{
    type Output = ();

    fn poll(
        self: std::pin::Pin<&mut Self>,
        _cx: &mut std::task::Context<'_>,
    ) -> Poll<Self::Output> {
        // SAFETY: `state` has been set in the future by the scope
        let state = unsafe { self.state.as_ref().unwrap() };
        if state.0.get().is_null() {
            let mut_ref = self
                .mut_ref
                .take()
                .expect("poll called several times on the same future");
            let mut_ref: *mut <T as Family>::Family = mut_ref;
            // SAFETY: Will be given back a reasonable lifetime in the `enter` method.
            let mut_ref: *mut <T as Family<'static>>::Family = mut_ref.cast();

            state.0.set(mut_ref);
            Poll::Pending
        } else {
            state.0.set(std::ptr::null_mut());
            Poll::Ready(())
        }
    }
}

/// Helper type for static types.
///
/// Types that don't contain a lifetime are `'static`, and have one obvious family.
///
/// The usefulness of using `'static` types in the scopes of this crate is dubious, but should you want to do this,
/// for any `T : 'static` pass a `TimeCapsule<SingleFamily<T>>` to your async function.
struct SingleFamily<T: 'static>(PhantomData<T>);
impl<'a, T: 'static> Family<'a> for SingleFamily<T> {
    type Family = T;
}

#[cfg(test)]
mod test {
    use super::*;
    #[test]
    fn produce_output() {
        let mut scope = BoxScope::new(
            |mut time_capsule: TimeCapsule<SingleFamily<u32>>| async move {
                let mut x = 0u32;
                loop {
                    time_capsule.freeze(&mut x).await;
                    x += 1;
                }
            },
        );

        assert_eq!(scope.enter(|x| *x + 42), 42);
        assert_eq!(scope.enter(|x| *x + 42), 43);
        scope.enter(|x| *x += 100);
        assert_eq!(scope.enter(|x| *x + 42), 145);
    }

    #[test]
    fn panicking_future() {
        let mut scope = BoxScope::new(|_: TimeCapsule<SingleFamily<u32>>| async move { panic!() });

        assert!(matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.enter(|x| println!("{x}"))
            })),
            Err(_)
        ));

        assert!(matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.enter(|x| println!("{x}"))
            })),
            Err(_)
        ));
    }

    #[test]
    fn panicking_future_after_once() {
        let mut scope = BoxScope::new(
            |mut time_capsule: TimeCapsule<SingleFamily<u32>>| async move {
                let mut x = 0u32;
                time_capsule.freeze(&mut x).await;
                panic!()
            },
        );

        scope.enter(|x| println!("{x}"));

        assert!(matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.enter(|x| println!("{x}"))
            })),
            Err(_)
        ));

        assert!(matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.enter(|x| println!("{x}"))
            })),
            Err(_)
        ))
    }

    #[test]
    fn panicking_enter() {
        let mut scope = BoxScope::new(
            |mut time_capsule: TimeCapsule<SingleFamily<u32>>| async move {
                let mut x = 0u32;
                loop {
                    time_capsule.freeze(&mut x).await;
                    x += 1;
                }
            },
        );

        scope.enter(|x| assert_eq!(*x, 0));

        assert!(matches!(
            std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                scope.enter(|_| panic!())
            })),
            Err(_)
        ));

        // '1' skipped due to panic
        scope.enter(|x| assert_eq!(*x, 2));
    }
}
