/*!
# Rip Rip Hooray: Ripping
*/

use cdtoc::{
	AccurateRip,
	Toc,
	Track,
};
use crate::{
	BYTES_PER_SAMPLE,
	cache_read,
	cache_write,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
	CD_LEADIN,
	chk_accuraterip,
	chk_ctdb,
	Disc,
	KillSwitch,
	NULL_SAMPLE,
	ReadOffset,
	RipRipError,
	Sample,
	SAMPLES_PER_SECTOR,
};
use dactyl::NicePercent;
use fyi_msg::{
	Msg,
	Progless,
};
use hound::{
	SampleFormat,
	WavSpec,
	WavWriter,
};
use serde::{
	Serialize,
	Deserialize,
};
use std::{
	io::Cursor,
	ops::Range,
	time::Duration,
};



/// # FLAG: C2 Support.
const FLAG_C2: u8 =        0b0001;

/// # FLAG: RAW PCM (instead of WAV).
const FLAG_RAW: u8 =       0b0010;

/// # FLAG: Reconfirm samples.
const FLAG_RECONFIRM: u8 = 0b0100;

/// # FLAG: Default.
const FLAG_DEFAULT: u8 = FLAG_C2;

/// # Quality Bar.
const QUALITY_BAR: &str = "########################################################################";

/// # Extra Sector Reads.
///
/// To account for potential read offset variation, all tracks are under- and
/// overread by ten sectors. (The appropriate portion is cut out when saving
/// the track.)
const SECTOR_BUFFER: u32 = 10;

/// # Extra Sample Reads.
///
/// Same as the sector buffer, but in samples.
const SAMPLE_BUFFER: u32 = SECTOR_BUFFER * SAMPLES_PER_SECTOR;

/// # Sleep Time.
///
/// Pause between repeated passes over the same track.
const SLEEP: Duration = Duration::from_secs(5);

/// # C2 Sample Set.
///
/// This contains a `bool` for each sample in a sector indicating whether or
/// not it contains an error.
type SectorC2s = [bool; SAMPLES_PER_SECTOR as usize];



#[derive(Debug, Clone)]
/// # Rip Options.
///
/// This struct holds the rip-related options like read offset, paranoia level,
/// which tracks to focus on, etc.
///
/// Options are set using builder-style methods, like:
///
/// ```
/// use riprip_core::RipOptions;
///
/// let opts = RipOptions::default()
///     .with_refine(3)
///     .with_tracks([1, 2, 3]);
///
/// assert_eq!(opts.refine(), 3);
/// assert_eq!(opts.tracks(), &[1, 2, 3]);
/// ```
pub struct RipOptions {
	offset: ReadOffset,
	paranoia: u8,
	refine: u8,
	flags: u8,
	tracks: Vec<u8>,
}

impl Default for RipOptions {
	fn default() -> Self {
		Self {
			offset: ReadOffset::default(),
			paranoia: 3,
			refine: 0,
			flags: FLAG_DEFAULT,
			tracks: Vec::new(),
		}
	}
}

impl RipOptions {
	#[must_use]
	/// # With Offset.
	///
	/// Set the AccurateRip, _et al_, drive read offset to apply when copying
	/// data from the disc. See [here](http://www.accuraterip.com/driveoffsets.htm) for more information.
	///
	/// It is critical the correct offset be applied, otherwise the contents of
	/// the rip may not be independently verifiable. This is doubly so when two
	/// or more drives are used for a single rip; without appropriate offsets
	/// the communal data could be corrupted.
	///
	/// The default is zero.
	pub fn with_offset(self, offset: ReadOffset) -> Self {
		Self {
			offset,
			..self
		}
	}

