/*!
# Rip Rip Hooray: Ripping
*/

use cdtoc::{
	AccurateRip,
	Track,
};
use crate::{
	BYTES_PER_SAMPLE,
	cache_read,
	cache_write,
	CD_DATA_C2_SIZE,
	CD_DATA_SIZE,
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



#[derive(Debug, Clone)]
/// # Rip Options.
///
/// This uses builder-style construction. Start with the [RipOptions::default],
/// then chain any desired `with_` methods.
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
	/// Set the `AccurateRip` read offset for the drive. See [here](http://www.accuraterip.com/driveoffsets.htm)
	/// for more information, or use the detection features built into a
	/// program like [fre:ac](https://github.com/enzo1982/freac/) to determine
	/// the appropriate value.
	///
	/// Note: it is critical this be set correctly, particularly when multiple
	/// drives are used to rip-rip the same content.
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
	/// In general, this should only be disabled if a drive does not support
	/// the feature.
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
	/// Whenever a drive reports _any_ C2 errors for a block, consider all
	/// samples in that block suspicious until they have been confirmed this
	/// many times.
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
	/// they'll be saved as WAV files.
	///
	/// The default is false.
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
	/// If true, previously-accepted samples will be downgraded, requring
	/// reconfirmation (from an additional read).
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
	/// unread or unconfirmed.
	///
	/// The default is zero; the max is 15.
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
	/// Set the tracks-of-interest by their indexes. If empty, all tracks will
	/// be scheduled for ripping.
	///
	/// The default is all tracks, but you'll generally want to reserve this
	/// program for recovering _problem tracks_.
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
/// well as various state information.
pub(super) struct Rip {
	ar: AccurateRip,
	idx: u8,
	rip_lsn: Range<i32>, // The track range with 10 extra sectors on either end.
	state: Vec<RipSample>,
}

impl Rip {
	#[allow(clippy::cast_possible_wrap)] // These are known constants; they fit.
	/// # New.
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

		Ok(Self { ar, idx, rip_lsn, state })
	}
}

impl Rip {
	/// # Rip a Track!
	///
	/// This will rip a track, potentially multiple times in a row.
	pub(super) fn rip(
		&mut self,
		disc: &Disc,
		opt: &RipOptions,
		progress: &Progless,
		killed: &KillSwitch,
	) -> Result<Option<String>, RipRipError> {
		// If we're resuming, we might need to "upgrade" previous iffy entries
		// to meet a lower paranoia requirement.
		let paranoia = opt.paranoia();
		for sample in &mut self.state {
			if let RipSample::Iffy(set) = sample {
				if paranoia <= set[0].1 {
					*sample = RipSample::Good(set[0].0);
				}
			}
		}

		// If we're reconfirming, let's also downgrade before we begin.
		if 1 < paranoia && opt.reconfirm() {
			let count = paranoia - 1;
			for sample in &mut self.state {
				if let RipSample::Good(nope) = sample {
					*sample = RipSample::Iffy(vec![(*nope, count)]);
				}
			}
		}

		// The buffer needs to be different sizes depending on whether or not
		// C2 error data is being fetched. To make lives easier, figure that
		// out now and defer to a sub-method.
		if opt.c2() {
			let mut buf = [0_u8; CD_DATA_C2_SIZE as usize];
			self._rip(disc, opt, &mut buf, progress, killed)
		}
		else {
			let mut buf = [0_u8; CD_DATA_SIZE as usize];
			self._rip(disc, opt, &mut buf, progress, killed)
		}
	}

