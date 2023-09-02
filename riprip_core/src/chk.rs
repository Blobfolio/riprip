/*!
# Rip Rip Hooray: Checksums
*/

use crate::{
	BYTES_PER_SAMPLE,
	BYTES_PER_SECTOR,
	cache_read,
	cache_write,
	RipSample,
	SAMPLES_PER_SECTOR,
};
use crc32fast::Hasher as Crc;
use cdtoc::{
	AccurateRip,
	Toc,
	Track,
};
use std::{
	sync::OnceLock,
	time::Duration,
};
use ureq::{
	Agent,
	AgentBuilder,
};



/// # Connection Agent.
static AGENT: OnceLock<Agent> = OnceLock::new();

/// # Maximum CTDB Offset Shift (in bytes).
const CTDB_WIGGLE: usize = BYTES_PER_SECTOR as usize * 10;

/// # Ten Sectors of Silence.
const SILENCE: &[u8] = &[0; CTDB_WIGGLE];



/// # Verify w/ AccurateRip.
///
/// This will download and cache the checksums from AccurateRip's servers, then
/// see if the track matches.
///
/// AccurateRip switched up checksum formats somewhere along the way, but both
/// provide statistical confidence, so this will check for and return each.
///
/// The computations are non-standard, but are more or less the sum of the
/// product of each sample pair (in byte form) and its relative index. All
/// data is factored, except the first `2939` samples of the first track, and
/// last `2941` samples of the last track.
///
/// AccurateRip only publishes checksums after they have been confirmed, so
/// even a value of one provides reasonable statistical certainty of
/// correctness.
///
/// AccurateRip is pressing-specific and their database only accepts
/// submissions from two Windows-only programs, so the match pool is limited
/// compared to CUETools.
pub(crate) fn chk_accuraterip(ar: AccurateRip, track: Track, data: &[RipSample])
-> Option<(u8, u8)> {
	// Fetch/cache the checksums.
	let dst = format!("state/{ar}__chk-ar.bin");
	let chk = cache_read(&dst).ok()
		.flatten()
		.or_else(|| {
			let url = ar.checksum_url();
			let chk = download(&url)?;
			let _res = cache_write(&dst, &chk);
			Some(chk)
		})
		.and_then(|chk| ar.parse_checksums(&chk).ok())
		.and_then(|mut chk| {
			let idx = usize::from(track.number() - 1);
			if idx < chk.len() { Some(chk.remove(idx)) }
			else { None }
		})?;

	// Figure out which samples we need to crunch.
	let pos = track.position();
	let start =
		if pos.is_first() { SAMPLES_PER_SECTOR as usize * 5 - 1 }
		else { 0 };
	let end =
		if pos.is_last() { data.len().saturating_sub(SAMPLES_PER_SECTOR as usize * 5 + 1) }
		else { data.len() };
	if end <= start { return None; }

	// Crunch!
	let mut crc1 = 0_u64; // Version #1.
	let mut crc2 = 0_u64; // Version #2.
	let mut idx = 0;

	for sample in data {
		if start <= idx && idx <= end {
			let slice = sample.as_array();
			let v = u64::from_le_bytes([
				slice[0], slice[1], slice[2], slice[3], 0, 0, 0, 0,
			]);

			let k = idx as u64 + 1;
			let kv = k * v;

			crc1 += kv;
			crc2 += (kv >> 32) + (kv & 0xFFFF_FFFF);
		}

		idx += 1;
		if idx > end { break; }
	}

	// Drop them to 32 bits.
	let crc1 = (crc1 & 0xFFFF_FFFF) as u32;
	let crc2 = (crc2 & 0xFFFF_FFFF) as u32;

	// Return the matches, if any.
	Some((
		chk.get(&crc1).copied().unwrap_or(0),
		chk.get(&crc2).copied().unwrap_or(0),
	))
}



