#![no_std]
#![feature(type_alias_impl_trait)]
#![feature(try_trait_v2)]

// We always pull in `std` during tests, because it's just easier
// to write tests when you can assume you're on a capable platform
#[cfg(test)]
extern crate std;

pub mod mutex;
pub mod rwlock;

pub mod x_deps {
    pub use abs_sync;
    pub use abs_sync::x_deps::pin_utils;

    pub use atomex;
    pub use atomex::x_deps::funty;
}