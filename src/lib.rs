#![allow(unused_features)]
#![feature(try_trait_v2)]

#![no_std]

// We always pull in `std` during tests, because it's just easier
// to write tests when you can assume you're on a capable platform
#[cfg(test)]
extern crate std;

pub mod mutex;
pub mod rwlock;

pub mod x_deps {
    pub use abs_sync;

    pub use atomex;
    pub use atomex::x_deps::funty;
}
