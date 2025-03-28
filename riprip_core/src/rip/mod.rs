/*!
# Rip Rip Hooray: Ripping
*/

pub(super) mod buf;
pub(super) mod data;
mod iter;
mod log;
pub(super) mod opts;
mod quality;
pub(super) mod sample;


use cdtoc::{
	Toc,
	Track,
};
use crate::{
	chk_accuraterip,
	chk_ctdb,
	COLOR_BAD,
	COLOR_CONFIRMED,
	COLOR_LIKELY,
	COLOR_MAYBE,
	Disc,
	KillSwitch,
	LibcdioInstance,
	RipBuffer,
	RipOptions,
	RipRipError,
	RipState,
	SavedRips,
	SECTOR_OVERREAD,
	state_path,
};
use dactyl::{
	NiceElapsed,
	NiceU32,
	NiceU8,
	traits::NiceInflection,
};
use fyi_msg::{
	Msg,
	Progless,
};
use iter::OffsetRipIter;
use log::RipLog;
use quality::TrackQuality;
use std::{
	borrow::Cow,
	collections::BTreeMap,
	num::NonZeroU32,
	path::PathBuf,
	time::Instant,
};



/// # Sassy Setup Messages.
const STANDBY: [&str; 2] = [
	"Reconnoitering the rip…",
	"Ripticulating splines…",
];



/// # Rip Manager.
///
/// This holds the disc details, ripping options, etc., to coordinate the rip
/// action(s) when `Ripper::rip` is called.
pub(crate) struct Ripper<'a> {
	/// # Date Created.
	now: Instant,

	/// # Disc Details.
	disc: &'a Disc,

	/// # Options.
	opts: RipOptions,

	/// # Track Details.
	tracks: BTreeMap<u8, RipEntry>,

	/// # Total Sectors (across all passes, plus one)
	total: NonZeroU32,
}

