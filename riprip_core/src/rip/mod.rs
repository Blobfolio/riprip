/*!
# Rip Rip Hooray: Ripping
*/

pub(super) mod buf;
pub(super) mod data;
pub(super) mod iter;
pub(super) mod opts;
mod quality;

use cdtoc::Track;
use crate::{
	chk_accuraterip,
	chk_ctdb,
	Disc,
	KillSwitch,
	ReadOffset,
	RipBuffer,
	RipOptions,
	RipRipError,
	RipSamples,
	SAMPLE_OVERREAD,
};
use dactyl::NiceU32;
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



/// # Rip Session.
///
/// This struct holds everything needed to (re-)rip a track.
pub(crate) struct Rip<'a> {
	disc: &'a Disc,
	opts: &'a RipOptions,
	distance: ReadIter,
	state: RipSamples,
	q_desync: u32,
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
			q_desync: 0,
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
		// Rip the track or not.
		let confirmed =
			if killed.killed() { self.state.is_confirmed() }
			else {
				// Reset the counts before beginning?
				if self.opts.reset_counts() { self.state.reset_counts(); }

				self._rip(progress, killed)?
			};

		// Extract it!
		self.state.save_track(self.opts.raw()).map(|k| (k, confirmed))
	}

	#[allow(
		clippy::cast_possible_truncation,
		clippy::cast_possible_wrap,
	)]
	/// # Rip (For Real).
	///
	/// This is the ripping workhorse. It loops through the desired number of
	/// passes, (re)reading sectors that have room for improvement, checking
	/// the resulting track against AccurateRip/CTDB, and updating the saved
	/// state file.
	///
	/// Returns `true` if the rip has been confirmed, `false` if not.
	///
	/// ## Errors
	///
	/// I/O errors other than general read errors are bubbled up.
	fn _rip(&mut self, progress: &Progless, killed: &KillSwitch)
	-> Result<bool, RipRipError> {
		// Set up the buffer.
		let mut buf = RipBuffer::default();
		if self.opts.subchannel() { buf = buf.with_subchannel(); }
		if self.opts.c2() { buf = buf.with_c2(self.opts.strict()); }

		// A few other variables…
		let offset = self.opts.offset();
		let rip_rng = self.state.sector_rip_range();
		let lsn_start = rip_rng.start;
		let leadout = self.disc.toc().audio_leadout() as i32;
		let progress_label = format!("Track #{:02}", self.state.track().number());
		let mut confirmed = self.state.is_confirmed();

		// Onto the pass(es)!
		for pass in 0..self.opts.passes() {
			// Note the starting state.
			let before = self.state.quick_hash();
			self.q_desync = 0;

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
				rip_title(pass, self.state.is_new(), self.opts.backwards()),
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

				// Read it!
				let desynced =
					match buf.read_sector(self.disc.cdio(), read_lsn) {
						// Good is good!
						Ok(()) => false,
						// Silently skip generic read errors.
						Err(RipRipError::CdRead(_)) => {
							progress.increment();
							continue;
						},
						// Count up subcode desync but accept the data as bad.
						Err(RipRipError::SubchannelDesync) => {
							self.q_desync += 1;
							true
						},
						// Abort for all other kinds of errors.
						Err(e) => return Err(e),
					};

				// Patch the data, unless the user just aborted, as that will
				// probably have messed up the data.
				if ! killed.killed() {
					for ((old, new), err) in state.iter_mut()
						.zip(buf.samples())
						.zip(buf.errors())
					{
						old.update(new, err, desynced);
					}
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

		Msg::custom(
			"Ripped",
			if q_to.is_confirmed() { 10 } else { 4 },
			&q_to.summarize(track.number()),
		)
			.with_newline(true)
			.eprint();

		// Print the bar and legend(s).
		eprintln!("        {}", q_to.bar());
		let (legend_a, legend_b) = q_to.legend(&self.q_from);
		if let Some(legend_a) = legend_a { eprintln!("        {legend_a}"); }
		eprintln!("        {legend_b} \x1b[2msamples\x1b[0m");

		// Mention subchannel errors, if any.
		if ! q_to.is_confirmed() && self.q_desync != 0 {
			eprintln!(
				"        \x1b[91m{}\x1b[0;2m sector subchannel error{}\x1b[0m",
				NiceU32::from(self.q_desync),
				if self.q_desync == 1 { "" } else { "s" },
			);
		}

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

/// # Rip Title.
///
/// Return a description for the rip progress bar, drawing attention to
/// direction and newness, with a little bit of periodic sass.
const fn rip_title(pass: u8, new: bool, backwards: bool) -> &'static str {
	match (! new) as u8 + pass {
		0 => "Starting a new rip…",
		1 =>
			if backwards && pass == 0 { "Re-ripping the iffy bits, backwards, and in heels…" }
			else { "Re-ripping the iffy bits…" },
		5  => "Ripticulating splines…",
		10 => "Reconnoitering the rip…",
		15 => "Rip-a-dee-doo-dah, rip-a-dee-ay…",
		20 => "Recovery is more of an art than a science, really…",
		25 => "The quickest way is sometimes the longest…",
		32 if new => "Pulling the rip cord…",
		33 if ! new => "Pulling the rip cord…",
		_ => "Re-re-ripping, et cetera, ad nauseam…",
	}
}
