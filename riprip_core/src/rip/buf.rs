/*!
# Rip Rip Hooray: Rip Buffer.
*/

use crate::{
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	LibcdioInstance,
	RipRipError,
	Sample,
	SAMPLES_PER_SECTOR,
};

const FLAG_C2: u8 =          0b0000_0001; // Read C2.
const FLAG_C2_STRICT: u8 =   0b0000_0011; // Treat C2 errors per-sector.
const FLAG_SYNC: u8 =        0b0000_0100; // Read subchannel (sync).
const FLAG_SYNC_STRICT: u8 = 0b0000_1100; // Read subchannel (sync).
const FLAG_DESYNC: u8 =      0b0001_0000; // Read was desynced, or Q was bad.



#[derive(Debug)]
/// # Rip Buffer.
///
/// All sorts of different buffer sizes are needed for different contexts. This
/// struct eliminates a lot of the headache of figuring all that out.
pub(crate) struct RipBuffer {
	buf: [u8; CD_DATA_C2_SIZE as usize],
	flags: u8,
}

impl Default for RipBuffer {
	fn default() -> Self {
		Self {
			buf: [0; CD_DATA_C2_SIZE as usize],
			flags: 0,
		}
	}
}

impl RipBuffer {
	/// # With C2.
	///
	/// Toggle C2 support. If `strict` and `c2`, C2 errors will be treated as
	/// sector-level problems rather than per-sample ones.
	pub(crate) const fn with_c2(self, c2: bool, strict: bool) -> Self {
		let flags =
			if c2 {
				if strict { self.flags | FLAG_C2_STRICT }
				else {
					let flags = self.flags & ! FLAG_C2_STRICT;
					flags | FLAG_C2
				}
			}
			else { self.flags & ! FLAG_C2_STRICT };

		Self {
			buf: self.buf,
			flags,
		}
	}

	/// # With Subchannel.
	///
	/// Toggle subchannel support (for timecode syncing). If `strict`, a desync
	/// will flip all C2 bits bad for the sector.
	pub(crate) const fn with_sync(self, sync: bool, strict: bool) -> Self {
		let flags =
			if sync {
				if strict { self.flags | FLAG_SYNC_STRICT }
				else {
					let flags = self.flags & ! FLAG_SYNC_STRICT;
					flags | FLAG_SYNC
				}
			}
			else { self.flags & ! FLAG_SYNC_STRICT };

		Self {
			buf: self.buf,
			flags,
		}
	}
}

/// # Setters.
impl RipBuffer {
	/// # Read Sector.
	///
	/// Read a single sector from the disc into the buffer.
	///
	/// Depending on the options, this will fetch some combination of audio
	/// data, C2 error pointers, and subchannel (for timestamp verification).
	///
	/// ## Errors
	///
	/// This will return any I/O related errors encountered, or if timestamp
	/// verification fails, a desync error.
	pub(crate) fn read_sector(&mut self, cdio: &LibcdioInstance, lsn: i32)
	-> Result<(), RipRipError> {
		// Reading C2.
		if self.has_flag(FLAG_C2) {
			// Subchannel?
			if self.has_flag(FLAG_SYNC) {
				// Read and verify timecode.
				self.read_subchannel(cdio, lsn)?;

				// If strict and we failed, we don't need to reread the C2.
				if self.has_flag(FLAG_SYNC_STRICT | FLAG_DESYNC) {
					return Ok(());
				}

				// Otherwise hash the data so we can make sure we're
				let hash = crc32fast::hash(self.data_slice());

				// Read again with C2 details.
				self.read_c2(cdio, lsn)?;

				// Make sure we got the same data both times.
				if hash == crc32fast::hash(self.data_slice()) { Ok(()) }
				// If not, treat it like a generic read error.
				else { Err(RipRipError::CdRead(lsn)) }
			}
			// Data + C2.
			else { self.read_c2(cdio, lsn) }
		}
		// No C2, but subchannel?
		else if self.has_flag(FLAG_SYNC) {
			self.read_subchannel(cdio, lsn)?;

			// Make sure our C2 bits are all set to good since we aren't
			// requesting that data, unless we purposefully failed them.
			if ! self.has_flag(FLAG_SYNC_STRICT | FLAG_DESYNC) {
				self.set_c2_good();
			}

			Ok(())
		}
		// Just the data!
		else {
			// Make sure our C2 bits are all set to good since we aren't
			// requesting that data.
			self.set_c2_good();

			// Read the data.
			cdio.read_cd(self.data_slice_mut(), lsn)
		}
	}

