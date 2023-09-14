/*!
# Rip Rip Hooray: C2
*/

use crate::{
	CD_C2_SIZE,
	RipRipError,
};



#[derive(Debug, Clone)]
/// # C2 Error Pointers.
///
/// This holds the C2 data for an audio CD sector. Each bit here corresponds to
/// a byte there. Zero is good, one is bad.
///
/// Lest this be too easy, some drives support and/or require a 296-byte
/// variation, which includes a block-wide evaluation as the first or last two
/// trailing bytes. We'll try to detect and discard those since they're
/// redundant.
pub(crate) struct C2([u8; CD_C2_SIZE as usize]);

impl Default for C2 {
	#[inline]
	fn default() -> Self { Self([0; CD_C2_SIZE as usize]) }
}

impl C2 {
	/// # All Good?
	///
	/// Return true if there are no C2 errors.
	pub(crate) fn is_good(&self) -> bool { self.0.iter().all(|v| 0.eq(v)) }

	/// # Make Bad.
	///
	/// Mark all samples as bad to the bone.
	pub(crate) fn make_bad(&mut self) {
		for i in &mut self.0 { *i = 0b1111_1111; }
	}

	/// # Make Good.
	///
	/// Clear all error bits.
	pub(crate) fn make_good(&mut self) {
		for i in &mut self.0 { *i = 0; }
	}

	/// # Sample Errors.
	///
	/// Return an iterator over the samples in the set, noting which ones have
	/// errors.
	pub(crate) const fn sample_errors(&self) -> C2SampleErrors {
		C2SampleErrors {
			set: self,
			pos: 0,
			buf: None,
		}
	}

	/// # Update.
	///
	/// Replace the stored bits with the ones provided, and deal with variable
	/// length weirdness.
	///
	/// If the slice is empty, it will be assumed all bits are good.
	///
	/// This will return true if all samples are good, false if any are bad.
	///
	/// ## Errors
	///
	/// This will return an error if the block size is wrong or unsupported.
	pub(crate) fn update(&mut self, new: &[u8]) -> Result<bool, RipRipError> {
		// If there's no C2 information, we have to assume the data's fine.
		// Likewise if everything's fine, we don't have to think too hard about
		// what to do with it.
		if new.is_empty() {
			self.make_good();
			Ok(true)
		}
		// Copy the bits into place!
		else if new.len() == usize::from(CD_C2_SIZE) {
			self.0.copy_from_slice(new);
			Ok(self.is_good())
		}
		// This shouldn't happen, but return an error if it does so the bug can
		// be fixed.
		else { Err(RipRipError::Bug("Invalid C2 block size!")) }
	}
}



/// # Per-Sample C2.
///
/// This iterator divides up the C2 responses into per-sample states, returning
/// `true` if the sample contains an error, `false` if not.
pub(crate) struct C2SampleErrors<'a> {
	set: &'a C2,
	pos: usize,
	buf: Option<bool>,
}

impl<'a> Iterator for C2SampleErrors<'a> {
	type Item = bool;

	fn next(&mut self) -> Option<Self::Item> {
		// Return the second half of the last byte checked.
		if let Some(next) = self.buf.take() { return Some(next); }

		// Read the next pair.
		let pair: u8 = self.set.0.get(self.pos).copied()?;
		self.pos += 1;

		// Figure out the status of each sample in the pair. Return the first,
		// and move the second to the buffer for next time.
		let next = 0 != pair & 0b1111_0000;
		self.buf.replace(0 != pair & 0b0000_1111);
		Some(next)
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl<'a> ExactSizeIterator for C2SampleErrors<'a> {
	fn len(&self) -> usize {
		// Each byte is 2 samples, so double what's left, then add one for the
		// buffer value, if any.
		usize::from(CD_C2_SIZE).saturating_sub(self.pos) * 2 + usize::from(self.buf.is_some())
	}
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_c2() {
		let mut c2 = C2::default();
		assert!(c2.is_good());
		assert!(c2.sample_errors().all(|v| ! v));

		c2.make_bad();
		assert!(! c2.is_good());
		assert!(c2.sample_errors().all(|v| v));

		c2.make_good();
		assert!(c2.is_good());
		assert!(c2.sample_errors().all(|v| ! v));

		let mut tmp = [0; CD_C2_SIZE as usize];
		assert_eq!(c2.update(&tmp), Ok(true));
		assert!(c2.is_good());
		assert!(c2.sample_errors().all(|v| ! v));

		tmp[3] = 1;
		assert_eq!(c2.update(&tmp), Ok(false));
		assert!(! c2.is_good());
		assert_eq!(c2.sample_errors().filter(|v| *v).count(), 1);

		// Make sure we've got 588 values too.
		assert_eq!(c2.sample_errors().count(), 588);
	}
}
