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
	Disc,
	KillSwitch,
	LibcdioInstance,
	ReadOffset,
	RipBuffer,
	RipOptions,
	RipRipError,
	RipState,
	SavedRips,
	SECTOR_OVERREAD,
};
use dactyl::{
	NiceElapsed,
	traits::NiceInflection,
};
use fyi_msg::{
	Msg,
	Progless,
};
use iter::ReadIter;
use log::RipLog;
use quality::TrackQuality;
use std::{
	collections::BTreeMap,
	ops::Range,
	path::PathBuf,
	time::Instant,
};



/// # Rip Manager.
///
/// This holds the disc details, ripping options, etc., to coordinate the rip
/// action(s) when `Ripper::rip` is called.
pub(crate) struct Ripper<'a> {
	now: Instant,
	disc: &'a Disc,
	opts: RipOptions,
	tracks: BTreeMap<u8, RipEntry>,
	sectors: u32,
}

impl<'a> Ripper<'a> {
	/// # New!
	///
	/// Initialize from a disc and options.
	///
	/// This verifies the requested track list actually matches the disc, then
	/// counts up the total number of sectors being traversed, but that's it.
	/// The real work comes later.
	pub(crate) fn new(disc: &'a Disc, opts: &RipOptions) -> Result<Self, RipRipError> {
		// Redo the options so we can weed out invalid tracks.
		let opts = prune_tracks(opts, disc.toc())?;

		// Build up a basic list of the tracks we're going to be working on,
		// and add up their rippable sector counts to give us a grand total.
		let toc = disc.toc();
		let mut tracks = BTreeMap::default();

		// Count up the sectors, and pre-populate the tracks. We'll pad the
		// total by one so we can keep the progress bar alive for a few
		// extra status-related tasks at the end of the rip.
		let padding = u32::from(SECTOR_OVERREAD) * 2 - u32::from(opts.offset().sectors_abs());
		let mut total_sectors: u32 = 1;
		for t in opts.tracks().filter_map(|t|
			if t == 0 { toc.htoa() }
			else { toc.audio_track(usize::from(t)) }
		) {
			let sectors = u32::try_from(t.duration().sectors()).ok()
				.and_then(|n| n.checked_add(padding))
				.ok_or(RipRipError::RipOverflow)?;
			total_sectors = total_sectors.checked_add(sectors)
				.ok_or(RipRipError::RipOverflow)?;

			tracks.insert(t.number(), RipEntry {
				dst: None,
				track: t,
				sectors,
				quality: None,
				ar: None,
				ctdb: None,
			});
		}

		Ok(Self {
			now: Instant::now(),
			disc,
			opts,
			tracks,
			sectors: total_sectors,
		})
	}

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
	pub(crate) fn rip(&mut self, progress: &Progless, killed: &KillSwitch)
	-> Result<(), RipRipError> {
		let toc = self.disc.toc();
		let _res = progress.reset(self.sectors * u32::from(self.opts.passes()));
		let mut share = RipShare::new(self.disc, progress, killed);

		// Load the first track's state.
		let (_, first_track) = self.tracks.first_key_value().ok_or(RipRipError::Noop)?;
		set_progress_title(progress, first_track.track.number(), "Initializing…");
		let mut state = RipState::from_track(
			toc,
			first_track.track,
			self.opts.resume(), // Only false on the first pass.
			self.opts.reset(), // Only true on the first pass.
		)?;

		for pass in 1..=self.opts.passes() {
			// Fire up the log if we're logging.
			if self.opts.verbose() { share.log.bump_pass(); }
			share.bump_pass(&self.opts);

			for entry in self.tracks.values_mut() {
				// Skip the work if we aborted or already confirmed the track
				// is complete.
				if killed.killed() || entry.skippable() {
					progress.increment_n(entry.sectors);
					continue;
				}

				// Switch states if needed.
				if state.track() != entry.track {
					set_progress_title(progress, entry.track.number(), "Initializing…");
					state = RipState::from_track(
						toc,
						entry.track,
						pass != 1 || self.opts.resume(), // Only false on the first pass.
						pass == 1 && self.opts.reset(), // Only true on the first pass.
					)?;
				}

				// Run some initial tests to see if we need to do anything
				// further.
				if entry.quality.is_none() {
					entry.update_quality(&state, &self.opts);
				}

				// Actual rip.
				entry.rip(&mut share, &mut state, &self.opts)?;
			}

			// Flip the read order for next time?
			if self.opts.flip_flop() {
				self.opts = self.opts.with_backwards(! self.opts.backwards());
			}
		}

		progress.finish();
		Ok(())
	}
}

