/*!
# Rip Rip Hooray: Ripping
*/

pub(super) mod data;
pub(super) mod iter;
pub(super) mod opts;
mod quality;

use cdtoc::Track;
use crate::{
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	chk_accuraterip,
	chk_ctdb,
	Disc,
	KillSwitch,
	ReadOffset,
	RipOptions,
	RipRipError,
	RipSamples,
	SAMPLES_PER_SECTOR,
};
use dactyl::NiceFloat;
use fyi_msg::{
	Msg,
	Progless,
};
use quality::TrackQuality;
use iter::ReadIter;
use std::{
	ops::Range,
	path::PathBuf,
};



/// # Color: Bad.
const COLOR_BAD: &str = "91";

/// # Color: Maybe.
const COLOR_MAYBE: &str = "38;5;208";

/// # Color: Likely.
const COLOR_LIKELY: &str = "93";

/// # Color: Confirmed.
const COLOR_CONFIRMED: &str = "92";

/// # Sample Padding.
///
/// Our rip ranges are padded on either end by 10 sectors to make it easier for
/// drives with different read offsets to contribute to the same rip.
const SAMPLE_OVERREAD: u16 = SAMPLES_PER_SECTOR * 10;

/// # C2 Sample Set.
///
/// This contains a `bool` for each sample in a sector indicating whether or
/// not it contains an error.
type SectorC2s = [bool; SAMPLES_PER_SECTOR as usize];



/// # Rip Session.
///
/// This struct holds everything needed to (re-)rip a track.
pub(crate) struct Rip<'a> {
	disc: &'a Disc,
	opts: &'a RipOptions,
	distance: ReadIter,
	state: RipSamples,
	q_from: TrackQuality,
	q_ar: Option<(u8, u8)>,
	q_ctdb: Option<u16>,
}

impl<'a> Rip<'a> {
	/// # New!
	///
	/// Initialize, but don't start, a new rip session.
	pub(crate) fn new(disc: &'a Disc, track: Track, opts: &'a RipOptions)
	-> Result<Self, RipRipError> {
		let state = RipSamples::from_track(disc.toc(), track, opts.resume())?;
		let rng = state.sector_rip_range();
		let rng = rip_distance(
			rng.end - rng.start,
			opts.offset()
		);
		let distance = ReadIter::new(rng.start, rng.end, opts.backwards());
		let q_from = state.track_quality(opts.cutoff());

		Ok(Self {
			disc,
			opts,
			distance,
			state,
			q_from,
			q_ar: None,
			q_ctdb: None,
		})
	}

	/// # Rip!
	///
	/// Rip the track, maybe more than once!
	///
	/// This returns the destination path and a bool indicating whether or not
	/// AccurateRip/CTDB like the result, or an error.
	pub(crate) fn rip(&mut self, progress: &Progless, killed: &KillSwitch)
	-> Result<(PathBuf, bool), RipRipError> {
		let confirmed =
			if killed.killed() { self.state.is_confirmed() }
			else {
				// Same method two ways. The only difference is the buffer
				// size; a larger buffer is required for C2 when ripping
				// without.
				if self.opts.c2() {
					let mut buf = [0_u8; CD_DATA_C2_SIZE as usize];
					self._rip(&mut buf, progress, killed)?
				}
				else {
					let mut buf = [0_u8; CD_DATA_SIZE as usize];
					self._rip(&mut buf, progress, killed)?
				}
			};

		self.state.save_track(self.opts.raw()).map(|k| (k, confirmed))
	}