impl<'a> Ripper<'a> {
	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	/// # New!
	///
	/// Initialize from a disc and options.
	///
	/// This verifies the requested track list actually matches the disc, then
	/// counts up the total number of sectors being traversed, but that's it.
	/// The real work comes later.
	pub(crate) fn new(disc: &'a Disc, opts: &RipOptions) -> Result<Self, RipRipError> {
		// Build up entries for each track, prepopulating quality, etc., for
		// existing entries. We'll also be printing a temporary message since
		// it might take a while.
		let toc = disc.toc();
		let padding = u32::from(SECTOR_OVERREAD) * 2 - u32::from(opts.offset().sectors_abs());
		let tracks = opts.tracks()
			.map(|idx| RipEntry::new(toc, idx, padding).map(|e| (idx, e)))
			.collect::<Result<BTreeMap<u8, RipEntry>, RipRipError>>()?;
		if tracks.is_empty() { return Err(RipRipError::Noop); }

		// Last but not least, add up all the sectors to give us a total for
		// the progress bar during ripping. (The +1 is to leave room for some
		// title changes after the last read operation.)
		let total = tracks.values()
			.try_fold(0_u32, |acc, e| acc.checked_add(e.sectors))
			.and_then(|n| n.checked_mul(u32::from(opts.passes())))
			.and_then(|n| n.checked_add(1 + tracks.len() as u32))
			.and_then(NonZeroU32::new)
			.ok_or(RipRipError::RipOverflow)?;

		Ok(Self {
			now: Instant::now(),
			disc,
			opts: *opts,
			tracks,
			total,
		})
	}

	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	/// # Rip All Passes and Tracks!
	///
	/// This sets up some shared buffers, the progress bar, etc., then loops
	/// through each "pass", and within that, loops through each track. In
	/// other words, this will rip tracks 1, 2, 3, 1, 2, 3, rather than 1, 1,
	/// 2, 2, 3, 3.
	///
	/// This approach results in more accurate reads and less wear and tear on
	/// the drive, but adds some complication and overhead, namely in that the
	/// large state data has to be opened/closed multiple times when doing
	/// multiple passes, except when there's only one track being worked on.
	///
	/// Whatever. It is what it is.
	///
	/// The actual read/writing logic is handled by `RipEntry::rip`, called by
	/// this method.
	///
	/// Aside from the ripping, this will also verify and export each track.
	///
	/// ## Errors
	///
	/// General read errors aren't a show-stopper, but if the drive doesn't
	/// support an operation at all — it's missing a feature, etc. — or there
	/// are I/O issues with the state data, etc., those will kill the process
	/// and be returned.
	pub(crate) fn rip(&mut self, progress: &Progless, killed: KillSwitch)
	-> Result<(), RipRipError> {
		// We should definitely have a first track, but if for some reason we
		// don't there's nothing more to do!
		let Some(first_track) = self.tracks.values().map(|t| t.track).next() else {
			return Ok(());
		};

		// Load a bunch of other stuff!
		let toc = self.disc.toc();
		progress.reset(self.total);
		progress.set_title(Some(Msg::custom("Initializing", 199, standby_msg())));
		let mut state = RipState::new(toc, first_track, &self.opts)?;
		let mut share = RipShare::new(self.disc, progress, killed);

		// Before we run through the passes, let's set up the initial quality,
		// etc. But only if we're resuming.
		if self.opts.resume() {
			let mut first_track = None;
			for entry in self.tracks.values_mut() {
				if
					! killed.killed() &&
					state_path(toc, entry.track).is_ok_and(|s| s.is_file())
				{
					state.replace(entry.track, &self.opts)?;
					if entry.preverify(&state, &self.opts)? {
						let _res = share.progress.push_msg(happy_track_msg(entry.track));
						progress.increment_n(entry.sectors * u32::from(self.opts.passes()));
					}
				}

				// Note the first non-skippable track so we can reset afterward.
				if first_track.is_none() && ! entry.skippable() {
					first_track.replace(entry.track);
				}

				progress.increment();
			}

			// Disable the count-resetting option; that will have triggered
			// during this pass if applicable.
			self.opts = self.opts.with_reset(false);

			// Reset the loaded track to the first one we'll actually be
			// ripping.
			if let Some(first_track) = first_track {
				state.replace(first_track, &self.opts)?;
			}
			// Nothing to do!
			else {
				progress.finish();
				return Ok(());
			}
		}
		// Otherwise we can skip this step.
		else { progress.increment_n(self.tracks.len() as u32); }

		// Loop each pass!
		for pass in 1..=self.opts.passes() {
			// Fire up the log if we're logging.
			if self.opts.verbose() { share.log.bump_pass(); }

			// Bump the pass in our shared data. We can skip the initial cache
			// bust if this entry is brand new, and we aren't no-resuming or
			// anything like that.
			share.bump_pass(&self.opts);
			if pass == 1 && state.is_new() && self.opts.resume() {
				share.force_bust = false;
			}

			// Loop each track!
			for entry in self.tracks.values_mut() {
				// Skip the work if we aborted or already confirmed the track
				// is complete.
				if entry.skippable() { continue; }
				if killed.killed() {
					progress.increment_n(entry.sectors);
					continue;
				}

				// Switch states if needed.
				if state.track() != entry.track {
					set_progress_title(progress, entry.track.number(), "Initializing…");
					state.replace(entry.track, &self.opts)?;
				}

				// Rip it! If the result comes back confirmed and we were
				// planning additional passes, we can increase the progress
				// (remove them from the todo) accordingly.
				if entry.rip(&mut share, &mut state, &self.opts)? {
					let skip = u32::from(self.opts.passes() - pass) * entry.sectors;
					if skip != 0 { progress.increment_n(skip); }
					let _res = share.progress.push_msg(happy_track_msg(entry.track));
				}
			}

			// Flip the read order for next time?
			if self.opts.flip_flop() {
				self.opts = self.opts.with_backwards(! self.opts.backwards());
			}
			// After the first pass, always resume, never reset.
			if pass == 1 {
				self.opts = self.opts.with_resume(true);
			}
		}

		progress.finish();

		// Add some line breaks if we printed any confirmation messages.
		if self.tracks.values().any(RipEntry::skippable) { eprintln!("\n"); }

		Ok(())
	}

	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	/// # Status.
	///
	/// Check the status of each track and nothing else.
	pub(crate) fn status(&mut self, progress: &Progless, killed: KillSwitch)
	-> Result<(), RipRipError> {
		// We should definitely have a first track, but if for some reason we
		// don't there's nothing more to do!
		let Some(first_track) = self.tracks.values().map(|t| t.track).next() else {
			return Err(RipRipError::FirstTrackNum);
		};

		// Load a bunch of other stuff!
		let toc = self.disc.toc();
		let _res = progress.try_reset(self.tracks.len() as u32);
		progress.set_title(Some(Msg::custom("Analyzing", 199, standby_msg())));
		let mut state = RipState::new(toc, first_track, &self.opts)?;

		// Take a look!
		for entry in self.tracks.values_mut() {
			if killed.killed() { return Err(RipRipError::Killed); }

			if state_path(toc, entry.track).is_ok_and(|s| s.is_file()) {
				state.replace(entry.track, &self.opts)?;
				entry.preverify(&state, &self.opts)?;
			}

			progress.increment();
		}

		progress.finish();

		Ok(())
	}
}

