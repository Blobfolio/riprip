/*!
# Rip Rip Hooray: Log
*/

use cdtoc::Track;
use crate::macros::log;
use dactyl::{
	NiceElapsed,
	NiceU64,
	NiceU16,
};
use std::{
	num::NonZeroU8,
	time::Instant,
};



/// # Pass Counter.
///
/// This struct is largely a legacy of an earlier logging implementation, but
/// remains useful for tracking the runtime and problematic sector/sample
/// totals by pass.
pub(super) struct RipLog {
	/// # Pass Number, Timestamp.
	pass: Option<(NonZeroU8, Instant)>,

	/// # Problematic Sectors.
	sectors: u64,

	/// # Problematic Samples.
	samples: u64,
}

impl Drop for RipLog {
	/// # Final Print (Maybe).
	///
	/// This will print any remaining log data before retiring.
	fn drop(&mut self) { self.flush(); }
}

impl RipLog {
	/// # New Instance.
	pub(super) const fn new() -> Self {
		Self {
			pass: None,
			sectors: 0,
			samples: 0,
		}
	}

	/// # New Pass!
	///
	/// This prints the contents of the previous pass, if any, and increments
	/// the pass counter so it can start all over again.
	pub(super) fn bump_pass(&mut self) {
		self.flush();

		// Unnecessary but unhurtful.
		self.sectors = 0;
		self.samples = 0;

		let next = self.pass.map_or(NonZeroU8::MIN, |(p, _)| p.saturating_add(1));
		log!(@info "Starting pass {next}.");
		self.pass.replace((next, Instant::now()));
	}

	/// # Add Bad Sample Count.
	///
	/// Record the number of bad samples (`total`) associated with `lsn`.
	pub(super) fn bump_problems(
		&mut self,
		track: Track,
		lsn: i32,
		bad: u16,
		confused: u16,
	) {
		let bad = u16::min(bad, 588);
		let confused = u16::min(confused, 588);
		if bad != 0 {
			log!(
				@warn
				"Read {} bad sample{} from {lsn:06} (track {:02}).",
				NiceU16::from(bad),
				if bad == 1 { "" } else { "s" },
				track.number(),
			);
		}
		if confused != 0 {
			log!(
				@warn
				"Read {} bad sample{} from {lsn:06} (track {:02}).",
				NiceU16::from(confused),
				if confused == 1 { "" } else { "s" },
				track.number(),
			);
		}
		if bad != 0 || confused != 0 {
			self.sectors += 1;
			self.samples += u64::from(bad + confused);
		}
	}

	/// # Flush.
	///
	/// Print the held data, if any, to STDOUT, and drain it so a new pass can
	/// start fresh.
	///
	/// This uses a locked writer so content should appear in the correct
	/// order, but one never knows with terminals…
	fn flush(&mut self) {
		// Header.
		let Some((pass, start)) = self.pass else { return; };

		log!(@info "Finished pass {pass} in {}.", NiceElapsed::from(start));
		if 0 != self.sectors {
			log!(
				@warn
				"Found {} problematic sample{} across {} sector{}.",
				NiceU64::from(self.samples),
				if self.samples == 1 { "" } else { "s" },
				NiceU64::from(self.sectors),
				if self.sectors == 1 { "" } else { "s" },
			);

			self.sectors = 0;
			self.samples = 0;
		}
	}
}
