/*!
# Rip Rip Hooray: Ripping
*/

pub(super) mod data;
pub(super) mod iter;
pub(super) mod opts;

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
use dactyl::{
	NiceFloat,
	traits::SaturatingFrom,
};
use fyi_msg::{
	Msg,
	Progless,
};
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

/// # Quality Bar.
const QUALITY_BAR: &str = "########################################################################";

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

		Ok(Self {
			disc,
			opts,
			distance,
			state,
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

		// Onto the pass(es)!
		for pass in 0..self.opts.passes() {
			// Reset progress bar.
			let _res = progress.reset((self.distance.len() as u32).saturating_add(1)); // This won't fail.

			// Bust the cache.
			if self.opts.cache_bust() && ! (killed.killed() || confirmed) {
				progress.set_title(Some(Msg::custom("Standby", 11, "Busting the cache…")));
				self.disc.cdio().bust_cache(rip_rng.clone(), leadout);
			}

			// Update the progress title to reflect the track at hand.
			progress.set_title(Some(Msg::custom(
				rip_title_prefix(pass + resume),
				199,
				&format!(
					"Track #{}{}…",
					self.state.track().number(),
					if self.opts.backwards() { " (backwards)" } else { "" },
				)
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
			progress.set_title(Some(Msg::custom("Standby", 11, "Verifying the ripped track…")));
			let ar = chk_accuraterip(
				self.disc.toc(),
				self.state.track(),
				self.state.track_slice(),
			);
			let ctdb = chk_ctdb(
				self.disc.toc(),
				self.state.track(),
				self.state.track_slice(),
			);

			// If the rip was confirmed with enough confidence, mark it
			// thusly!
			let conf = self.opts.confidence();
			if
				! confirmed &&
				(
					ar.map_or(false, |(v1, v2)| conf <= v1 || conf <= v2) ||
					ctdb.map_or(false, |v| u16::from(conf) <= v)
				)
			{
				self.state.confirm_track();
				confirmed = true;
			}

			// Save the state.
			progress.set_title(Some(Msg::custom("Standby", 11, "Saving the state…")));
			let saved = self.state.save_state();
			progress.finish();

			if saved.is_err() {
				Msg::warning("The rip state could not be saved.").eprint();
			}

			// Summarize the results.
			self.summarize(confirmed, ar, ctdb);

			// Maybe stop early?
			if confirmed || killed.killed() { break; }
		} // End pass.

		Ok(confirmed)
	}

	/// # Summarize.
	///
	/// Count up the different sample statuses and print a nice colored bar and
	/// legend to demonstrate the "quality". This will also print out
	/// AccurateRip and CTDB results, if any.
	fn summarize(&self, confirmed: bool, ar: Option<(u8, u8)>, ctdb: Option<u16>) {
		let track = self.state.track();
		let (q_bad, q_maybe, q_likely, q_confirmed) =
			if confirmed { (0, 0, 0, usize::saturating_from(track.samples())) }
			else { self.state.track_quality(self.opts.cutoff()) };
		let q_total = q_bad + q_maybe + q_likely + q_confirmed;

		// All good.
		if confirmed {
			Msg::custom("Ripped", 10, &format!(
				"Track #{} has been accurately ripped!",
				track.number(),
			))
		}
		// All bad.
		else if q_bad == q_total {
			Msg::custom("Ripped", 4, &format!(
				"Track #{} still needs a lot of work!",
				track.number(),
			))
		}
		// Nothing likely yet.
		else if q_likely == 0 && q_confirmed == 0 {
			let p = NiceFloat::from(
				dactyl::int_div_float(q_maybe * 100, q_total).unwrap_or(0.0)
			);
			Msg::custom("Ripped", 4, &format!(
				"Track #{} is \x1b[2m(maybe)\x1b[0m {}% complete.",
				track.number(),
				p.compact_str(),
			))
		}
		// A completeness range.
		else {
			let q_total = q_bad + q_maybe + q_likely + q_confirmed;
			let low = NiceFloat::from(
				dactyl::int_div_float((q_likely + q_confirmed) * 100, q_total).unwrap_or(0.0)
			);
			let high = NiceFloat::from(
				dactyl::int_div_float((q_maybe + q_likely + q_confirmed) * 100, q_total).unwrap_or(0.0)
			);

			// If rounding makes the percentages the same, just print one.
			if low.precise_str(3) == high.precise_str(3) {
				Msg::custom("Ripped", 4, &format!(
					"Track #{} is \x1b[2m(likely)\x1b[0m {}% complete.",
					track.number(),
					low.compact_str(),
				))
			}
			// Otherwise show both.
			else {
				Msg::custom("Ripped", 4, &format!(
					"Track #{} is \x1b[2m(likely)\x1b[0m {}% – {}% complete.",
					track.number(),
					low.precise_str(3),
					high.precise_str(3),
				))
			}
		}
			.with_newline(true)
			.eprint();

		// Print a color-coded bar and legend.
		print_bar(q_bad, q_maybe, q_likely, q_confirmed);

		// Add AccurateRip, if any.
		let conf = self.opts.confidence();
		macro_rules! color {
			($v:expr, $conf:expr) => (
				if $v == 0 { COLOR_BAD }
				else if $v < $conf { COLOR_MAYBE }
				else { COLOR_CONFIRMED }
			);
		}
		if let Some((v1, v2)) = ar {
			let c1 = color!(v1, conf);
			let c2 = color!(v2, conf);
			eprintln!(
				"        AccurateRip: \x1b[{c1}m{:02}\x1b[0;2m+\x1b[0;{c2}m{:02}\x1b[0m",
				u8::min(99, v1),
				u8::min(99, v2),
			);
		}

		// Add CTDB, if any.
		if let Some(v1) = ctdb {
			let c1 = color!(v1, u16::from(conf));
			eprintln!(
				"        CUETools DB: \x1b[{c1}m{:03}\x1b[0m",
				u16::min(999, v1),
			);
		}

		// An extra new line to give some separation between this and the next
		// operation.
		eprintln!();
	}
}



#[allow(clippy::cast_precision_loss)]
/// # Print Summary Bar and Legend.
fn print_bar(q_bad: usize, q_maybe: usize, q_likely: usize, q_confirmed: usize) {
	let q_total = q_bad + q_maybe + q_likely + q_confirmed;
	let b_total = QUALITY_BAR.len() as f64;
	macro_rules! bar_slice {
		($val:ident) => (
			if $val == 0 { 0 }
			else {
				usize::max(1, (dactyl::int_div_float($val, q_total).unwrap_or(0.0) * b_total).floor() as usize)
			}
		);
	}
	let mut bars =[
		bar_slice!(q_bad),
		bar_slice!(q_maybe),
		bar_slice!(q_likely),
		bar_slice!(q_confirmed),
	];

	// Fix up rounding so we always have a full bar.
	let b_len = bars.iter().copied().sum::<usize>();
	let b_diff = b_len.abs_diff(QUALITY_BAR.len());

	// Too big.
	if b_len > QUALITY_BAR.len() {
		let max = bars.iter().copied().max().unwrap();
		for b in &mut bars {
			if *b == max {
				*b -= b_diff;
				break;
			}
		}
	}
	// Too small.
	else if 0 != b_diff {
		let max = bars.iter().copied().max().unwrap();
		for b in &mut bars {
			if *b == max {
				*b += b_diff;
				break;
			}
		}
	}

	eprintln!(
		"        \x1b[{COLOR_BAD}m{}\x1b[0;{COLOR_MAYBE}m{}\x1b[0;{COLOR_LIKELY}m{}\x1b[0;{COLOR_CONFIRMED}m{}\x1b[0m",
		&QUALITY_BAR[..bars[0]],
		&QUALITY_BAR[..bars[1]],
		&QUALITY_BAR[..bars[2]],
		&QUALITY_BAR[..bars[3]],
	);

	let mut breakdown = Vec::with_capacity(4);
	if q_bad != 0 { breakdown.push(format!("\x1b[{COLOR_BAD}m{q_bad}\x1b[0m")); }
	if q_maybe != 0 { breakdown.push(format!("\x1b[{COLOR_MAYBE}m{q_maybe}\x1b[0m")); }
	if q_likely != 0 { breakdown.push(format!("\x1b[{COLOR_LIKELY}m{q_likely}\x1b[0m")); }
	if q_confirmed != 0 { breakdown.push(format!("\x1b[{COLOR_CONFIRMED}m{q_confirmed}\x1b[0m")); }

	eprintln!("        {} \x1b[2msamples\x1b[0m", breakdown.join("\x1b[2m + \x1b[0m"));
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
/// Just for fun, the prefix used for the progress bar title during ripping
/// changes a little from pass-to-pass.
const fn rip_title_prefix(pass: u8) -> &'static str {
	match pass {
		0 => "Ripping",
		1 => "Re-Ripping",
		2 => "Re-Re-Ripping",
		3 => "Re-Re-Re-Ripping",
		_ => "Re-Re-Re-Etc.-Ripping",
	}
}