impl Ripper<'_> {
	/// # Summarize.
	///
	/// Print a colored bar, some numbers, and a status for the rip as a whole.
	/// This is displayed along with the `Disc` summary details once all work
	/// has completed.
	pub(crate) fn summarize(&self) {
		// Add up the totals
		let Some((q1, q2)) = self.tracks.values()
			.map(|t| t.quality)
			.reduce(|a, b| (a.0 + b.0, a.1 + b.1)) else { return; };

		// Print some words.
		let ripped = self.tracks.values().filter(|t| t.dst.is_some()).count();
		let elapsed = NiceElapsed::from(self.now.elapsed());
		Msg::custom("Ripped", 199, &format!(
			"{}, {}, in {elapsed}.",
			ripped.nice_inflect("track", "tracks"),
			self.opts.passes().nice_inflect("pass", "passes"),
		))
			.with_newline(true)
			.eprint();
		Msg::custom("Status", 199, &q2.summarize())
			.with_newline(true)
			.eprint();

		// Print the bar and legend(s).
		eprintln!("        {}", q2.bar());
		let legend = q2.legend(&q1);
		if let Some(legend_a) = legend.start() { eprintln!("        {legend_a}"); }
		eprintln!("        {legend} \x1b[2msamples\x1b[0m");

		// An extra line to give some separation between this task and the
		// next.
		eprintln!();
	}

	#[expect(clippy::option_if_let_else, reason = "Too messy.")]
	#[expect(clippy::type_complexity, reason = "It is only used internally.")]
	/// # Summarize Per-Track Status.
	///
	/// Print a simple table of each track and its status.
	pub(crate) fn summarize_status(&self) {
		use std::io::Write;

		let writer = std::io::stderr();
		let mut handle = writer.lock();

		let conf = self.opts.confidence();
		let zero = NiceU32::from(0_u32);

		//             Idx Bad      Maybe    Likely,  AR                CTDB.
		let rows: Vec<(u8, NiceU32, NiceU32, NiceU32, Option<(u8, u8)>, Option<u16>)> =
			self.tracks.values()
			.map(|t| {
				let idx = t.track.number();
				let (_, q) = t.quality;
				let ar = t.ar.filter(|&(v1, v2)| conf <= v1 || conf <= v2);
				let ctdb = t.ctdb.filter(|&v1| u16::from(conf) <= v1);
				let likely = NiceU32::from(q.confirmed() + q.likely());
				let maybe = NiceU32::from(q.maybe());
				let bad =
					if maybe == zero && likely == zero { zero }
					else { NiceU32::from(q.bad()) };

				(idx, bad, maybe, likely, ar, ctdb)
			})
			.collect();

		// Find the max widths so we can align pretty.
		let mut any_ar = false;
		let mut any_ctdb = false;
		let mut wbad: usize = 3;
		let mut wmaybe: usize = 5;
		let mut wlikely: usize = 6;
		for (_, bad, maybe, likely, ar, ctdb) in &rows {
			any_ar = any_ar || ar.is_some();
			any_ctdb = any_ctdb || ctdb.is_some();

			wbad = usize::max(wbad, bad.len());
			wmaybe = usize::max(wmaybe, maybe.len());
			wlikely = usize::max(wlikely, likely.len());
		}

		// Print each track.
		for (idx, bad, maybe, likely, ar, ctdb) in rows {
			// No status?
			if bad == zero && maybe == zero && likely == zero {
				writeln!(
					&mut handle,
					"{idx:02}  \x1b[2m{:>wbad$}  {:>wmaybe$}  {:>wlikely$}\x1b[0m",
					"--",
					"--",
					"--",
				).unwrap();
			}
			// Status!
			else {
				writeln!(
					&mut handle,
					"{idx:02}  \x1b[{COLOR_BAD}m{:>wbad$}  \x1b[0;{COLOR_MAYBE}m{:>wmaybe$}  \x1b[0;{}m{:>wlikely$}\x1b[0m{}{}",
					bad.as_str(),
					maybe.as_str(),
					if ar.is_some() || ctdb.is_some() { COLOR_CONFIRMED } else { COLOR_LIKELY },
					likely.as_str(),
					if let Some((v1, v2)) = ar {
						let nice_v1 = NiceU8::from(v1.min(99));
						let nice_v2 = NiceU8::from(v2.min(99));
						Cow::Owned(format!(
							"        \x1b[1;{COLOR_CONFIRMED}m{}\x1b[0;2m/\x1b[0;1;{COLOR_CONFIRMED}m{}\x1b[0m",
							if v1 == 0 { "--" } else { nice_v1.as_str2() },
							if v2 == 0 { "--" } else { nice_v2.as_str2() },
						))
					}
					else if any_ar { Cow::Borrowed("             ") }
					else { Cow::Borrowed("") },
					if let Some(v1) = ctdb {
						Cow::Owned(format!(
							"       \x1b[1;{COLOR_CONFIRMED}m{:03}\x1b[0m",
							v1.min(999),
						))
					}
					else if any_ctdb { Cow::Borrowed("          ") }
					else { Cow::Borrowed("") },
				).unwrap();
			}
		}

		// One more line for good measure.
		writeln!(&mut handle).unwrap();
		handle.flush().unwrap();
	}

	/// # Finish.
	///
	/// Dissolve the instance and return the tracks we actually exported, along
	/// with their confirmation details. Specifically, this returns the file
	/// path and AccurateRip/CTDB match counts, indexed by track number.
	pub(crate) fn finish(self) -> Option<SavedRips> {
		let conf = self.opts.confidence();
		let out: SavedRips = self.tracks.into_iter()
			.filter_map(|(k, v)| {
				let dst = v.dst?;
				let ar =
					if k == 0 && v.quality.1.is_likely() { Some((u8::MAX, u8::MAX))}
					else { v.ar.filter(|&(v1, v2)| conf <= v1 || conf <= v2) };
				let ctdb =
					if k == 0 && v.quality.1.is_likely() { Some(u16::MAX) }
					else { v.ctdb.filter(|&v1| u16::from(conf) <= v1) };
				Some((k, (dst, ar, ctdb)))
			})
			.collect();

		if out.is_empty() { None }
		else { Some(out) }
	}
}



