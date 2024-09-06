/*!
# Rip Rip Hooray: Rip Buffer.
*/

use crate::{
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	KillSwitch,
	LibcdioInstance,
	RipOptions,
	RipRipError,
	Sample,
	SAMPLES_PER_SECTOR,
};
use std::ops::Range;



#[derive(Debug)]
/// # Rip Buffer.
///
/// All sorts of different buffer sizes are needed for different contexts. This
/// struct eliminates a lot of the headache of figuring all that out.
///
/// It is sized to accommodate the biggest dataset — audio + C2 — but gets
/// sub-sliced for smaller reads too. One buffer for all!
pub(crate) struct RipBuffer([u8; CD_DATA_C2_SIZE as usize]);

/// # Setters.
impl RipBuffer {
	#[inline]
	/// # New Instance.
	pub(crate) const fn new() -> Self { Self([0; CD_DATA_C2_SIZE as usize]) }

	#[inline]
	/// # Cache Bust.
	///
	/// See `LibcdioInstance::cache_bust` for the complete rant.
	pub(crate) fn cache_bust(
		&mut self,
		cdio: &LibcdioInstance,
		len: u32,
		rng: &Range<i32>,
		leadout: i32,
		backwards: bool,
		killed: &KillSwitch,
	) {
		cdio.cache_bust(self.data_slice_mut(), len, rng, leadout, backwards, killed);
	}

	/// # Read Sector.
	///
	/// Read a single sector from the disc into the buffer.
	///
	/// Depending on the options, this will fetch some combination of audio
	/// data, C2 error pointers, and subchannel (for timestamp verification).
	///
	/// Returns `true` if no C2 or sync errors were reported.
	///
	/// ## Errors
	///
	/// This will return any I/O related errors encountered, or if timestamp
	/// verification fails, a desync error.
	pub(crate) fn read_sector(&mut self, cdio: &LibcdioInstance, lsn: i32, opts: &RipOptions)
	-> Result<bool, RipRipError> {
		// Subchannel sync?
		if opts.sync() {
			self.read_subchannel(cdio, lsn)?;

			// Hash the data so we can compare it with the C2 version.
			let hash = crc32fast::hash(self.data_slice());

			// Read again with C2 details.
			let good = self.read_c2(cdio, lsn, opts)?;

			// Make sure we got the same data both times.
			if hash == crc32fast::hash(self.data_slice()) { Ok(good) }
			// If not, treat it like a generic read error.
			else { Err(RipRipError::CdRead) }
		}
		// Normal read.
		else { self.read_c2(cdio, lsn, opts) }
	}

	/// # Read C2.
	///
	/// Read the sector with C2 error pointers.
	///
	/// If strict mode is in effect and there are any C2 errors, all samples
	/// will be marked as having an error.
	///
	/// Returns true if no C2 errors were reported.
	fn read_c2(&mut self, cdio: &LibcdioInstance, lsn: i32, opts: &RipOptions)
	-> Result<bool, RipRipError> {
		// Just in case the read is bogus, let's flip all C2 to bad beforehand.
		self.set_bad();

		// Okay, read away!
		cdio.read_cd_c2(&mut self.0, lsn)?;

		// How'd we do?
		let good = self.all_good();

		// If we're in strict mode and there's any error, set all bits
		// to error.
		if opts.strict() && ! good { self.set_bad(); }

		Ok(good)
	}

	/// # Read Subchannel.
	///
	/// Read the sector and verify the subchannel's timecode matches the sector
	/// we're requesting.
	///
	/// In the case of a desync, the data will be added to the state as "bad".
	fn read_subchannel(&mut self, cdio: &LibcdioInstance, lsn: i32)
	-> Result<(), RipRipError> {
		cdio.read_subchannel(
			&mut self.0[..usize::from(CD_DATA_SUBCHANNEL_SIZE)],
			lsn,
		)
	}
}

/// # Getters.
impl RipBuffer {
	/// # Sector Iter.
	///
	/// Return an iterator over the samples and C2 statuses of the last-read
	/// sector.
	pub(crate) const fn samples(&self) -> RipBufferIter {
		RipBufferIter {
			set: &self.0,
			pos: 0,
		}
	}
}

/// # Internal.
impl RipBuffer {
	/// # No C2 Errors?
	///
	/// Returns `true` if all C2 bits are happy and error-free.
	fn all_good(&self) -> bool {
		self.0.iter().skip(usize::from(CD_DATA_SIZE)).all(|v| 0.eq(v))
	}