	#[allow(
		clippy::cast_possible_truncation,
		clippy::cast_possible_wrap,
	)]
	/// # Rip (For Real).
	///
	/// This method is separated out from the main one primarily because the
	/// fixed data buffer has a variable size depending on whether or not C2
	/// pointers are to be included. Creating those in the previous step allows
	/// us to avoid conflicts with Rust's type checker.
	///
	/// Returns `true` if the rip has been confirmed, `false` if not.
	fn _rip(&mut self, buf: &mut [u8], progress: &Progless, killed: &KillSwitch)
	-> Result<bool, RipRipError> {
		let resume = u8::from(! self.state.is_new());
		let offset = self.opts.offset();
		let rip_rng = self.state.sector_rip_range();
		let lsn_start = rip_rng.start;
		let leadout = self.disc.toc().audio_leadout() as i32;
		let mut c2: SectorC2s = [false; SAMPLES_PER_SECTOR as usize];
		let mut confirmed = self.state.is_confirmed();
		let progress_label = format!("Track #{:02}", self.state.track().number());

		// Onto the pass(es)!
		for pass in 0..self.opts.passes() {
			// Note the starting state.
			let before = self.state.quick_hash();

			// Reset progress bar. (This won't fail.)
			let _res = progress.reset((self.distance.len() as u32).saturating_add(1));

			// Bust the cache, but only if desired and productive.
			if
				self.opts.cache_bust() &&
				! (
					killed.killed() ||
					confirmed ||
					self.state.is_likely(offset, self.opts.cutoff())
				)
			{
				progress.set_title(Some(Msg::custom(progress_label.as_str(), 199, "Busting the cache…")));
				self.disc.cdio().bust_cache(rip_rng.clone(), leadout);
			}

			// Update the progress title to reflect the track at hand.
			progress.set_title(Some(Msg::custom(
				progress_label.as_str(),
				199,
				rip_title(pass + resume, self.opts.backwards()),
			)));

			// Pull down the data, one sector at a time.
			for k in self.distance.clone() {
				// Figure out which sector we're reading from, and what offset
				// sample that corresponds to.
				let read_lsn = lsn_start + k;
				let state = self.state.offset_sector_mut(read_lsn, offset)?;

				// We can skip this block if the user aborted or there's
				// nothing to refine.
				if
					confirmed ||
					killed.killed() ||
					state.iter().all(|v| v.is_likely(self.opts.cutoff()))
				{
					progress.increment();
					continue;
				}

				// Otherwise we have to actually talk to the drive. Ug.
				match self.disc.cdio().read_cd(buf, read_lsn) {
					Ok(()) =>
						// Parse the C2 data. Each bit represents one byte of
						// audio data, we'll never worry about sub-sample
						// accuracy.
						if self.opts.c2() {
							// Set errors at sector level.
							if self.opts.strict() {
								reset_c2(
									&mut c2,
									buf.iter()
										.skip(usize::from(CD_DATA_SIZE))
										.any(|&v| 0 != v)
								);
							}
							// Set errors at sample level.
							else {
								for (k2, &v) in c2.chunks_exact_mut(2).zip(buf.iter().skip(usize::from(CD_DATA_SIZE))) {
									k2[0] = 0 != v & 0b1111_0000;
									k2[1] = 0 != v & 0b0000_1111;
								}
							}
						}
						// Assume C2 is fine since that data is absent.
						else { reset_c2(&mut c2, false); },
					// Assume total C2 failure if there's a hard read error.
					Err(RipRipError::CdRead(_)) => { reset_c2(&mut c2, true); },
					// Other kinds of errors are show-stoppers; abort!
					Err(e) => return Err(e),
				}

				// Patch the data!
				for ((old, new), err) in state.iter_mut()
					.zip(buf[..usize::from(CD_DATA_SIZE)].chunks_exact(4))
					.zip(c2.iter().copied())
				{
					old.update(new.try_into().unwrap(), err);
				}

				progress.increment();
			} // End block.

			// Verification.
			if (self.q_ar.is_none() && self.q_ctdb.is_none()) || self.state.quick_hash() != before {
				progress.set_title(Some(Msg::custom(progress_label.as_str(), 199, "Verifying the ripped track…")));
				self.verify(&mut confirmed);
			}

			// Save the state.
			if self.state.quick_hash() != before {
				progress.set_title(Some(Msg::custom(progress_label.as_str(), 199, "Saving the state…")));
				let saved = self.state.save_state();
				if saved.is_err() {
					Msg::warning("The rip state could not be saved.").eprint();
				}
			}

			// Maybe stop early?
			if confirmed || killed.killed() { break; }
		} // End pass.

		progress.finish();
		Ok(confirmed)
	}

	/// # Verify.
	///
	/// Check the rip against AccurateRip/CUETools.
	fn verify(&mut self, confirmed: &mut bool) {
		// HTOA isn't verifiable. Boo.
		if self.state.track().is_htoa() { return; }

		self.q_ar = chk_accuraterip(
			self.disc.toc(),
			self.state.track(),
			self.state.track_slice(),
		);

		self.q_ctdb = chk_ctdb(
			self.disc.toc(),
			self.state.track(),
			self.state.rip_slice(),
		);

		// If the rip was confirmed with enough confidence, mark it
		// thusly!
		let conf = self.opts.confidence();
		if
			! *confirmed &&
			(
				self.q_ar.map_or(false, |(v1, v2)| conf <= v1 || conf <= v2) ||
				self.q_ctdb.map_or(false, |v| u16::from(conf) <= v)
			)
		{
			self.state.confirm_track();
			*confirmed = true;
		}
	}

	/// # Summarize.
	///
	/// Count up the different sample statuses and print a nice colored bar and
	/// legend to demonstrate the "quality". This will also print out
	/// AccurateRip and CTDB results, if any.
	pub(crate) fn summarize(&self) {
		// Figure out where we landed.
		let q_to = self.state.track_quality(self.opts.cutoff());
		let track = self.state.track();

		// Print a heading.
		if q_to.is_confirmed() {
			Msg::custom("Ripped", 10, &format!(
				"Track #{} has been accurately ripped!",
				track.number(),
			))
		}
		else if q_to.is_bad() {
			Msg::custom("Ripped", 4, &format!(
				"Track #{} still needs a lot of work!",
				track.number(),
			))
		}
		else {
			// Percentage(s) complete.
			let p_lo = NiceFloat::from(q_to.percent_likely());
			let p_hi = NiceFloat::from(q_to.percent_maybe());
			let qualifier = if q_to.maybe() == 0 { "likely" } else { "maybe" };

			// Show one percent if rounding makes both equivalent.
			if
				q_to.maybe() == 0 ||
				q_to.likely() == 0 ||
				p_lo.precise_str(3) == p_hi.precise_str(3)
			{
				// Omit the percentage entirely.
				if p_hi.compact_str() == "100" {
					Msg::custom("Ripped", 4, &format!(
						"Track #{} is \x1b[2m({qualifier})\x1b[0m complete.",
						track.number(),
					))
				}
				// Show it in its full glory.
				else {
					Msg::custom("Ripped", 4, &format!(
						"Track #{} is \x1b[2m({qualifier})\x1b[0m {}% complete.",
						track.number(),
						p_hi.compact_str(),
					))
				}
			}
			// Drop the 100% percent and call it "at least".
			else if p_hi.compact_str() == "100" {
				Msg::custom("Ripped", 4, &format!(
					"Track #{} is \x1b[2m({qualifier})\x1b[0m at least {}% complete.",
					track.number(),
					p_lo.precise_str(3),
				))
			}
			// Show both!
			else {
				Msg::custom("Ripped", 4, &format!(
					"Track #{} is \x1b[2m({qualifier})\x1b[0m {}% — {}% complete.",
					track.number(),
					p_lo.precise_str(3),
					p_hi.precise_str(3),
				))
			}
		}
			.with_newline(true)
			.eprint();

		// Print the bar and legend(s).
		eprintln!("        {}", q_to.bar());
		let (legend_a, legend_b) = q_to.legend(&self.q_from);
		if let Some(legend_a) = legend_a { eprintln!("        {legend_a}"); }
		eprintln!("        {legend_b} \x1b[2msamples\x1b[0m");

		// Third-party verification?
		if self.state.track().is_htoa() {
			eprintln!("        \x1b[2mHTOA tracks cannot be matched with AccurateRip or CUETools,\x1b[0m");
			if q_to.is_likely() {
				eprintln!("        \x1b[2mbut a \x1b[0;{COLOR_LIKELY}mlikely \x1b[0;2mrip is the next best thing, so good job!\x1b[0m");
			}
			else {
				eprintln!("        \x1b[2mso you should aim for a status of \x1b[0;{COLOR_LIKELY}mlikely\x1b[0;2m to be safe.\x1b[0m");
			}
		}
		else if self.q_ar.is_some() || self.q_ctdb.is_some() {
			let conf = self.opts.confidence();
			macro_rules! color {
				($v:expr, $conf:expr) => (
					if $v == 0 { COLOR_BAD }
					else if $v < $conf { COLOR_MAYBE }
					else { COLOR_CONFIRMED }
				);
			}

			if let Some((v1, v2)) = self.q_ar {
				let c1 = color!(v1, conf);
				let c2 = color!(v2, conf);
				eprintln!(
					"        AccurateRip: \x1b[{c1}m{:02}\x1b[0;2m+\x1b[0;{c2}m{:02}\x1b[0m",
					u8::min(99, v1),
					u8::min(99, v2),
				);
			}
			if let Some(v1) = self.q_ctdb {
				let c1 = color!(v1, u16::from(conf));
				eprintln!(
					"        CUETools DB: \x1b[{c1}m{:03}\x1b[0m",
					u16::min(999, v1),
				);
			}
		}

		// An extra line to give some separation between this task and the
		// next.
		eprintln!();
	}
}