/// # Basic Track Rip Info.
///
/// This is basically a track state's state, without all the pesky data bloat.
/// The `Ripper` structure uses this so it can broadly know where everything
/// stands, without the cost of perpetually holding the _full_ data for an
/// entire album or anything crazy like that.
struct RipEntry {
	/// # Destination Path.
	dst: Option<PathBuf>,

	/// # Track Details.
	track: Track,

	/// # Number of Sectors.
	sectors: u32,

	/// # Quality (Before and After).
	quality: (TrackQuality, TrackQuality),

	/// # AccurateRip Confidence.
	ar: Option<(u8, u8)>,

	/// # CTDB Confidence.
	ctdb: Option<u16>,
}

impl RipEntry {
	/// # New!
	///
	/// Try loading the previous rip (unless we aren't resuming) to grab the
	/// stats, and if confirmed, recrunch AccurateRip/CTDB so we can avoid
	/// having to ever look at it again.
	///
	/// ## Errors
	///
	/// This will return an error if the track is not in the TOC or the state
	/// file exists but is corrupt.
	fn new(toc: &Toc, idx: u8, padding: u32) -> Result<Self, RipRipError> {
		let track =
			if idx == 0 { toc.htoa() }
			else { toc.audio_track(usize::from(idx)) }
			.ok_or(RipRipError::NoTrack(idx))?;

		// Make sure the padded sector count fits u32. The state will do this
		// too, but a little redundancy isn't the end of the world.
		let sectors = u32::try_from(track.duration().sectors()).ok()
			.and_then(|n| n.checked_add(padding))
			.ok_or(RipRipError::RipOverflow)?;

		// Set the initial quality to bad; we'll fix this before getting
		// started.
		let samples = u32::try_from(track.duration().samples())
			.ok()
			.and_then(NonZeroU32::new)
			.ok_or(RipRipError::RipOverflow)?;
		let quality = TrackQuality::new_bad(samples);

		Ok(Self {
			dst: None,
			track,
			sectors,
			quality: (quality, quality),
			ar: None,
			ctdb: None,
		})
	}
}

