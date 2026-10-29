use std::iter::Sum;
use std::ops::{Add, AddAssign};

use serde::{Deserialize, Serialize};

/// How much space an item takes, and how much removing it would free, in bytes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Size {
    /// The sum of file lengths.
    pub logical: u64,
    /// Space allocated on disk. It can be smaller than `logical` (compressed, sparse, or cloud
    /// placeholder files) or larger (rounding up to whole clusters).
    pub on_disk: u64,
    /// The part of `on_disk` that removing the item is certain to free. Data shared with files
    /// outside the item, through hard links or copy-on-write clones, isn't counted.
    pub guaranteed: u64,
}

impl Add for Size {
    type Output = Self;

    fn add(self, other: Self) -> Self {
        Self {
            logical: self.logical + other.logical,
            on_disk: self.on_disk + other.on_disk,
            guaranteed: self.guaranteed + other.guaranteed,
        }
    }
}

impl AddAssign for Size {
    fn add_assign(&mut self, other: Self) {
        *self = *self + other;
    }
}

impl Sum for Size {
    fn sum<I: Iterator<Item = Self>>(sizes: I) -> Self {
        sizes.fold(Self::default(), Add::add)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sizes_add_field_by_field() {
        let sizes = [
            Size {
                logical: 10,
                on_disk: 4096,
                guaranteed: 4096,
            },
            Size {
                logical: 5,
                on_disk: 0,
                guaranteed: 0,
            },
        ];

        assert_eq!(
            sizes.into_iter().sum::<Size>(),
            Size {
                logical: 15,
                on_disk: 4096,
                guaranteed: 4096,
            }
        );
    }
}
