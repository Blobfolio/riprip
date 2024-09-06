/*!
# Rip Rip Hooray: Read Iterator
*/

use crate::{
	RipRipError,
	RipSample,
	SAMPLES_PER_SECTOR,
};
use std::{
	iter::Rev,
	ops::Range,
	slice::{
		ChunksExactMut,
		RChunksExactMut,
	},
};



#[derive(Debug)]
/// # Offset Rip Iterator.
///
/// This iterator yields the sector LSNs to read from, and the mutable sector
/// slices (offset-adjusted) to write back to.
///
/// This will usually be slightly shorter than the full, padded rip range for
/// the track, as the offset will likely prevent reading and/or writing all the
/// way up to both edges.
///
/// Depending on the settings, it might run start to end, or end to start, but
/// either way ultimately yields the same data.
pub(super) struct OffsetRipIter<'a> {
	/// # Reader.
	read: EitherRangeIter,

	/// # Writer.
	write: EitherChunksIter<'a>,
}

impl<'a> OffsetRipIter<'a> {
	/// # New.
	///
	/// Start up a new iterator given the range, slice, and direction.
	///
	/// ## Errors.
	///
	/// This method contains a lot of pseudo-assertions that will trigger an
	/// error if there's a bug, but since that shouldn't ever happen, it should
	/// be fine. ;)
	pub(super) fn new(lsn: Range<i32>, slice: &'a mut[RipSample], backwards: bool)
	-> Result<Self, RipRipError> {
		// Get the read part going first.
		let read = EitherRangeIter::new(lsn, backwards);

		// Make sure the slice is sector aligned.
		if 0 != slice.len() % usize::from(SAMPLES_PER_SECTOR) {
			return Err(RipRipError::Bug("OffsetRipIter slice length not sector-aligned!"));
		}
		let write = EitherChunksIter::new(slice, backwards);

		// We're good if the lengths match.
		if write.len() == read.len() {
			Ok(Self { read, write })
		}
		// Otherwise it's a bug.
		else {
			Err(RipRipError::Bug("OffsetRipIter lsn and slice have different lengths!"))
		}
	}
}

impl<'a> Iterator for OffsetRipIter<'a> {
	type Item = (i32, &'a mut[RipSample]);

	fn next(&mut self) -> Option<Self::Item> {
		let a = self.read.next()?;
		let b = self.write.next()?;
		Some((a, b))
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl<'a> ExactSizeIterator for OffsetRipIter<'a> {
	fn len(&self) -> usize { self.read.len() }
}



#[derive(Debug)]
/// # Forwards/Backwards Chunks.
///
/// This enum exists solely to allow us to hold either a forwards or backwards
/// mutable chunk iterator under a single variable type. All iterator business
/// is passed straight on through.
enum EitherChunksIter<'a> {
	/// # Forward Reader.
	Forward(ChunksExactMut<'a, RipSample>),

	/// # Backward Reader.
	Backward(RChunksExactMut<'a, RipSample>)
}

impl<'a> EitherChunksIter<'a> {
	/// # New Instance.
	fn new(raw: &'a mut[RipSample], backwards: bool) -> Self {
		if backwards { Self::Backward(raw.rchunks_exact_mut(usize::from(SAMPLES_PER_SECTOR))) }
		else { Self::Forward(raw.chunks_exact_mut(usize::from(SAMPLES_PER_SECTOR))) }
	}
}

impl<'a> Iterator for EitherChunksIter<'a> {
	type Item = &'a mut[RipSample];
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

impl<'a> ExactSizeIterator for EitherChunksIter<'a> {
	fn len(&self) -> usize {
		match self {
			Self::Forward(i) => i.len(),
			Self::Backward(i) => i.len(),
		}
	}
}



#[derive(Debug, Clone)]
/// # Forwards/Backwards Range.
///
/// This enum exists solely to allow us to hold either a forwards or backwards
/// range iterator under a single variable type. All iterator business is
/// passed straight on through.
enum EitherRangeIter {
	/// # Forward Iter.
	Forward(Range<i32>),

	/// # Backward Iter.
	Backward(Rev<Range<i32>>)
}

impl EitherRangeIter {
	/// # New Instance.
	fn new(rng: Range<i32>, backwards: bool) -> Self {
		if backwards { Self::Backward(rng.rev()) }
		else { Self::Forward(rng) }
	}
}

impl Iterator for EitherRangeIter {
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

impl ExactSizeIterator for EitherRangeIter {
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
	fn t_either_range_iter() {
		let a: Vec<i32> = EitherRangeIter::new(5..100, false).collect();
		let mut b: Vec<i32> = EitherRangeIter::new(5..100, true).collect();
		assert_ne!(a, b, "Sets should be in the opposite order!");
		assert!(! a.contains(&100), "The range is supposed to be exclusive.");
		b.reverse();
		assert_eq!(a, b, "Sets should match after reversing one of them!");
	}
}
