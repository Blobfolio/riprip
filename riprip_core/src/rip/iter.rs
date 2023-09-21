/*!
# Rip Rip Hooray: Read Iterator
*/

use std::{
	iter::Rev,
	ops::Range,
};



#[derive(Debug, Clone)]
/// # Read Iterator.
///
/// This is just a simple wrapper that allows treat forward and backward `Range`
/// iterators as the same thing without Rust complaining about them being
/// different types.
///
/// Iteration-wise, this simply passes through the inner values.
pub(super) enum ReadIter {
	Forward(Range<i32>),
	Backward(Rev<Range<i32>>)
}

impl ReadIter {
	/// # New Instance.
	///
	/// Generate the right kind of iterator based on the value of `backwards`.
	pub(super) fn new(start: i32, end: i32, backwards: bool) -> Self {
		if backwards { Self::Backward((start..end).rev()) }
		else { Self::Forward(start..end) }
	}
}

impl Iterator for ReadIter {
	type Item = i32;
	fn next(&mut self) -> Option<Self::Item> {
		match self {
			Self::Forward(i) => i.next(),
			Self::Backward(i) => i.next(),
		}
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl ExactSizeIterator for ReadIter {
	fn len(&self) -> usize {
		match self {
			Self::Forward(i) => i.len(),
			Self::Backward(i) => i.len(),
		}
	}
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_read_iter() {
		let a: Vec<i32> = ReadIter::new(5, 100, false).collect();
		let mut b: Vec<i32> = ReadIter::new(5, 100, true).collect();
		assert_ne!(a, b, "Sets should be in the opposite order!");
		assert!(! a.contains(&100), "The range is supposed to be exclusive.");
		b.reverse();
		assert_eq!(a, b, "Sets should match after reversing one of them!");
	}
}