impl RipEntry {
	/// # Rip!
	///
	/// Of the million different `rip` methods spread throughout this program,
	/// we've finally reached THE ONE THAT RIPS! Haha.
	///
	/// In other words, this what decides which sectors need to be read from
	/// the disc, and does it, saving them to the state at the correct read
	/// offset.
	///
	/// It runs sector-by-sector, skipping any blocks that contain nothing but
	/// confirmed or likely samples.
	///
	/// It also handles verbose logging, verification, and track export. (Plus
	/// if there are changes, it will resave the state file.)
	///
	/// Returns `true` if the track has been confirmed.
	///
	/// ## Errors
	///
	/// This will bubble up any errors encountered, except run-of-the-mill
	/// sector read or sync errors, which are simply recorded to the state as
	/// "bad" and/or skipped.
	fn rip(&mut self, share: &mut RipShare, state: &mut RipState, opts: &RipOptions)
	-> Result<bool, RipRipError> {
		// Update the title.
		let title = format!(
			"{}{}{}…",
			if share.pass == 1 && state.is_new() { "Ripping fresh" } else { "Re-ripping" },
			if opts.passes() == 1 { String::new() } else { format!(", pass #{}", share.pass) },
			if opts.backwards() { ", backwards, and in heels" } else { "" },
		);
		set_progress_title(share.progress, self.track.number(), &title);

		let mut any_read = false;
		let before = state.quick_hash();
		let rip_rng = state.sector_rip_range();

		for (read_lsn, sector) in state.offset_rip_iter(opts)? {
			// We can skip this block if the user aborted or there's
			// nothing to refine.
			if
				share.killed.killed() ||
				sector.iter().all(|v| v.is_likely(opts.rereads()))
			{
				share.progress.increment();
				continue;
			}

			// We might need to bust the cache before reading any track data.
			// This will trigger if the cache size has been set, we're doing
			// more than one pass, and either didn't read enough sectors on the
			// last pass to fill the buffer, or are reading the same track
			// back-to-back.
			if ! any_read {
				if let Some(cache_len) = share.should_bust_cache(self.track.number(), opts) {
					set_progress_title(
						share.progress,
						self.track.number(),
						"Busting the cache…",
					);
					share.log.add_cache_bust();
					share.buf.cache_bust(
						share.cdio,
						cache_len,
						&rip_rng,
						share.leadout,
						opts.backwards(),
						share.killed,
					);
					set_progress_title(share.progress, self.track.number(), &title);
				}
				else { share.last_read_track = self.track.number(); }
			}

			// Read and patch!
			any_read = true;
			share.pass_reads += 1;
			match share.buf.read_sector(share.cdio, read_lsn, opts) {
				// Good is good!
				Ok(all_good) => if ! share.killed.killed() {
					// Patch the data, unless the user just aborted, as that
					// will probably have messed up the data.
					for (old, (new, c2_err)) in sector.iter_mut().zip(share.buf.samples()) {
						old.update(new, c2_err, all_good);
					}
				},
				// Silently skip generic read errors.
				Err(RipRipError::CdRead) => if opts.verbose() {
					share.log.add_error(read_lsn, RipRipError::CdRead);
				},
				Err(RipRipError::SubchannelDesync) => if opts.verbose() {
					share.log.add_error(read_lsn, RipRipError::SubchannelDesync);
				},
				// Abort for all other kinds of errors.
				Err(e) => return Err(e),
			}

			// Count up the issues for this sector.
			if opts.verbose() {
				let mut total_bad = 0;
				let mut total_wishy = 0;
				for v in sector {
					if v.is_bad() { total_bad += 1; }
					else if v.is_confused() { total_wishy += 1; }
				}
				if total_bad != 0 {
					share.log.add_bad(self.track, read_lsn, total_bad);
				}
				if total_wishy != 0 {
					share.log.add_confused(self.track, read_lsn, total_wishy);
				}
			}

			share.progress.increment();
		}

		// Reverify if we changed any data, or haven't verified yet.
		self.quality.1 = state.track_quality(opts);
		if self.ar.is_none() || self.ctdb.is_none() || before != state.quick_hash() {
			self.verify(state, opts, share.progress);
		}

		// Save the state if we changed any data.
		let changed = before != state.quick_hash();
		if changed {
			// Resave the state.
			set_progress_title(
				share.progress,
				self.track.number(),
				"Saving the state…",
			);
			let _res = state.save_state();
		}

		// Don't forget to extract the track. Do this after every pass
		// in case people want to fuck with CUETools immediately.
		if self.dst.is_none() || changed {
			self.dst.replace(state.save_track()?);
		}

		Ok(self.skippable())
	}