#[inline]
/// # Reset C2 Statuses.
///
/// Change all C2 statuses to `val`.
fn reset_c2(set: &mut SectorC2s, val: bool) {
	for c2 in set { *c2 = val; }
}

/// # Rippable Sectors.
///
/// Read offsets mean data is written to a slightly different location than it
/// is read from. Hence _offset_.
///
/// (Different drives read data slightly earlier or later for whatever dumb
/// reason; offsets normalize the results so regardless of the drive, the rip
/// will always be the same.)
///
/// Our theoretical "rip range" is padded on both ends to account for this,
/// but since we only want to cover sectors that can be both read and written,
/// we won't end up using all of that space.
///
/// This method returns the minimum and maximum distance from the start of the
/// rip range that we can safely travel from.
fn rip_distance(max_sectors: i32, offset: ReadOffset) -> Range<i32> {
	let mut rng_start: i32 = 0;
	let mut rng_end: i32 = max_sectors;
	let sectors_abs = i32::from(offset.sectors_abs());

	// Negative offsets require the data be pushed forward to "start"
	// at the right place.
	if offset.is_negative() { rng_end -= sectors_abs; }
	// Positive offsets require the data be pulled backward instead.
	else { rng_start += sectors_abs; }

	rng_start..rng_end
}

#[inline]
/// # Rip Title Prefix.
///
/// Just for fun, change up the progress bar title from rip to rip.
const fn rip_title(pass: u8, backwards: bool) -> &'static str {
	match (pass, backwards) {
		(0, false) => "Ripping…",
		(1, false) => "Re-Ripping…",
		(2, false) => "Re-Re-Ripping…",
		(3, false) => "Re-Re-Re-Ripping…",
		(4, false) => "Re-Re-Re-Re-Ripping…",
		(5, false) => "Re-Re-Re-Re-Re-Ripping…",
		(_, false) => "Re-Re-Re-Re-Re-[…]-Ripping…",
		(0, true) => "Ripping… (Backwards)",
		(1, true) => "Re-Ripping… (Backwards)",
		(2, true) => "Re-Re-Ripping… (Backwards)",
		(3, true) => "Re-Re-Re-Ripping… (Backwards)",
		(4, true) => "Re-Re-Re-Re-Ripping… (Backwards)",
		(5, true) => "Re-Re-Re-Re-Re-Ripping… (Backwards)",
		(_, true) => "Re-Re-Re-Re-Re-[…]-Ripping… (Backwards)",
	}
}