/// # Verify w/ CUETools.
///
/// This will download and cache the checksums from CUETools's servers, then
/// see if the track matches.
///
/// Unlike AccurateRip, CUETools checksums are standard CRC32 hashes of the
/// full track byte stream, except for the first and last track, which ignore
/// the leading `5880` and trailing `5880..=11172` samples respectively.
///
/// The end-trim's variability comes from a quirk of the parity data CUETools
/// also tracks, which requires the disc-side data be evenly divisible by ten
/// sectors. (That parity data isn't relevant here, but is what makes CUETools
/// repair work, so is worth a little extra weirdness!)
///
/// Because CRC32 is used, it is computationally feasible to check for matches
/// across different album pressings. (Pressings from the same master usually
/// contain identical data that is merely shifted up or down some arbitrary
/// number of samples.)
///
/// Offset matching is most effective when the adjacent track data is
/// available, but since there's usually a good amount of null-padding around
/// tracks, null samples can be used as the next best thing.
///
/// Since Rip Rip Hooray only looks at one track at a time, it may undercount
/// matches, but will usually find enough for yes/no verification.
///
/// It is worth noting that CUETools submissions are published more or less
/// immediately and require no second opinion, so this method will return `0`
/// for any value less than `2`.
pub(crate) fn chk_ctdb(toc: &Toc, ar: AccurateRip, track: Track, data: &[RipSample]) -> Option<u16> {
	// Fetch/cache the checksums.
	let dst = format!("state/{ar}__chk-ctdb.bin");
	let mut chk = cache_read(&dst).ok()
		.flatten()
		.or_else(|| {
			let url = toc.ctdb_checksum_url();
			let chk = download(&url)?;
			let _res = cache_write(&dst, &chk);
			Some(chk)
		})
		.and_then(|chk| {
			let chk = String::from_utf8(chk).ok()?;
			toc.ctdb_parse_checksums(&chk).ok()
		})
		.and_then(|mut chk| {
			let idx = usize::from(track.number() - 1);
			if idx < chk.len() { Some(chk.remove(idx)) }
			else { None }
		})?;

	// Reserve space at the beginning and end for offset-wiggling. We'll shift
	// 10 sectors in either direction, but also need to account for the
	// "ignored" regions at the start and end of the disc.
	let pos = track.position();
	let prefix =
		// The first 10 sectors are ignored for the first track.
		if pos.is_first() { SAMPLES_PER_SECTOR as usize * 20 }
		else { SAMPLES_PER_SECTOR as usize * 10 };
	let suffix =
		// The last 10 + (album % 10) sectors are ignored for the last track.
		if pos.is_last() {
			SAMPLES_PER_SECTOR as usize * 20 +
			usize::try_from(toc.duration().samples()).ok()? % (SAMPLES_PER_SECTOR as usize * 10)
		}
		else { SAMPLES_PER_SECTOR as usize * 10 };

	// We need at least one sector leftover.
	if data.len() < prefix + suffix + SAMPLES_PER_SECTOR as usize { return None; }

	// Carve up the data. We need to keep the ends in byte form, but can
	// precalculate the CRC for the entire middle chunk.
	let mut start = Vec::with_capacity(prefix * BYTES_PER_SAMPLE as usize);
	let mut middle = Crc::new();
	let mut end = Vec::with_capacity(suffix * BYTES_PER_SAMPLE as usize);
	let end_starts = data.len() - suffix;
	for (k, sample) in data.iter().enumerate() {
		if k < prefix { start.extend_from_slice(sample.as_slice()); }
		else if k < end_starts { middle.update(sample.as_slice()); }
		else { end.extend_from_slice(sample.as_slice()); }
	}

	// Check for a straight match before wiggling.
	let mut confidence = 0;
	let mut crc = Crc::new();
	if pos.is_first() { crc.update(&start[CTDB_WIGGLE..]); }
	else { crc.update(&start); }
	crc.combine(&middle);
	crc.update(&end[..CTDB_WIGGLE]);
	if let Some(v) = chk.remove(&crc.finalize()) {
		confidence += v;
		if chk.is_empty() {
			return Some(if confidence < 2 { 0 } else { confidence });
		}
	}

	// Shift and crunch and shift and crunch!
	for shift in 1..=SAMPLES_PER_SECTOR as usize * 10 {
		let shift = shift * BYTES_PER_SAMPLE as usize;

		// Negative offset.
		let mut crc = Crc::new();

		// Because the ignored region of the first track is the same as our
		// wiggle, it always has enough data for negative shifting.
		if pos.is_first() { crc.update(&start[CTDB_WIGGLE - shift..]); }
		// For other tracks, we need to supplement with silence.
		else {
			crc.update(&SILENCE[..shift]);
			crc.update(&start);
		}

		// Add the middle.
		crc.combine(&middle);

		// Maybe add the end.
		if shift != CTDB_WIGGLE { crc.update(&end[..CTDB_WIGGLE - shift]); }

		// Check it!
		if let Some(v) = chk.remove(&crc.finalize()) {
			confidence += v;
			if chk.is_empty() {
				return Some(if confidence < 2 { 0 } else { confidence });
			}
		}

		// Now positive!
		let mut crc = Crc::new();

		// Maybe add the start.
		if shift != CTDB_WIGGLE {
			// Ignored regions still apply.
			if pos.is_first() { crc.update(&start[CTDB_WIGGLE + shift..]); }
			else { crc.update(&start[shift..]); }
		}

		// Add the middle.
		crc.combine(&middle);

		// Because the ignored region of the last track is >= our wiggle, it
		// always has enough for positive shifting.
		if pos.is_last() { crc.update(&end[..CTDB_WIGGLE + shift]); }
		// For other tracks, we need to supplement with silence.
		else {
			crc.update(&end);
			crc.update(&SILENCE[..shift]);
		}

		// Check it!
		if let Some(v) = chk.remove(&crc.finalize()) {
			confidence += v;
			if chk.is_empty() {
				return Some(if confidence < 2 { 0 } else { confidence });
			}
		}
	}

	Some(if confidence < 2 { 0 } else { confidence })
}



/// # Connection Agent.
fn agent() -> &'static Agent {
	AGENT.get_or_init(||
		AgentBuilder::new()
			.timeout(Duration::from_secs(15))
			.user_agent(concat!(
				"Mozilla/5.0 (X11; Linux x86_64; rv:",
				env!("CARGO_PKG_VERSION"),
				") RipRip/",
				env!("CARGO_PKG_VERSION"),
			))
			.max_idle_connections(0)
			.build()
	)
}

/// # Download.
fn download(url: &str) -> Option<Vec<u8>> {
	let res = agent().get(url).call().ok()?;

	let mut out = Vec::new();
	res.into_reader().read_to_end(&mut out).ok()?;

	if out.is_empty() { None }
	else { Some(out) }
}
