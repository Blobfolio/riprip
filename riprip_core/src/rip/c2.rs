/*!
# Rip Rip Hooray: C2
*/

use crate::{
	CD_C2_SIZE,
	CD_C2B_SIZE,
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
		if new.is_empty() || new.iter().all(|v| 0.eq(v)) {
			self.make_good();
			return Ok(true);
		}

		// If the 296-byte block mode is used, we have to figure out which side
		// that damn block bit is stored on. It's supposed to be on the left,
		// but some drives place it on the end instead.
		if new.len() == usize::from(CD_C2B_SIZE) {
			// See if the first two and/or last two bytes contain non-zero
			// data. One of them is supposed to be representative of the sector
			// as a whole, so one of them should be non-zero given the fact c2
			// errors were returned.
			let lhs = new[0] != 0 || new[1] != 0;
			let rhs = new[usize::from(CD_C2B_SIZE) - 1] != 0 || new[usize::from(CD_C2B_SIZE) - 2] != 0;

			// If both are non-zero, we can't really know which is which, so
			// let's just treat the entire block as bad.
			if lhs && rhs {
				self.make_bad();
				return Ok(false);
			}

			// If both are zero, that can't be right! The drive likely doesn't
			// support this c2 block size.
			if ! lhs && ! rhs {
				self.make_bad();
				return Err(RipRipError::C2Mode296);
			}

			// Chop off whichever two bytes have values, and copy the rest into
			// place.
			if lhs { self.0.copy_from_slice(&new[2..]); }
			else   { self.0.copy_from_slice(&new[..usize::from(CD_C2B_SIZE) - 2]); }
		}
		// The 294-byte block mode can be copied straight!
		else if new.len() == usize::from(CD_C2_SIZE) { self.0.copy_from_slice(new); }
		// If for some reason we accidentally passed a different slice size in,
		// return an error so the bug can be found and fixed.
		else {
			return Err(RipRipError::Bug("Invalid C2 block size!"));
		}

		// Done!
		Ok(false)
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