	/// # Read C2.
	///
	/// Read the sector with C2 error pointers.
	///
	/// If strict mode is in effect and there are any C2 errors, all samples
	/// will be marked as having an error.
	fn read_c2(&mut self, cdio: &LibcdioInstance, lsn: i32)
	-> Result<(), RipRipError> {
		cdio.read_cd(&mut self.buf, lsn)?;

		// If we're in strict mode and there's any error, set all bits
		// to error.
		if self.has_flag(FLAG_C2_STRICT) && ! self.is_c2_good() { self.set_c2_bad(); }

		Ok(())
	}

	/// # Read Subchannel.
	///
	/// Read the sector and verify the subchannel's timecode matches the sector
	/// we're requesting.
	///
	/// In the case of a desync, the data will be added to the state as "bad".
	fn read_subchannel(&mut self, cdio: &LibcdioInstance, lsn: i32)
	-> Result<(), RipRipError> {
		// Unset desync.
		self.flags &= ! FLAG_DESYNC;

		let res = cdio.read_subchannel(
			&mut self.buf[..usize::from(CD_DATA_SUBCHANNEL_SIZE)],
			lsn,
		);

		// If the read desynced, set the flag. If strict, flip all C2 to error
		// before returning.
		if matches!(res, Err(RipRipError::SubchannelDesync)) {
			self.flags |= FLAG_DESYNC;
			if self.has_flag(FLAG_SYNC_STRICT) {
				self.set_c2_bad();
			}
			return Ok(());
		}

		// Otherwise return whatever we got.
		res
	}

	/// # Mark All C2 Bad.
	fn set_c2_bad(&mut self) {
		for v in self.c2_slice_mut() { *v = 0b1111_1111; }
	}

	/// # Mark All C2 Good.
	fn set_c2_good(&mut self) {
		for v in self.c2_slice_mut() { *v = 0; }
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
			set: &self.buf,
			pos: 0,
		}
	}

	/// # Sync Error?
	///
	/// Returns `true` if the last read had a sync error.
	pub(crate) const fn sync_error(&self) -> bool { self.has_flag(FLAG_DESYNC) }
}

/// # Internal.
impl RipBuffer {
	/// # C2 Slice Mut.
	///
	/// Return the portion of the buffer containing the C2 error bits.
	fn c2_slice_mut(&mut self) -> &mut [u8] { &mut self.buf[usize::from(CD_DATA_SIZE)..] }

	/// # Data Slice.
	///
	/// Return the portion of the buffer containing the audio data.
	fn data_slice(&self) -> &[u8] { &self.buf[..usize::from(CD_DATA_SIZE)] }

	/// # Data Slice.
	///
	/// Return the portion of the buffer containing the audio data.
	fn data_slice_mut(&mut self) -> &mut [u8] { &mut self.buf[..usize::from(CD_DATA_SIZE)] }

	/// # No C2 Errors?
	///
	/// Returns `true` if all C2 bits are happy and error-free.
	fn is_c2_good(&self) -> bool {
		self.buf.iter().skip(usize::from(CD_DATA_SIZE)).all(|v| 0.eq(v))
	}

	/// # Has Flag?
	///
	/// Return true if the flag is set.
	const fn has_flag(&self, flag: u8) -> bool { flag == self.flags & flag }
}



/// # Per Sample Iterator.
pub(crate) struct RipBufferIter<'a> {
	set: &'a [u8; CD_DATA_C2_SIZE as usize],
	pos: usize,
}

impl<'a> Iterator for RipBufferIter<'a> {
	type Item = (Sample, bool);

	fn next(&mut self) -> Option<Self::Item> {
		if self.pos < usize::from(SAMPLES_PER_SECTOR) {
			// Samples are at the beginning.
			let pos = self.pos * 4;
			let sample: Sample = self.set[pos..pos + 4].try_into().ok()?;

			// C2 is at the end, and stored in half-bytes, so that's fun.
			let pos = usize::from(CD_DATA_SIZE) + self.pos.wrapping_div(2);
			let c2_err =
				// Even indexes get the first half.
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
		let mut buf = RipBuffer::default();
		buf.buf[4..8].copy_from_slice(&[1, 1, 1, 1]);
		buf.buf[usize::from(CD_DATA_SIZE)] = 0b0000_1111;
		buf.buf[usize::from(CD_DATA_SIZE) + 1] = 0b1111_1111;
		buf.buf[usize::from(CD_DATA_SIZE) + 2] = 0b1111_0000;

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
	}
}