	#[must_use]
	/// # With C2 Error Pointers.
	///
	/// Enable or disable the use of C2 error pointer information.
	///
	/// This feature is critical for ensuring any degree of transfer accuracy,
	/// but if a drive doesn't support it, it should be disabled.
	///
	/// The default is enabled.
	pub fn with_c2(self, c2: bool) -> Self {
		let flags =
			if c2 { self.flags | FLAG_C2 }
			else { self.flags & ! FLAG_C2 };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Paranoia Level.
	///
	/// Whenever a drive reports _any_ C2 or read errors for a block, consider
	/// _all_ samples in that block — namely the allegedly good ones — as
	/// suspicious until the same values have been returned this many times.
	///
	/// The default is three.
	///
	/// Custom values are automatically capped at `1..=32`.
	pub fn with_paranoia(self, mut paranoia: u8) -> Self {
		if paranoia == 0 { paranoia = 1; }
		else if paranoia > 32 { paranoia = 32; }
		Self {
			paranoia,
			..self
		}
	}

	#[must_use]
	/// # With Raw PCM Output.
	///
	/// When `true`, tracks will be exported in raw PCM format. When `false`,
	/// they'll be saved as WAV files instead.
	///
	/// The default is `false`.
	pub fn with_raw(self, raw: bool) -> Self {
		let flags =
			if raw { self.flags | FLAG_RAW }
			else { self.flags & ! FLAG_RAW };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Reconfirmation.
	///
	/// If true, previously-accepted samples will be "downgraded" to
	/// "suspicious", requring reconfirmation from subsequent reads.
	///
	/// The default is disabled.
	pub fn with_reconfirm(self, reconfirm: bool) -> Self {
		let flags =
			if reconfirm { self.flags | FLAG_RECONFIRM }
			else { self.flags & ! FLAG_RECONFIRM };

		Self {
			flags,
			..self
		}
	}

	#[must_use]
	/// # With Refine Passes.
	///
	/// Execute this many additional rip passes so long as any samples remain
	/// unread or unconfirmed. This is equivalent to re-running the entire
	/// program X number of times, but saves you the trouble of having to do
	/// that.
	///
	/// The default is zero; the max is `15`, just to give the drive a break.
	pub fn with_refine(self, mut refine: u8) -> Self {
		if refine > 15 { refine = 15; }
		Self {
			refine,
			..self
		}
	}

	#[must_use]
	/// # With Tracks.
	///
	/// Set the tracks-of-interest by their indexes. If empty, all audio tracks
	/// on the disc will be ripped.
	///
	/// The default is all tracks.
	pub fn with_tracks<I>(mut self, iter: I) -> Self
	where I: IntoIterator<Item=u8> {
		self.tracks.truncate(0);
		self.tracks.extend(iter);
		self.tracks.sort_unstable();
		self.tracks.dedup();
		self
	}
}

impl RipOptions {
	#[must_use]
	/// # Offset.
	pub const fn offset(&self) -> ReadOffset { self.offset }

	#[must_use]
	/// # Use C2 Error Pointers?
	pub const fn c2(&self) -> bool { FLAG_C2 == self.flags & FLAG_C2 }

	#[must_use]
	/// # Paranoia Level.
	pub const fn paranoia(&self) -> u8 { self.paranoia }

	#[must_use]
	/// # Number of Passes.
	///
	/// Return the total number of passes, e.g. `1 + refine`.
	pub const fn passes(&self) -> u8 { self.refine() + 1 }

	#[must_use]
	/// # Save as Raw PCM?
	pub const fn raw(&self) -> bool { FLAG_RAW == self.flags & FLAG_RAW }

	#[must_use]
	/// # Require Reconfirmation?
	pub const fn reconfirm(&self) -> bool { FLAG_RECONFIRM == self.flags & FLAG_RECONFIRM }

	#[must_use]
	/// # Number of Refine Passes.
	pub const fn refine(&self) -> u8 { self.refine }

	#[must_use]
	/// # Tracks.
	pub fn tracks(&self) -> &[u8] { &self.tracks }
}



#[derive(Debug)]
/// # A Rip!
///
/// This struct represents a rip-in-progress. It holds the data gathered, as
/// well as various state information. Its eponymous `Rip::rip` method handles
/// the actual ripping.
pub(super) struct Rip {
	ar: AccurateRip,
	track: Track,
	rip_lsn: Range<i32>, // The track range with 10 extra sectors on either end.
	state: Vec<RipSample>,
}

impl Rip {
	#[allow(clippy::cast_possible_wrap)] // These are known constants; they fit.
	/// # New.
	///
	/// Prepare — but do not execte — a new rip for the track. The AccurateRip
	/// ID is used to prevent collisions with state data between different
	/// discs (in the event multiple rips are run from the same CWD without
	/// cleanup).
	///
	/// This will look for and load a previous rip state if it exists. If for
	/// any reason the numbers don't work out, it will prompt to see if you
	/// want to start over or abort.
	///
	/// Ripping requires an annoying large number of casts between arbitrary
	/// numeric types. This method pre-tests those conversions so we know
	/// everything will fit each type.
	///
	/// ## Errors
	///
	/// This will return errors if the numbers can't be converted between the
	/// necessary types, cache errors are encountered, or the data cannot be
	/// initialized.
	pub(super) fn new(ar: AccurateRip, track: Track) -> Result<Self, RipRipError> {
		let idx = track.number();
		let rng = track.sector_range_normalized();

		// Make sure the range fits i32.
		let track_lsn =
			i32::try_from(rng.start).map_err(|_| RipRipError::RipOverflow(idx))?..
			i32::try_from(rng.end).map_err(|_| RipRipError::RipOverflow(idx))?;

		// Make sure we can add the buffer to each end too.
		let rip_lsn =
			track_lsn.start.checked_sub(SECTOR_BUFFER as i32).ok_or(RipRipError::RipOverflow(idx))?..
			track_lsn.end.checked_add(SECTOR_BUFFER as i32).ok_or(RipRipError::RipOverflow(idx))?;

		// Make sure the range in samples fits i32, u32, and usize.
		let expected_len = (rip_lsn.end - rip_lsn.start).checked_mul(SAMPLES_PER_SECTOR as i32)
			.and_then(|v| u32::try_from(v).ok())
			.and_then(|v| usize::try_from(v).ok())
			.ok_or(RipRipError::RipOverflow(idx))?;

		// Do we have an existing copy to resume?
		let mut state = Vec::new();
		if let Some(old) = cache_read(state_path(ar, idx))? {
			// Make sure it makes sense.
			let old = bincode::deserialize::<Vec<RipSample>>(&old);
			if old.as_ref().map_or(true, |o| o.len() != expected_len) {
				Msg::warning(format!("The state data for track #{idx} is corrupt.")).eprint();
				if ! fyi_msg::confirm!(yes: "Do you want to start over?") {
					return Err(RipRipError::Killed);
				}
			}

			// Use it if it's good!
			if let Ok(old) = old {
				if old.len() == expected_len { state = old; }
			}
		}

		// Fix the sizing if necessary.
		if state.len() != expected_len {
			state.truncate(0);
			state.try_reserve(expected_len).map_err(|_| RipRipError::RipOverflow(idx))?;
			state.resize(expected_len, RipSample::Tbd);
		}

		Ok(Self { ar, track, rip_lsn, state })
	}
}

impl Rip {
	/// # Rip a Track!
	///
	/// Actually rip the data!
	pub(super) fn rip(
		&mut self,
		disc: &Disc,
		opts: &RipOptions,
		progress: &Progless,
		killed: &KillSwitch,
	) -> Result<Option<String>, RipRipError> {
		// If we're resuming, we might need to "upgrade" previous iffy entries
		// to meet a lower paranoia requirement.
		let paranoia = opts.paranoia();
		for sample in &mut self.state {
			if let RipSample::Iffy(set) = sample {
				if paranoia <= set[0].1 {
					*sample = RipSample::Good(set[0].0);
				}
			}
		}

		// If we're reconfirming, we might need to "downgrade" previous good
		// entries to require their reconfirmation. In such cases, we'll start
		// the count at one below the paranoia level.
		if 1 < paranoia && opts.reconfirm() {
			let count = paranoia - 1;
			for sample in &mut self.state {
				if let RipSample::Good(nope) = sample {
					*sample = RipSample::Iffy(vec![(*nope, count)]);
				}
			}
		}

		if ! killed.killed() {
			// Same method two ways. The only difference is the buffer size;
			// a larger buffer is required for C2 when ripping without.
			if opts.c2() {
				let mut buf = [0_u8; CD_DATA_C2_SIZE as usize];
				self._rip(disc, opts, &mut buf, progress, killed)?;
			}
			else {
				let mut buf = [0_u8; CD_DATA_SIZE as usize];
				self._rip(disc, opts, &mut buf, progress, killed)?;
			}
		}

		// Lastly, save the track!
		let dst = self.extract(opts.raw())?;
		Ok(Some(dst))
	}

	#[allow(clippy::cast_possible_truncation, clippy::cast_possible_wrap)]
	/// # Actual Rip (for real this time).
	///
	/// This is separated from `Rip::rip` to allow for different fixed buffer
	/// sizes. We have to initialize those ahead of time to keep Rust happy.
	fn _rip(
		&mut self,
		disc: &Disc,
		opts: &RipOptions,
		buf: &mut [u8],
		progress: &Progless,
		killed: &KillSwitch,
	) -> Result<(), RipRipError> {
		// Lots of variables!
		let offset = opts.offset();
		let resume = u8::from(self.state.iter().any(RipSample::is_good));
		let (min_sector, max_sector) = self.rip_distance(offset);
		let state_path = state_path(self.ar, self.track.number());
		let mut c2: SectorC2s = [false; SAMPLES_PER_SECTOR as usize];
		let leadout = disc.toc().audio_leadout() as i32 - CD_LEADIN as i32;

		// Onto the pass(es)!
		for pass in 0..opts.passes() {
			if pass != 0 { std::thread::sleep(SLEEP); }

			// Update/reset the progress bar.
			let _res = progress.reset((max_sector - min_sector) as u32); // This won't fail.
			progress.set_title(Some(Msg::custom(
				rip_title(pass + resume),
				199,
				&format!("Track #{}…", self.track.number())
			)));

			// Update the data, one sector at a time.
			for k in min_sector..max_sector {
				// Cut out the offset-adjusted portion of the state
				// corresponding to the sector being read. (We'll likely write
				// data a little earlier or later than we read it.)
				let state_start =
					if offset.is_negative() { k * SAMPLES_PER_SECTOR as usize + offset.samples_abs() as usize }
					else { k * SAMPLES_PER_SECTOR as usize - offset.samples_abs() as usize };
				let state = &mut self.state[state_start..=state_start + 588];

				// Skip the range if we're done or there's nothing to refine.
				if killed.killed() || state.iter().all(RipSample::is_good) {
					progress.increment();
					continue;
				}

				// The starting LSN for this section.
				let lsn = self.rip_lsn.start + k as i32;

				// If this LSN is unreadable, we can assume the data is null
				// and save ourselves the trouble of actually reading from the
				// disc.
				if lsn < 0 || lsn >= leadout {
					for sample in &mut *state {
						*sample = RipSample::Good(NULL_SAMPLE);
					}
					progress.increment();
					continue;
				}

				// Otherwise we have to actually talk to the drive. Ug.
				match disc.cdio().read_cd(buf, lsn) {
					Ok(()) =>
						// Parse the C2 data. Each bit represents one byte of
						// audio data, but it's silly to zoom so far down;
						// we'll treat sample pairs as pass/fail, quartering
						// the effort.
						if opts.c2() {
							for (k2, &v) in c2.chunks_exact_mut(2).zip(&buf[CD_DATA_SIZE as usize..]) {
								k2[0] = 0 != v & 0b1111_0000;
								k2[1] = 0 != v & 0b0000_1111;
							}
						}
						// Assume C2 is fine since we aren't asking for that
						// data.
						else { reset_c2(&mut c2, false); },
					// Assume total C2 failure if there's a hard read error.
					Err(RipRipError::CdRead(_)) => { reset_c2(&mut c2, true); },
					// Other kinds of errors are show-stoppers; abort!
					Err(e) => return Err(e),
				}

				// Any C2 issues across the whole sector?
				let sector_c2 = c2.iter().any(|v| *v);

				// Patch the data!
				for ((old, new), sample_c2) in state.iter_mut()
					.zip(buf[..CD_DATA_SIZE as usize].chunks_exact(4))
					.zip(c2.iter().copied()) {
					if let Ok(new) = Sample::try_from(new) {
						old.update(new, opts.paranoia(), sample_c2, sector_c2);
					}
				}

				progress.increment();
			}

			// Summarize the approximate quality.
			progress.finish();
			let (mut q_good, mut q_maybe, q_bad) = self.track_quality();

			// If the data is decent, see if the track matches third-party
			// checksum databases (for added assurance).
			let (ar, ctdb) =
				if q_bad == 0 { self.verify(disc.toc()) }
				else { (None, None) };
			let verified =
				ar.map_or(false, |(v1, v2)| v1 != 0 || v2 != 0) ||
				ctdb.map_or(false, |v| 0 != v);

			// If the track matched, we can upgrade the maybes.
			if verified && 0 != q_maybe {
				q_good += q_maybe;
				q_maybe = 0;
				let rng = self.track_range();
				for sample in &mut self.state[rng] {
					if let RipSample::Iffy(set) = sample {
						*sample = RipSample::Good(set[0].0);
					}
				}
			}

			// Okay, *now* we're summarizing!
			if verified {
				Msg::custom("Ripped", 10, &format!(
					"Track #{} has been accurately ripped!",
					self.track.number(),
				))
			}
			else {
				let p1 = dactyl::int_div_float(q_good, q_good + q_maybe + q_bad).unwrap_or(0.0);
				Msg::custom("Ripped", 10, &format!(
					"Track #{} is \x1b[2m(roughly)\x1b[0m {} complete.",
					self.track.number(),
					NicePercent::from(p1),
				))
			}
				.with_newline(true)
				.eprint();

			// Inject a graphical-ish breakdown too for beauty.
			print_bar(q_good, q_maybe, q_bad, ar, ctdb);

			// Save the state file.
			if bincode::serialize(&self.state).ok()
				.and_then(|out| cache_write(&state_path, &out).ok())
				.is_none()
			{
				Msg::warning("The rip state couldn't be saved.").eprint();
			}

			// Should we stop early?
			if killed.killed() || self.track_good() { break; }
		}

		Ok(())
	}

	#[allow(clippy::cast_possible_truncation)]
	/// # Extract the Track.
	///
	/// This extracts and saves the offset-adjusted track — using the best data
	/// available — to disk in either raw PCM or WAV format.
	///
	/// It returns the destination path used for reference.
	///
	/// ## Errors
	///
	/// This will bubble up any file I/O-type errors encountered.
	fn extract(&self, raw: bool) -> Result<String, RipRipError> {
		let dst = rip_path(self.track.number(), raw);
		let rng = self.track_range();

		// Raw is easy; we just need to flatten the samples.
		if raw {
			let mut flat: Vec<u8> = Vec::with_capacity((rng.end - rng.start) * BYTES_PER_SAMPLE as usize);
			for v in &self.state[rng] {
				flat.extend_from_slice(v.as_slice());
			}
			cache_write(&dst, &flat)?;
		}
		// Wav requires _headers_ and shit.
		else {
			let spec = WavSpec {
				channels: 2,
				sample_rate: 44100,
				bits_per_sample: 16,
				sample_format: SampleFormat::Int,
			};
			let mut buf = Cursor::new(Vec::with_capacity((rng.end - rng.start) * BYTES_PER_SAMPLE as usize + 44));
			let mut wav = WavWriter::new(&mut buf, spec)
				.map_err(|_| RipRipError::Write(dst.clone()))?;

			// In CD contexts, a sample is general one L+R pair. In other
			// contexts, like hound, L and R are each their own sample.
			// (We need to multiple our internal count by 2 to match.)
			{
				let mut writer = wav.get_i16_writer((rng.end - rng.start) as u32 * 2);
				for sample in &self.state[rng] {
					let sample = sample.as_array();
					writer.write_sample(i16::from_le_bytes([sample[0], sample[1]]));
					writer.write_sample(i16::from_le_bytes([sample[2], sample[3]]));
				}
				writer.flush().map_err(|_| RipRipError::Write(dst.clone()))?;
			}

			wav.flush().ok()
				.and_then(|_| wav.finalize().ok())
				.and_then(|_| cache_write(&dst, &buf.into_inner()).ok())
				.ok_or_else(|| RipRipError::Write(dst.clone()))?;
		}

		Ok(dst)
	}

	/// # Verify Rip!
	///
	/// Check the rip against the AccurateRip and CUETools databases and return
	/// the confidences, if any.
	///
	/// AccurateRip has two different algorithms for historical reasons, hence
	/// two different confidences. They also stop counting at 99, so we don't
	/// need anything bigger than a `u8`.
	fn verify(&self, toc: &Toc) -> (Option<(u8, u8)>, Option<u16>) {
		let state = self.track_slice();
		let ar = chk_accuraterip(self.ar, self.track, state);
		let ctdb = chk_ctdb(toc, self.ar, self.track, state);
		(ar, ctdb)
	}
}

impl Rip {
	#[allow(clippy::integer_division)]
	/// # Rippable Sectors.
	///
	/// "Offsets" basically mean that when we read data at, say, LSN/sample 0,
	/// we'd need to record them as having come from, say, +667 samples. The 10
	/// sector padding we're keeping ensures we never throw away read data, but
	/// we can't necessarily fill them all the way to the edges either.
	///
	/// This returns the minimum and maximum sector distance from `rip_lsn.0`
	/// that can be both read and written, given the offset.
	///
	/// Regardless of how much is skipped, the complete track data in the
	/// middle will always get covered.
	fn rip_distance(&self, offset: ReadOffset) -> (usize, usize) {
		let mut min_sector: usize = 0;
		let mut max_sector: usize = self.state.len() / SAMPLES_PER_SECTOR as usize;
		let sectors_abs = offset.sectors_abs() as usize;

		// Negative offsets require the data be pushed forward to "start"
		// at the right place.
		if offset.is_negative() { max_sector -= sectors_abs; }
		// Positive offsets require the data be pulled backward instead.
		else { min_sector += sectors_abs; }

		(min_sector, max_sector)
	}

	/// # Track All Good?
	///
	/// Returns `true` if all samples within the track range are good.
	fn track_good(&self) -> bool {
		self.track_slice().iter().all(RipSample::is_good)
	}

	/// # Track Quality.
	///
	/// Return the number of good, maybe, and bad samples within the track
	/// range.
	fn track_quality(&self) -> (usize, usize, usize) {
		let mut good = 0;
		let mut maybe = 0;
		let mut bad = 0;
		for v in self.track_slice() {
			match v {
				RipSample::Good(_) => { good += 1; },
				RipSample::Iffy(_) => { maybe += 1; },
				_ => { bad += 1; },
			}
		}

		(good, maybe, bad)
	}

	/// # Track Range.
	///
	/// Return the (state) index range corresponding to the actual track.
	fn track_range(&self) -> Range<usize> {
		SAMPLE_BUFFER as usize..self.state.len() - SAMPLE_BUFFER as usize
	}

	/// # Track Slice.
	///
	/// Return the slice of `self.state` comprising the track.
	fn track_slice(&self) -> &[RipSample] {
		let rng = self.track_range();
		&self.state[rng]
	}
}



#[derive(Debug, Clone, Default, Deserialize, Serialize)]
/// # Rip Sample.
///
/// This is a combined sample/status structure, making it easy to know where
/// any given sample stands at a glance.
///
/// This is almost but not quite `Copy` because we have to store an unknown
/// number of samples for `Self::Iffy`. Oh well.
pub(super) enum RipSample {
	#[default]
	/// # Not yet read.
	Tbd,

	/// # The drive gave us something but said it was bad.
	Bad(Sample),

	/// # Sample(s) awaiting paranoia confirmation.
	///
	/// Iffy samples are sorted by popularity (most to least), so the first
	/// entry is always the "best", relatively speaking.
	Iffy(Vec<(Sample, u8)>),

	/// # It should be good!
	Good(Sample),
}

impl RipSample {
	/// # As Slice.
	///
	/// Return the most appropriate single 4-byte sample as a slice.
	pub(super) fn as_slice(&self) -> &[u8] {
		match self {
			Self::Tbd => NULL_SAMPLE.as_slice(),
			Self::Bad(s) | Self::Good(s) => s.as_slice(),
			Self::Iffy(ref s) => s[0].0.as_slice(),
		}
	}

	/// # As Array.
	pub(super) fn as_array(&self) -> [u8; 4] {
		match self {
			Self::Tbd => NULL_SAMPLE,
			Self::Bad(s) | Self::Good(s) => *s,
			Self::Iffy(ref s) => s[0].0,
		}
	}

	/// # Is Good?
	const fn is_good(&self) -> bool { matches!(self, Self::Good(_)) }

	/// # Update.
	///
	/// Potentially update an entry.
	///
	/// Good entries don't change.
	///
	/// TBD entries _always_ change:
	/// * If `sample_c2`, to Bad
	/// * If `sector_c2` and `paranoia`, to Iffy
	/// * Otherwise to Good
	///
	/// Bad samples change if not `sample_c2`:
	/// * If `paranoia`, to Iffy
	/// * Otherwise to Good
	///
	/// Iffy samples change if not `sample_c2`:
	/// * If confirmed `paranoia` times, to Good
	/// * Otherwise still Iffy, but with updated table
	fn update(&mut self, new: Sample, paranoia: u8, sample_c2: bool, sector_c2: bool) {
		match self {
			// Leave good entries alone.
			Self::Good(_) => {},

			// Always update a TBD.
			Self::Tbd =>
				if sample_c2 { *self = Self::Bad(new); }
				else if sector_c2 && 1 < paranoia { *self = Self::Iffy(vec![(new, 1)]); }
				else { *self = Self::Good(new); },

			// Bad can only move to iffy, unless there's no paranoia to apply.
			Self::Bad(_) => if ! sample_c2 {
				if 1 < paranoia { *self = Self::Iffy(vec![(new, 1)]); }
				else { *self = Self::Good(new); }
			},

			// Iffy entries are a little more involved.
			Self::Iffy(set) => if ! sample_c2 {
				// See if the sample is in the set.
				let mut found = false;
				for (old, count) in &mut *set {
					if new.eq(old) {
						*count += 1;
						found = true;
						if *count >= paranoia {
							*self = Self::Good(new);
							return;
						}
						break;
					}
				}

				// It's new.
				if ! found { set.push((new, 1)); }

				// Sort by popularity, most to least.
				set.sort_unstable_by(|a, b| b.1.cmp(&a.1));
			},
		}
	}
}



#[allow(
	clippy::cast_possible_truncation,
	clippy::cast_precision_loss,
	clippy::cast_sign_loss,
)]
/// # Print Quality Bar.
///
/// This presents the final quality of a rip as a colored bar, with colored
/// labels. It also appends the AccurateRip/CUETools results, if any.
fn print_bar(
	good: usize,
	maybe: usize,
	bad: usize,
	ar: Option<(u8, u8)>,
	ctdb: Option<u16>,
) {
	let all = good + maybe + bad;
	let b_total = QUALITY_BAR.len() as f64;
	let b_maybe =
		if maybe == 0 { 0 }
		else {
			usize::max(1, (dactyl::int_div_float(maybe, all).unwrap_or(0.0) * b_total).floor() as usize)
		};
	let b_bad =
		if bad == 0 { 0 }
		else {
			usize::max(1, (dactyl::int_div_float(bad, all).unwrap_or(0.0) * b_total).floor() as usize)
		};
	let b_good = QUALITY_BAR.len() - b_maybe - b_bad;
	eprintln!(
		"        \x1b[1;91m{}\x1b[0;1;93m{}\x1b[0;1;92m{}\x1b[0m",
		&QUALITY_BAR[..b_bad],
		&QUALITY_BAR[..b_maybe],
		&QUALITY_BAR[..b_good],
	);

	let mut breakdown = Vec::with_capacity(3);
	if 0 != bad { breakdown.push(format!("\x1b[91m{bad}\x1b[0m")); }
	if 0 != maybe { breakdown.push(format!("\x1b[93m{maybe}\x1b[0m")); }
	if 0 != good { breakdown.push(format!("\x1b[92m{good}\x1b[0m")); }
	if ! breakdown.is_empty() {
		eprintln!("        {} \x1b[2msamples\x1b[0m", breakdown.join(" \x1b[2m+\x1b[0m "));
	}

	if ar.is_some() || ctdb.is_some() {
		eprintln!("        \x1b[38;5;4m-----\x1b[0m");

		if let Some((v1, v2)) = ar {
			if v1 == 0 && v2 == 0 {
				eprintln!("        AccurateRip: \x1b[91m00+00\x1b[0m");
			}
			else {
				eprintln!(
					"        AccurateRip: \x1b[92m{:02}+{:02}\x1b[0m",
					u8::min(99, v1),
					u8::min(99, v2),
				);
			}
		}

		if let Some(v) = ctdb {
			if v == 0 {
				eprintln!("        CUETools: \x1b[91m000\x1b[0m");
			}
			else {
				eprintln!(
					"        CUETools: \x1b[92m{:03}\x1b[0m",
					u16::min(999, v),
				);
			}
		}
	}

	eprintln!();
}

