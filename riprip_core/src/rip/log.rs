/*!
# Rip Rip Hooray: Log
*/

use cdtoc::Track;
use crate::RipRipError;
use dactyl::NiceElapsed;
use std::{
	io::Write,
	num::NonZeroU8,
	time::Instant,
};
use utc2k::FmtUtc2k;



/// # Super Basic Log.
///
/// This holds the log-worthy details from an individual pass, printing it out
/// en masse at the end of the run.
///
/// Doing it this way, versus printing each line in realtime, ensures
/// consistent ordering, otherwise it's a crapshoot.
pub(super) struct RipLog {
	pass: Option<(NonZeroU8, Instant)>,
	err: Vec<(i32, RipRipError, FmtUtc2k)>,
	state: Vec<(u8, i32, u8, RipLogKind)>,
}

impl Drop for RipLog {
	/// # Final Print Maybe.
	fn drop(&mut self) { self.flush(); }
}

impl RipLog {
	/// # New Instance.
	pub(super) const fn new() -> Self {
		Self {
			pass: None,
			err: Vec::new(),
			state: Vec::new(),
		}
	}

	/// # New Pass!
	pub(super) fn pass(&mut self, pass: u8) {
		self.flush();

		// Unnecessary but unhurtful.
		self.err.truncate(0);
		self.state.truncate(0);

		// This should never fail.
		if let Some(pass) = NonZeroU8::new(pass) {
			self.pass.replace((pass, Instant::now()));
		}
	}

	/// # Add Error.
	pub(super) fn add_error(&mut self, lsn: i32, err: RipRipError) {
		self.err.push((lsn, err, FmtUtc2k::now()));
	}

	/// # Add Bad Sample Count.
	pub(super) fn add_bad(&mut self, track: Track, lsn: i32, total: u8) {
		self.state.push((
			track.number(),
			lsn,
			total,
			RipLogKind::Bad,
		));
	}

	/// # Add Confused Sample Count.
	pub(super) fn add_confused(&mut self, track: Track, lsn: i32, total: u8) {
		self.state.push((
			track.number(),
			lsn,
			total,
			RipLogKind::Confused,
		));
	}

	/// # Flush.
	fn flush(&mut self) {
		// Header.
		let Some((pass, start)) = self.pass.take() else { return; };
		let writer = std::io::stdout();
		let mut handle = writer.lock();
		let _res = writeln!(
			&mut handle,
			r"##
## Pass {pass}: {}
## Problematic Sectors: {}
## Problematic Samples: {}
##",
			NiceElapsed::from(start),
			self.state.len(),
			self.state.iter().fold(0_usize, |acc, (_, _, v, _)| acc + usize::from(*v))
		);

		// Miscellaneous errors.
		if ! self.err.is_empty() {
			for (lsn, err, time) in self.err.drain(..) {
				let _res = writeln!(&mut handle, r"## [{time}] {lsn:06} {err}");
			}
			let _res =writeln!(&mut handle, "##");
		}

		// Sample issues.
		if ! self.state.is_empty() {
			self.state.sort_unstable_by(|a, b| a.1.cmp(&b.1));
			for (track, lsn, samples, kind) in self.state.drain(..) {
				let _res = writeln!(
					&mut handle,
					r"{track:02}  {lsn:06}  {samples:03}  {}",
					kind.as_str(),
				);
			}
		}

		// Write it!
		let _res = handle.flush();
	}
}



#[derive(Debug, Clone, Copy)]
/// # Sample Issue Kind.
enum RipLogKind {
	Bad,
	Confused,
}

impl RipLogKind {
	/// # As Str.
	const fn as_str(self) -> &'static str {
		match self {
			Self::Bad => "BAD",
			Self::Confused => "CONFUSED",
		}
	}
}
