/*!
# Rip Rip Hooray: Rip Buffer.
*/

use crate::{
	LibcdioInstance,
	CD_C2_SIZE,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_DATA_SUBCHANNEL_SIZE,
	RipRipError,
};
use std::slice::ChunksExact;

const FLAG_C2: u8 =         0b0001;
const FLAG_C2_STRICT: u8 =  0b0011;
const FLAG_SUBCHANNEL: u8 = 0b0100;

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
	/// Leverage C2 error pointer information when reading data from the disc.
	/// If `strict`, a C2 error for one sample acts like a C2 error for all
	/// (i.e. the whole sector is considered bad).
	pub(crate) const fn with_c2(self, strict: bool) -> Self {
		let flags =
			if strict { self.flags | FLAG_C2_STRICT }
			else { self.flags | FLAG_C2 };

		Self {
			buf: self.buf,
			flags,
		}
	}

	/// # With Subchannel.
	///
	/// Use subcode timestamps to cross-check the sector being requested is
	/// actually the sector being read. (All other subcode data is ignored.)
	pub(crate) const fn with_subchannel(self) -> Self {
		let flags = self.flags | FLAG_SUBCHANNEL;
		Self {
			buf: self.buf,
			flags,
		}
	}
}

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
		match (self.has_flag(FLAG_SUBCHANNEL), self.has_flag(FLAG_C2)) {
			// Read both!
			(true, true) => {
				// Verify and hash the data so we can compare the separate
				// read.
				self.read_subchannel(cdio, lsn)?;
				let hash = crc32fast::hash(self.data_slice());

				// Read again with C2 details.
				self.read_c2(cdio, lsn)?;

				// Make sure we got the same data both times.
				if hash == crc32fast::hash(self.data_slice()) { Ok(()) }
				// If not, treat it like a generic read error.
				else { Err(RipRipError::CdRead(lsn)) }
			},
			// Read subchannel.
			(true, false) => {
				self.read_subchannel(cdio, lsn)?;

				// Make sure our C2 bits are all set to good since we aren't
				// requesting that data.
				self.set_c2_good();
				Ok(())
			},
			// Read C2.
			(false, true) => self.read_c2(cdio, lsn),
			// Just the data.
			(false, false) => {
				// Make sure our C2 bits are all set to good since we aren't
				// requesting that data.
				self.set_c2_good();

				// Read the data.
				cdio.read_cd(
					&mut self.buf[..usize::from(CD_DATA_SIZE)],
					lsn,
				)
			},
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
		let res = cdio.read_subchannel(
			&mut self.buf[..usize::from(CD_DATA_SUBCHANNEL_SIZE)],
			lsn,
		);

		// Mark C2 bad if we got a sync error.
		if matches!(res, Err(RipRipError::SubchannelDesync)) {
			self.set_c2_bad();
		}

		res
	}

	/// # Mark All C2 Bad.
	fn set_c2_bad(&mut self) {
		for v in &mut self.buf[usize::from(CD_DATA_SIZE)..] {
			*v = 0b1111_1111;
		}
	}

	/// # Mark All C2 Good.
	fn set_c2_good(&mut self) {
		for v in &mut self.buf[usize::from(CD_DATA_SIZE)..] {
			*v = 0;
		}
	}
}

/// # Getters.
impl RipBuffer {
	/// # C2 Sample Errors.
	///
	/// Return an iterator over the per-sample C2 errors for the sector.
	pub(crate) fn errors(&self) -> BufferErrors {
		BufferErrors {
			set: self.c2_slice(),
			pos: 0,
			buf: None,
		}
	}

	/// # Data Samples.
	///
	/// Return an iterator over the 4-byte audio samples for the sector.
	pub(crate) fn samples(&self) -> BufferSamples {
		BufferSamples {
			set: self.data_slice().chunks_exact(4)
		}
	}
}

/// # Internal.
impl RipBuffer {
	/// # C2 Slice.
	///
	/// Return the portion of the buffer containing the C2 error bits.
	fn c2_slice(&self) -> &[u8] { &self.buf[usize::from(CD_DATA_SIZE)..] }

	/// # Data Slice.
	///
	/// Return the portion of the buffer containing the audio data.
	fn data_slice(&self) -> &[u8] { &self.buf[..usize::from(CD_DATA_SIZE)] }

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



/// # Per-Sample C2.
///
/// This iterator divides up the C2 responses into per-sample states, returning
/// `true` if the sample is bad, `false` if it is (allegedly) good.
pub(crate) struct BufferErrors<'a> {
	set: &'a [u8],
	pos: usize,
	buf: Option<bool>,
}

impl<'a> Iterator for BufferErrors<'a> {
	type Item = bool;

	fn next(&mut self) -> Option<Self::Item> {
		// Return the second half of the last byte checked.
		if let Some(next) = self.buf.take() { return Some(next); }

		// Read the next pair.
		let pair: u8 = self.set.get(self.pos).copied()?;
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

impl<'a> ExactSizeIterator for BufferErrors<'a> {
	fn len(&self) -> usize {
		// Each byte is 2 samples, so double what's left, then add one for the
		// buffer value, if any.
		usize::from(CD_C2_SIZE).saturating_sub(self.pos) * 2 + usize::from(self.buf.is_some())
	}
}



/// # Buffer Samples.
///
/// This iterator returns each sample as a 4-byte array.
pub(crate) struct BufferSamples<'a> {
	set: ChunksExact<'a, u8>,
}

impl<'a> Iterator for BufferSamples<'a> {
	type Item = [u8; 4];

	fn next(&mut self) -> Option<Self::Item> {
		self.set.next().and_then(|n| n.try_into().ok())
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl<'a> ExactSizeIterator for BufferSamples<'a> {
	fn len(&self) -> usize { self.set.len() }
}



#[cfg(test)]
mod test {
	use super::*;
	use crate::{
		NULL_SAMPLE,
		SAMPLES_PER_SECTOR,
	};

	#[test]
	fn t_buf_iters() {
		let buf = RipBuffer::default();

		// Both iterators should return 588 things.
		assert_eq!(buf.errors().count(), usize::from(SAMPLES_PER_SECTOR));
		assert_eq!(buf.samples().count(), usize::from(SAMPLES_PER_SECTOR));

		// In the default state, all samples should be null.
		assert!(buf.samples().all(|v| v == NULL_SAMPLE));

		// In the default state, there shouldn't be any errors.
		assert!(buf.errors().all(|v| ! v));
	}
}