	/// # Skippable?
	///
	/// Returns `true` if we have already loaded/exported this rip, and at last
	/// check all of its samples were confirmed or likely.
	///
	/// In other words, this is an all-or-nothing equivalent to the sector-by-
	/// sector skipping that would normally happen. Aside from avoiding an
	/// unnecessary loop, this prevents us having to read/decompress/deserialize
	/// the state data at all.
	const fn skippable(&self) -> bool {
		self.dst.is_some() && self.quality.1.is_confirmed()
	}

	/// # Verify Entry.
	///
	/// Unless this is the HTOA track, this will try to match the rip against
	/// the AccurateRip and CUETools databases, updating the match counts, if
	/// any. It will also mark the track as confirmed if it is, allowing us to
	/// skip any further work on it.
	///
	/// This will return `true` if verified.
	fn verify(&mut self, state: &RipState, opts: &RipOptions, progress: &Progless)
	-> bool {
		set_progress_title(progress, self.track.number(), "Verifying the rip…");

		// HTOA isn't verifiable. Boo.
		if self.track.is_htoa() { return false; }

		// Check AccurateRip and CTDB in separate threads.
		(self.ar, self.ctdb) = verify_track(self.track, state);

		// If we're confirmed and the state isn't, update the state and our
		// quality snapshot.
		let verified = opts.confidence() <= max_confidence(self.ar, self.ctdb);
		if verified && ! self.quality.1.is_confirmed() {
			self.quality.1 = TrackQuality::new_confirmed(self.quality.1.total());
		}

		// Return the answer.
		verified
	}

	/// # Pre-Verify Entry.
	///
	/// Check out the initial state of the rip before doing any new work. If
	/// previous work was done, update the starting quality to match.
	///
	/// Returns `true` if the track is already confirmed w/ AccurateRip or
	/// CUETools, `false` if not.
	///
	/// ## Errors
	///
	/// If the track is confirmed it will be exported here and now; an error
	/// will be returned in the unlikely event that fails.
	fn preverify(&mut self, state: &RipState, opts: &RipOptions)
	-> Result<bool, RipRipError> {
		if ! state.is_new() {
			(self.ar, self.ctdb) = verify_track(self.track, state);
			if opts.confidence() <= max_confidence(self.ar, self.ctdb) {
				let tmp = TrackQuality::new_confirmed(self.quality.1.total());
				self.quality = (tmp, tmp);
				self.dst.replace(state.save_track()?);
				return Ok(true);
			}

			let tmp = state.track_quality(opts);
			self.quality = (tmp, tmp);
		}

		Ok(false)
	}
}



/// # Rip Share.
///
/// This groups together all the shared elements needed exclusively during the
/// ripping run(s), eliminating the need to share ten million separate
/// variables between methods. Haha.
///
/// This does have some innate utility, however. It keeps track of the number
/// of reads made across all tracks during a given pass, as well as the last
/// track to have made a read, which we can then use to determine whether or
/// not cache busting is necessary.
///
/// (Unlike serial CD-rippers, we'd only ever read the same track twice in a
/// row, or sector twice in a row, if they're literally the only things left to
/// rip. We can usually get away with a lot less busting as a result.)
struct RipShare<'a> {
	/// # Buffer.
	buf: RipBuffer,

	/// # Event Log.
	log: RipLog,

	/// # Leadout Sector.
	leadout: i32,

	/// # Pass Number.
	pass: u8,

	/// # Reads This Pass.
	pass_reads: u32,

	/// # Force Bust?
	///
	/// When true, a cache bust will be attempted on the next read.
	force_bust: bool,

	/// # Last Read Track Number.
	last_read_track: u8,

	/// # CDIO Instance.
	cdio: &'a LibcdioInstance,

	/// # Progress Instance.
	progress: &'a Progless,

	/// # Killswitch.
	killed: KillSwitch,
}

impl<'a> RipShare<'a> {
	#[expect(clippy::cast_possible_wrap, reason = "False positive.")]
	/// # New Instance.
	const fn new(disc: &'a Disc, progress: &'a Progless, killed: KillSwitch) -> Self {
		Self {
			buf: RipBuffer::new(),
			log: RipLog::new(),
			leadout: disc.toc().audio_leadout_normalized() as i32,
			pass: 0,
			pass_reads: 0,
			force_bust: false,
			last_read_track: u8::MAX,
			cdio: disc.cdio(),
			progress,
			killed,
		}
	}