#[inline]
/// # Reset C2 Statuses.
///
/// Change all C2 status to `val`.
fn reset_c2(set: &mut SectorC2s, val: bool) {
	for c2 in set { *c2 = val; }
}

/// # Extraction Path.
///
/// Return the relative path to use for the ripped track.
fn rip_path(idx: u8, raw: bool) -> String {
	if raw { format!("{idx:02}.pcm") }
	else   { format!("{idx:02}.wav") }
}

#[inline]
/// # Rip Title.
///
/// Return the title to use for the progress bar. This is based on the number
/// of passes.
const fn rip_title(pass: u8) -> &'static str {
	match pass {
		0 => "Ripping",
		1 => "Re-Ripping",
		2 => "Re-Re-Ripping",
		3 => "Re-Re-Re-Ripping",
		_ => "Re-Re-Re-Etc.-Ripping",
	}
}

/// # State Path.
///
/// Return the relative path to use for the track's state file.
fn state_path(ar: AccurateRip, idx: u8) -> String {
	format!("state/{ar}__{idx:02}.state")
}



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_rip_options_c2() {
		for v in [false, true] {
			let opts = RipOptions::default().with_c2(v);
			assert_eq!(opts.c2(), v);
		}
	}

	#[test]
	fn t_rip_options_offset() {
		let offset5 = ReadOffset::try_from(b"5".as_slice()).expect("Read offset 5 failed.");
		let offset667 = ReadOffset::try_from(b"-667".as_slice()).expect("Read offset -667 failed.");
		for v in [offset5, offset667] {
			let opts = RipOptions::default().with_offset(v);
			assert_eq!(opts.offset(), v);
		}
	}

	#[test]
	fn t_rip_options_paranoia() {
		for v in [1, 2, 3] {
			let opts = RipOptions::default().with_paranoia(v);
			assert_eq!(opts.paranoia(), v);
		}

		// Min.
		let opts = RipOptions::default().with_paranoia(0);
		assert_eq!(opts.paranoia(), 1);

		// Max.
		let opts = RipOptions::default().with_paranoia(64);
		assert_eq!(opts.paranoia(), 32);
	}

	#[test]
	fn t_rip_options_raw() {
		for v in [false, true] {
			let opts = RipOptions::default().with_raw(v);
			assert_eq!(opts.raw(), v);
		}
	}

	#[test]
	fn t_rip_options_reconfirm() {
		for v in [false, true] {
			let opts = RipOptions::default().with_reconfirm(v);
			assert_eq!(opts.reconfirm(), v);
		}
	}

	#[test]
	fn t_rip_options_refine() {
		for v in [0, 1, 2, 3] {
			let opts = RipOptions::default().with_refine(v);
			assert_eq!(opts.refine(), v);
			assert_eq!(opts.passes(), v + 1);
		}

		// Max.
		let opts = RipOptions::default().with_refine(64);
		assert_eq!(opts.refine(), 15);
		assert_eq!(opts.passes(), 16);
	}

	#[test]
	fn t_rip_options_tracks() {
		let opts = RipOptions::default().with_tracks([1, 5, 5, 2, 3]);
		assert_eq!(opts.tracks(), &[1, 2, 3, 5]);
	}
}