	#[allow(
		clippy::cast_possible_truncation,
		clippy::cast_possible_wrap,
		clippy::integer_division,
	)]
	fn _rip(
		&mut self,
		disc: &Disc,
		opt: &RipOptions,
		buf: &mut [u8],
		progress: &Progless,
		killed: &KillSwitch,
	) -> Result<Option<String>, RipRipError> {
		if killed.killed() { return Ok(None) }

		let mut pass: u8 = 0;
		let resume = u8::from(self.state.iter().any(RipSample::is_good));
		let total = self.state.len() as u32 / SAMPLES_PER_SECTOR;
		let state_path = state_path(self.ar, self.idx);
		let mut c2 = [false; SAMPLES_PER_SECTOR as usize];
		let leadout = disc.toc().audio_leadout() as i32;

		// Onto the pass(es)!
		loop {
			let _res = progress.reset(total); // This won't fail.
			progress.set_title(Some(Msg::custom(
				format!("{}Ripping", "Re-".repeat(usize::min(5, usize::from(pass + resume)))),
				199,
				format!("Track #{}…", self.idx)
			)));
			pass += 1;

			// Update the data, one sector at a time.
			for (k, state) in self.state.chunks_exact_mut(SAMPLES_PER_SECTOR as usize).enumerate() {
				// Skip the range if we're done or there's nothing to refine.
				if killed.killed() || state.iter().all(RipSample::is_good) {
					progress.increment();
					continue;
				}

				// The starting LSN for this section.
				let lsn = self.rip_lsn.start + k as i32;

				// If it is in an unreadable place, assume the whole thing is
				// good, null samples all the way down!
				if lsn < 0 || lsn >= leadout {
					for sample in &mut *state {
						*sample = RipSample::Good(NULL_SAMPLE);
					}
					progress.increment();
					continue;
				}

				// Read it properly.
				match disc.cdio().read_cd(buf, lsn) {
					Ok(()) =>
						// Parse the C2 data. Each bit represents one byte of
						// audio data, but since we're tracking at a sample
						// level, we'll treat 4-bit groups as pass/fail.
						if opt.c2() {
							for (k2, &v) in c2.chunks_exact_mut(2).zip(&buf[CD_DATA_SIZE as usize..]) {
								k2[0] = 0 != v & 0b1111_0000;
								k2[1] = 0 != v & 0b0000_1111;
							}
						}
						// Assume C2 is fine since we aren't asking for any.
						else {
							for v in &mut c2 { *v = false; }
						},
					// Assume total C2 failure.
					Err(RipRipError::CdRead(_)) => {
						for v in &mut c2 { *v = true; }
					},
					// Other errors are show-stoppers; we should abort.
					Err(e) => return Err(e),
				}

				// Any C2 issues across the whole sector?
				let sector_c2 = c2.iter().any(|v| *v);

				// Patch the data!
				for ((old, new), sample_c2) in state.iter_mut()
					.zip(buf[..CD_DATA_SIZE as usize].chunks_exact(4))
					.zip(c2.iter().copied()) {
					if let Ok(new) = Sample::try_from(new) {
						old.update(new, opt.paranoia(), sample_c2, sector_c2);
					}
				}

				progress.increment();
			}

			// Summarize the quality.
			progress.finish();
			let (q_good, q_maybe, q_bad) = self.offset_quality(opt.offset());
			let q_all = q_good + q_maybe + q_bad;
			let p1 = dactyl::int_div_float(q_good, q_all).unwrap_or(0.0);
			Msg::custom(
				"Ripped",
				10,
				&format!(
					"Track #{} is \x1b[2m(roughly)\x1b[0m {} complete.",
					self.idx,
					NicePercent::from(p1),
				)
			)
				.with_newline(true)
				.eprint();
			print_bar(q_good, q_maybe, q_bad);

			// Save the state file.
			if bincode::serialize(&self.state).ok()
				.and_then(|out| cache_write(&state_path, &out).ok())
				.is_none()
			{
				Msg::warning("The rip state couldn't be saved.").eprint();
			}

			// Should we stop or keep going?
			if pass == opt.passes() || killed.killed() || self.offset_good(opt.offset()) {
				break;
			}
		}

		// Don't forget to save the track.
		let dst = self.extract(opt.offset(), opt.raw())?;
		Ok(Some(dst))
	}

	#[allow(clippy::cast_possible_truncation)]
	/// # Extract the Track.
	fn extract(&self, offset: ReadOffset, raw: bool) -> Result<String, RipRipError> {
		let dst = rip_path(self.idx, raw);
		let rng = self.offset_range(offset);

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

			// Our samples are pairs of L/R, but hound considers L and R to be
			// separate, hence we're doubling the count.
			{
				let mut writer = wav.get_i16_writer((rng.end - rng.start) as u32 * 2);
				for sample in &self.state[rng] {
					let sample = sample.as_slice();
					debug_assert!(sample.len() == 4, "Sample is not 4-bytes!");
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
}

impl Rip {
	/// # Track Sample Length.
	fn track_sample_len(&self) -> usize {
		self.state.len() - SAMPLE_BUFFER as usize * 2
	}

	/// # Offset All Good?
	fn offset_good(&self, offset: ReadOffset) -> bool {
		let rng = self.offset_range(offset);
		self.state[rng].iter().all(RipSample::is_good)
	}

	/// # Count Up Good / Maybe / Bad Samples at offset.
	fn offset_quality(&self, offset: ReadOffset) -> (usize, usize, usize) {
		let mut good = 0;
		let mut maybe = 0;
		let mut bad = 0;
		let rng = self.offset_range(offset);
		for v in &self.state[rng] {
			match v {
				RipSample::Good(_) => { good += 1; },
				RipSample::Iffy(_) => { maybe += 1; },
				_ => { bad += 1; },
			}
		}

		(good, maybe, bad)
	}

	#[allow(clippy::cast_possible_truncation)]
	/// # Offset Range.
	///
	/// Return the (state) index range of the offset set.
	fn offset_range(&self, offset: ReadOffset) -> Range<usize> {
		let skip = usize::from(
			if offset.is_negative() { SAMPLE_BUFFER as u16 - offset.samples_abs() }
			else { SAMPLE_BUFFER as u16 + offset.samples_abs() }
		);
		let take = self.track_sample_len();
		skip..take+skip
	}
}



#[derive(Debug, Clone, Default, Deserialize, Serialize)]
/// # Rip Sample.
///
/// This is a combined sample/status structure, making it easy to know where
/// any given sample stands at a glance.
enum RipSample {
	#[default]
	/// # Not yet read.
	Tbd,

	/// # The drive gave us something but said it was bad.
	Bad(Sample),

	/// # Sample(s) awaiting paranoia confirmation.
	Iffy(Vec<(Sample, u8)>),

	/// # It should be good!
	Good(Sample),
}

impl RipSample {
	/// # As Slice.
	///
	/// Return the most appropriate single sample as a slice.
	fn as_slice(&self) -> &[u8] {
		match self {
			Self::Tbd => NULL_SAMPLE.as_slice(),
			Self::Bad(s) | Self::Good(s) => s.as_slice(),
			Self::Iffy(ref s) => s[0].0.as_slice(),
		}
	}

	/// # Is Good?
	const fn is_good(&self) -> bool { matches!(self, Self::Good(_)) }

	/// # Update.
	///
	/// Potentially update an entry.
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

				// Sort by popularity.
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
/// Note: the left padding is the equivalent of "Ripped: ".
fn print_bar(good: usize, maybe: usize, bad: usize) {
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
	if breakdown.is_empty() { eprintln!(); }
	else {
		eprintln!("        {} \x1b[2msamples\x1b[0m\n", breakdown.join(" \x1b[2m+\x1b[0m "));
	}
}

/// # Extraction Path.
///
/// Return the relative path for the ripped track.
fn rip_path(idx: u8, raw: bool) -> String {
	if raw { format!("{idx:02}.pcm") }
	else   { format!("{idx:02}.wav") }
}

/// # State Path.
///
/// Return the relative path for the track's state file.
fn state_path(ar: AccurateRip, idx: u8) -> String {
	format!("state/{ar}__{idx:02}.state")
}
