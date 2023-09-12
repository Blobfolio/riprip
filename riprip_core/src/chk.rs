/*!
# Rip Rip Hooray: Checksums
*/

use crate::{
	BYTES_PER_SAMPLE,
	BYTES_PER_SECTOR,
	cache_path,
	CACHE_SCRATCH,
	CacheWriter,
	RipSample,
	SAMPLES_PER_SECTOR,
};
use crc32fast::Hasher as Crc;
use cdtoc::{
	Toc,
	Track,
};
use std::{
	path::Path,
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
pub(crate) fn chk_accuraterip(toc: &Toc, track: Track, data: &[RipSample])
-> Option<(u8, u8)> {
	// Fetch/cache the checksums.
	let ar = toc.accuraterip_id();
	let dst = cache_path(format!("{CACHE_SCRATCH}/{}__chk-ar.bin", toc.cddb_id())).ok()?;
	let chk = std::fs::read(&dst).ok()
		.or_else(|| {
			let url = ar.checksum_url();
			let chk = download(&url, &dst)?;
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
			let sample = sample.as_array();
			let v = u64::from_le_bytes([
				sample[0], sample[1], sample[2], sample[3], 0, 0, 0, 0,
			]);

			let k = idx as u64 + 1;
			let kv = k * v;

			crc1 += kv;
			crc2 += (kv >> 32) + (kv & 0xFFFF_FFFF);
		}

		idx += 1;
		if idx > end { break; }
	}

	// Sixty-four bits were only used to help with overflow; the final checksum
	// only uses half that much.
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
/// collects, which requires the disc-wide data be evenly divisible by ten
/// sectors. (That parity data isn't relevant here, but is pretty cool!)
///
/// Because CRC32sums can be _combined_, it is computationally feasible to
/// search for matches at _different_ offsets (e.g. across pressings), which
/// greatly increases the size of the potential match pool.
///
/// Most pressings will be within a thousand or so samples of one another, so
/// there isn't much point shifting data too much. Rip Rip checks `±5880`
/// to ensure the full ignored region at the start is testable.
///
/// Ideally shifts would incorporate the beginning/end of the adjacent track
/// data, but since Rip Rip doesn't have that, it null-pads instead. Most
/// track boundaries _do_ contain null samples so that works well, but may
/// undercount the matches as a result.
///
/// It is worth noting that CUETools submissions are published more or less
/// immediately and require no second opinion, so this method will return `0`
/// for any value less than `2` to avoid confusion.
pub(crate) fn chk_ctdb(toc: &Toc, track: Track, data: &[RipSample]) -> Option<u16> {
	// Fetch/cache the checksums.
	let dst = cache_path(format!("{CACHE_SCRATCH}/{}__chk-ctdb.xml", toc.cddb_id())).ok()?;
	let mut chk = std::fs::read(&dst).ok()
		.or_else(|| {
			let url = toc.ctdb_checksum_url();
			let chk = download(&url, &dst)?;
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

	// Shifts will alter the beginning and end of the track, but not the
	// middle. As such we need to hold the edges as raw bytes so their
	// checksums can be computed dynamically. Space-wise, we need enough to
	// match the maximum shift, plus the "ignored" regions if this track is the
	// first and/or last on the disc.
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

	// Make sure we have at least one sector's worth of data left over!
	if data.len() < prefix + suffix + SAMPLES_PER_SECTOR as usize { return None; }

	// Carve up the data! The start and end bytes will be stored in vectors to
	// keep them arbitrarily sliceable, but since everything else will remain
	// constant at any offset, we can precompute its checksum.
	let mut start = Vec::with_capacity(prefix * BYTES_PER_SAMPLE as usize);
	let mut middle = Crc::new();
	let mut end = Vec::with_capacity(suffix * BYTES_PER_SAMPLE as usize);
	let end_starts = data.len() - suffix;
	for (k, sample) in data.iter().enumerate() {
		if k < prefix { start.extend_from_slice(sample.as_slice()); }
		else if k < end_starts { middle.update(sample.as_slice()); }
		else { end.extend_from_slice(sample.as_slice()); }
	}

	// Before we start shifting shit around, let's see if we match at zero,
	// i.e. start + middle + end (minus ignorable regions, if any).
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

	// Shift and crunch and shift and crunch and shift and crunch…
	for shift in 1..=SAMPLES_PER_SECTOR as usize * 10 {
		// We're stepping in samples, but working in bytes.
		let shift = shift * BYTES_PER_SAMPLE as usize;

		// Let's try for a negative match first, shifting the data into the
		// end of the theoretical previous track. (We'll assume that track's
		// data is silence.)
		let mut crc = Crc::new();

		// Because the ignored region of the first track is the same as our
		// wiggle, it always has enough data reserved for negative shifting.
		// (Data is still being ignored, but now it's data we don't have, which
		// works out great!)
		if pos.is_first() { crc.update(&start[CTDB_WIGGLE - shift..]); }
		// For other tracks, we need to supplement with silence.
		else {
			crc.update(&SILENCE[..shift]);
			crc.update(&start);
		}

		// Add the middle.
		crc.combine(&middle);

		// Maybe add the end. The ignored regions of the last track don't
		// require special handling in this case.
		if shift != CTDB_WIGGLE { crc.update(&end[..CTDB_WIGGLE - shift]); }

		// Check it!
		if let Some(v) = chk.remove(&crc.finalize()) {
			confidence += v;
			if chk.is_empty() {
				return Some(if confidence < 2 { 0 } else { confidence });
			}
		}

		// Now let's check for a positive offset match, bleeding into the
		// theoretical next track. The ideas are the same, except now any
		// assumed silence will be at the end.
		let mut crc = Crc::new();

		// Maybe add the start.
		if shift != CTDB_WIGGLE {
			// The first track still requires special handling to keep the
			// ignorable bits ignored.
			if pos.is_first() { crc.update(&start[CTDB_WIGGLE + shift..]); }
			// Everything else is what it is.
			else { crc.update(&start[shift..]); }
		}

		// Add the middle.
		crc.combine(&middle);

		// Because the ignored region of the last track is >= our wiggle, it
		// always has enough in reserve for positive shifting.
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

	// As mentioned at the start, we shouldn't be confident in confidences less
	// than two, so to avoid confusion, we'll treat them as equivalent to no
	// matches at all.
	Some(if confidence < 2 { 0 } else { confidence })
}



/// # Connection Agent.
///
/// Storing the agent statically saves a little bit of overhead on reuse. Since
/// the checksums are cached locally, this may not get called at all.
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
///
/// Download and return the data!
fn download(url: &str, dst: &Path) -> Option<Vec<u8>> {
	use std::io::Write;

	// Download the data into a vector.
	let res = agent().get(url).call().ok()?;
	let mut out = Vec::new();
	res.into_reader().read_to_end(&mut out).ok()?;

	if out.is_empty() { None }
	else {
		// Cache the contents for next time.
		let _res = CacheWriter::new(dst).ok()
			.and_then(|mut writer| {
				writer.writer().write_all(&out).ok()?;
				writer.finish().ok()
			});

		Some(out)
	}
}