	/// # Bump Pass.
	///
	/// Increment the pass number, and potentially set the force-bust flag if
	/// the previous run failed to conduct enough reads to moot the cache on
	/// its own. (The flag exists so that we can clear the count straight away,
	/// and deal with busting if and when we actually need to read something.)
	fn bump_pass(&mut self, opts: &RipOptions) {
		// Force a cache bust if we didn't read enough during the previous pass
		// or are just getting started.
		let len = opts.cache_sectors();
		self.force_bust = len != 0 && self.pass_reads < len;
		self.pass_reads = 0;

		// Bump the pass.
		self.pass += 1;
	}

	/// # Should Bust Cache?
	///
	/// This method is only called at most once per track per pass, just before
	/// the first read operation (if there is one).
	///
	/// If the previous pass did not read enough samples to moot the cache on
	/// its own, or if the current track was the last track to be ripped (i.e.
	/// back-to-back), this will return the number of (random) sectors that
	/// need to be read to bust the cache.
	///
	/// Otherwise — most of the time — it will return `None`.
	fn should_bust_cache(&mut self, track: u8, opts: &RipOptions) -> Option<u32> {
		if self.force_bust || track == self.last_read_track {
			self.force_bust = false;
			self.last_read_track = track;
			let len = opts.cache_sectors();
			if 0 == len { None }
			else { Some(len) }
		}
		else { None }
	}
}



/// # Happy Track Message.
///
/// This returns a message for a track that has been confirmed.
fn happy_track_msg(track: Track) -> Msg {
	Msg::custom(
		"Accurate",
		10,
		&format!("Track #{:02} has been successfully rescued.", track.number()),
	)
		.with_newline(true)
}

#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
/// # Max Confidence.
///
/// Return the largest confidence value.
const fn max_confidence(ar: Option<(u8, u8)>, ctdb: Option<u16>) -> u8 {
	let mut max = 0;

	if let Some(v1) = ctdb {
		// AccurateRip tops out at 99, so we can leave early.
		if 99 <= v1 { return 99; }
		max = v1 as u8;
	}

	if let Some((v1, v2)) = ar {
		if max < v1 { max = v1; }
		if max < v2 { max = v2; }
	}

	max
}

/// # Set Progress Title.
///
/// Most of our progress bars share a common prefix based on the track number,
/// so this just abstracts away some of the tedium of generating that.
fn set_progress_title(progress: &Progless, idx: u8, msg: &str) {
	progress.set_title(Some(Msg::custom(
		format!("Track {idx:02}").as_str(),
		199,
		msg
	)));
}

/// # Stand By Message.
///
/// Pick a (reasonably) random message to display while setting up the rip.
fn standby_msg() -> &'static str {
	let idx = usize::try_from(utc2k::unixtime()).map_or(0, |n| n % STANDBY.len());
	STANDBY[idx]
}

/// # Track Number to Bitflag.
///
/// Redbook audio CDs can only have a maximum of 99 tracks — or 100 if we count
/// the HTOA as #0 — so we can represent all possible combinations using a
/// single `u128` bitflag. Aside from being `Copy`, this saves us the trouble
/// of having to sort/dedup some sort of vector-like structure.
///
/// This method converts a `u8` decimal into the equivalent flag. Out of range
/// values are silently treated as zero.
const fn track_idx_to_bits(idx: u8) -> u128 {
	if 99 < idx { 0 }
	else { 2_u128.pow(idx as u32) }
}

/// # Verify Track.
///
/// Check the track rip against both the AccurateRip and CUETools databases.
/// To improve performance, this performs each check in a separate thread.
fn verify_track(track: Track, state: &RipState) -> (Option<(u8, u8)>, Option<u16>) {
	std::thread::scope(|s| {
		let ar = s.spawn(|| chk_accuraterip(
			state.toc(),
			track,
			state.track_slice(),
		));
		let ctdb = s.spawn(|| chk_ctdb(
			state.toc(),
			track,
			state.rip_slice(),
		));
		(
			ar.join().ok().flatten().map(|(v1, v2)| (v1.min(99), v2.min(99))),
			ctdb.join().ok().flatten().map(|v1| v1.min(999)),
		)
	})
}