	/// # C2 Slice Mut.
	///
	/// Return the portion of the buffer containing the C2 error bits.
	fn c2_slice_mut(&mut self) -> &mut [u8] { &mut self.0[usize::from(CD_DATA_SIZE)..] }

	/// # Data Slice.
	///
	/// Return the portion of the buffer containing the audio data.
	fn data_slice(&self) -> &[u8] { &self.0[..usize::from(CD_DATA_SIZE)] }

	/// # Data Slice Mut.
	///
	/// Return the portion of the buffer containing the audio data.
	fn data_slice_mut(&mut self) -> &mut [u8] { &mut self.0[..usize::from(CD_DATA_SIZE)] }

	#[inline]
	/// # Mark All C2 Bad.
	fn set_bad(&mut self) {
		for v in self.c2_slice_mut() { *v = 0b1111_1111; }
	}
}



/// # Per Sample Iterator.
///
/// This returns each individual sample (pair) along with its C2 error status
/// (`false` for good, `true` for bad). Since the data range covers one sector,
/// this will always produce exactly `588` results.
pub(crate) struct RipBufferIter<'a> {
	/// # Samples.
	set: &'a [u8; CD_DATA_C2_SIZE as usize],

	/// # Current Index.
	pos: usize,
}

impl<'a> Iterator for RipBufferIter<'a> {
	type Item = (Sample, bool);

	fn next(&mut self) -> Option<Self::Item> {
		if self.pos < usize::from(SAMPLES_PER_SECTOR) {
			// Samples are at the beginning. It is tempting to unsafely recast
			// the slice to an array because we know it's the right length, but
			// -O3 figures that out, so it doesn't make any difference.
			let pos = self.pos * 4;
			let sample: Sample = self.set[pos..pos + 4].try_into().ok()?;

			// C2 is at the end, and stored in half-bytes, so that's fun.
			let pos = usize::from(CD_DATA_SIZE) + self.pos.wrapping_div(2);
			let c2_err =
				// Even indexes get the first half. As with the sample part,
				// `get_unchecked` would be tempting, but -O3 removes the
				// panic-able checks for us.
				if 0 == self.pos & 1 { 0 != self.set[pos] & 0b1111_0000 }
				// Odds the second.
				else { 0 != self.set[pos] & 0b0000_1111 };

			// Increment for next time.
			self.pos += 1;

			// Return what we got this time.
			Some((sample, c2_err))
		}
		else { None }
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl<'a> ExactSizeIterator for RipBufferIter<'a> {
	fn len(&self) -> usize {
		usize::from(SAMPLES_PER_SECTOR).saturating_sub(self.pos)
	}
}



#[cfg(test)]
mod test {
	use super::*;
	use crate::NULL_SAMPLE;

	#[test]
	fn t_buf_iters() {
		let mut buf = RipBuffer::new();
		buf.0[4..8].copy_from_slice(&[1, 1, 1, 1]);
		buf.0[usize::from(CD_DATA_SIZE)] = 0b0000_1111;
		buf.0[usize::from(CD_DATA_SIZE) + 1] = 0b1111_1111;
		buf.0[usize::from(CD_DATA_SIZE) + 2] = 0b1111_0000;

		// Test the goodness.
		assert!(! buf.all_good());

		// Make sure our manually-set values turn up at the right place.
		let mut iter = buf.samples();
		assert_eq!(iter.next(), Some((NULL_SAMPLE, false)));
		assert_eq!(iter.next(), Some(([1, 1, 1, 1], true)));
		assert_eq!(iter.next(), Some((NULL_SAMPLE, true)));
		assert_eq!(iter.next(), Some((NULL_SAMPLE, true)));
		assert_eq!(iter.next(), Some((NULL_SAMPLE, true)));

		// And that the total length winds up being 588.
		for _ in 5..usize::from(SAMPLES_PER_SECTOR) {
			assert_eq!(iter.next(), Some((NULL_SAMPLE, false)));
		}
		assert!(iter.next().is_none());

		// Make sure goodness/badness works.
		for v in &mut buf.0 { *v = 0; }
		assert!(buf.all_good());
		assert!(buf.samples().all(|(_, err)| ! err), "Missing goodness!");

		buf.set_bad();
		assert!(! buf.all_good());
		assert!(buf.samples().all(|(_, err)| err), "Missing error!");
	}
}