impl<'a> Ripper<'a> {
	/// # Summarize.
	///
	/// Print a colored bar, some numbers, and a status for the rip as a whole.
	/// This is displayed along with the `Disc` summary details once all work
	/// has completed.
	pub(crate) fn summarize(&self) {
		// Add up the totals
		let Some((q1, q2)) = self.tracks.values()
			.filter_map(|t| t.quality)
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
		let (legend_a, legend_b) = q2.legend(&q1);
		if let Some(legend_a) = legend_a { eprintln!("        {legend_a}"); }
		eprintln!("        {legend_b} \x1b[2msamples\x1b[0m");

		// An extra line to give some separation between this task and the
		// next.
		eprintln!();
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
					if k == 0 && v.quality.map_or(false, |(_, q)| q.is_likely()) { Some((u8::MAX, u8::MAX))}
					else { v.ar.filter(|&(v1, v2)| conf <= v1 || conf <= v2) };
				let ctdb =
					if k == 0 && v.quality.map_or(false, |(_, q)| q.is_likely()) { Some(u16::MAX) }
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
	dst: Option<PathBuf>,
	track: Track,
	sectors: u32,
	quality: Option<(TrackQuality, TrackQuality)>,
	ar: Option<(u8, u8)>,
	ctdb: Option<u16>,
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
	/// ## Errors
	///
	/// This will bubble up any errors encountered, except run-of-the-mill
	/// sector read or sync errors, which are simply recorded to the state as
	/// "bad" and/or skipped.
	fn rip(&mut self, share: &mut RipShare, state: &mut RipState, opts: &RipOptions)
	-> Result<(), RipRipError> {
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
		let lsn_start = rip_rng.start;
		let dst_rng = rip_distance_iter(&rip_rng, opts.offset(), opts.backwards());

		for k in dst_rng {
			let read_lsn = lsn_start + k;
			let sector = state.offset_sector_mut(read_lsn, opts.offset())?;

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
		self.update_quality(state, opts);
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

		Ok(())
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
	fn skippable(&self) -> bool {
		self.dst.is_some() && self.quality.map_or(false, |(_, q)| q.is_confirmed())
	}

	/// # Verify Entry.
	///
	/// Unless this is the HTOA track, this will try to match the rip against
	/// the AccurateRip and CUETools databases, updating the match counts, if
	/// any. It will also mark the track as confirmed if it is, allowing us to
	/// skip any further work on it.
	///
	/// This will return `true` if verified.
	fn verify(&mut self, state: &mut RipState, opts: &RipOptions, progress: &Progless)
	-> bool {
		set_progress_title(progress, self.track.number(), "Verifying the rip…");

		// HTOA isn't verifiable. Boo.
		if self.track.is_htoa() { return false; }

		// Check AccurateRip and CTDB in separate threads.
		std::thread::scope(|s| {
			let ar = s.spawn(|| chk_accuraterip(
				state.toc(),
				self.track,
				state.track_slice(),
			));
			let ctdb = s.spawn(|| chk_ctdb(
				state.toc(),
				self.track,
				state.rip_slice(),
			));
			self.ar = ar.join().ok().flatten();
			self.ctdb = ctdb.join().ok().flatten();
		});

		// If we're confirmed and the state isn't, update the state and our
		// quality snapshot.
		let conf = opts.confidence();
		let verified =
			self.ar.map_or(false, |(v1, v2)| conf <= v1 || conf <= v2) ||
			self.ctdb.map_or(false, |v| u16::from(conf) <= v);
		if verified && ! self.quality.map_or(false, |(_, q)| q.is_confirmed()) {
			state.confirm_track();
			self.update_quality(state, opts);
		}

		// Return the answer.
		verified
	}

	/// # Update Quality.
	///
	/// Store the latest and greatest quality counts to the second slot, or if
	/// no quality has been recorded yet, to both slots.
	///
	/// This is its own method only because it is kind of annoying to write to
	/// only _part_ of an `Option<(a, b)>` pair. Haha.
	fn update_quality(&mut self, state: &RipState, opts: &RipOptions) {
		let new = state.track_quality(opts.rereads());
		match self.quality.as_mut() {
			Some((_, q2)) => { *q2 = new; },
			None => { self.quality = Some((new, new)); },
		}
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
	buf: RipBuffer,
	log: RipLog,
	leadout: i32,
	pass: u8,
	pass_reads: u32,
	force_bust: bool,
	last_read_track: u8,
	cdio: &'a LibcdioInstance,
	progress: &'a Progless,
	killed: &'a KillSwitch,
}

impl<'a> RipShare<'a> {
	#[allow(clippy::cast_possible_wrap)]
	/// # New Instance.
	const fn new(disc: &'a Disc, progress: &'a Progless, killed: &'a KillSwitch) -> Self {
		Self {
			buf: RipBuffer::new(),
			log: RipLog::new(),
			leadout: disc.toc().audio_leadout() as i32,
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
		// Force a cache bust if we didn't read enough during the previous pass.
		if self.pass != 0 {
			let len = opts.cache_sectors();
			self.force_bust = len != 0 && self.pass_reads < len;
			self.pass_reads = 0;
		}

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



/// # Prune Invalid Tracks.
///
/// Make sure all tracks in the options are actually part of the disc, and
/// print warnings if not.
///
/// If for some reason every track is invalid, an error will be returned.
fn prune_tracks(old: &RipOptions, toc: &Toc) -> Result<RipOptions, RipRipError> {
	let mut new = *old;
	for t in old.tracks() {
		if t == 0 {
			if toc.htoa().is_none() {
				new = new.without_track(0);
				Msg::warning("This disc does not have an HTOA.").eprint();
			}
		}
		else if toc.audio_track(usize::from(t)).is_none() {
			new = new.without_track(t);
			Msg::warning(format!("This disc does not have a track #{t}.")).eprint();
		}
	}

	if new.has_tracks() { Ok(new) }
	else { Err(RipRipError::Noop) }
}

/// # Rip Distance Iter.
///
/// Depending on the read offset, some of the edgiest padding regions may not
/// be readable and/or writable. This returns an iterator of safe distances
/// from the starting LSN where both can happen.
fn rip_distance_iter(rng: &Range<i32>, offset: ReadOffset, backwards: bool)
-> ReadIter {
	let mut rng_start: i32 = 0;
	let mut rng_end: i32 = rng.end - rng.start;
	let sectors_abs = i32::from(offset.sectors_abs());

	// Negative offsets require the data be pushed forward to "start"
	// at the right place.
	if offset.is_negative() { rng_end -= sectors_abs; }
	// Positive offsets require the data be pulled backward instead.
	else { rng_start += sectors_abs; }

	ReadIter::new(rng_start, rng_end, backwards)
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
