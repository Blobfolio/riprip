/*!
# Rip Rip Hooray: Checksums
*/

use crate::{
	BYTES_PER_SAMPLE,
	cache_path,
	CACHE_SCRATCH,
	CacheWriter,
	RipSample,
	SAMPLE_OVERREAD,
	SAMPLES_PER_SECTOR,
};
use crc32fast::Hasher as Crc;
use cdtoc::{
	Toc,
	Track,
};
use std::{
	path::Path,
	sync::{
		Arc,
		atomic::{
			AtomicU16,
			Ordering::Relaxed,
		},
		Mutex,
		OnceLock,
	},
	time::Duration,
};
use ureq::{
	Agent,
	AgentBuilder,
};



/// # Connection Agent.
static AGENT: OnceLock<Agent> = OnceLock::new();

/// # Maximum CTDB Offset Shift (in bytes).
const CTDB_WIGGLE: usize = CTDB_WIGGLE_SAMPLES * BYTES_PER_SAMPLE as usize;

/// # Maximum CTDB Offset Shift (in samples).
const CTDB_WIGGLE_SAMPLES: usize = SAMPLE_OVERREAD as usize;



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
	let dst = cache_path(format!("{CACHE_SCRATCH}/{}__chk-ar.bin", ar.cddb_id())).ok()?;
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
		if pos.is_first() { usize::from(SAMPLES_PER_SECTOR) * 5 - 1 }
		else { 0 };
	let end =
		if pos.is_last() { data.len().saturating_sub(usize::from(SAMPLES_PER_SECTOR) * 5 + 1) }
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
/// Shifting works best when the adjacent track data is known, which we can
/// accommodate since we overrip tracks by 10 sectors on either side anyway.
/// (Depending on the overlap some samples from those 20 sectors won't have
/// been read, but most should be, and null samples should be good enough for
/// the rest.)
///
/// Speaking of, the `data` passed to this method is the _full_ rip range, not
/// just the track portion.
///
/// Also of note: CUETools submissions are published more or less immediately
/// and require no second opinion, so this method will return `0` for any value
/// less than `2` to avoid confusion.
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

	// Our data range is the track with ten extra sectors on either end. We
	// need to keep that padding in byte form, as well as the portions of the
	// track that might get shifted off or are ignored. That works out to
	// `max-shift * 2`, with a bit extra for the first and last track to
	// account for their ignored regions.
	let pos = track.position();
	let prefix =
		// The first 10 sectors are ignored for the first track.
		if pos.is_first() { CTDB_WIGGLE_SAMPLES * 3 }
		else { CTDB_WIGGLE_SAMPLES * 2 };
	let suffix =
		// The last 10 + (album % 10) sectors are ignored for the last track.
		if pos.is_last() {
			CTDB_WIGGLE_SAMPLES * 3 +
			usize::try_from(toc.duration().samples()).ok()? % CTDB_WIGGLE_SAMPLES
		}
		else { CTDB_WIGGLE_SAMPLES * 2 };

	// Prefix and suffix are in samples, but it will also be handy to know how
	// many bytes are being ignored for the start and end, so let's calculate
	// that now.
	let ignore_first = (prefix - CTDB_WIGGLE_SAMPLES * 2) * usize::from(BYTES_PER_SAMPLE);
	let ignore_last = (suffix - CTDB_WIGGLE_SAMPLES * 2) * usize::from(BYTES_PER_SAMPLE);

	// Before we start slicing, make sure there is at least one sector's worth
	// of data to shove in the middle, or it's too short to bother with.
	if data.len() < prefix + suffix + usize::from(SAMPLES_PER_SECTOR) { return None; }

	// Carve it up! We need to keep the start and end in byte form so they can
	// be dynamically resliced, but everything else (the middle) can be
	// immediately crunched into a CRC32 since it will always be present at any
	// offset.
	let mut start = Vec::with_capacity(prefix * usize::from(BYTES_PER_SAMPLE));
	let mut middle = Crc::new();
	let mut end = Vec::with_capacity(suffix * usize::from(BYTES_PER_SAMPLE));
	let end_starts = data.len() - suffix;
	for (k, sample) in data.iter().enumerate() {
		if k < prefix { start.extend_from_slice(sample.as_slice()); }
		else if k < end_starts { middle.update(sample.as_slice()); }
		else { end.extend_from_slice(sample.as_slice()); }
	}

	// Check the zero shift first.
	let mut confidence = 0;
	let mut crc = Crc::new();
	crc.update(&start[CTDB_WIGGLE + ignore_first..]);
	crc.combine(&middle);
	crc.update(&end[..end.len() - CTDB_WIGGLE - ignore_last]);

	// Check it!
	if let Some(v) = chk.remove(&crc.finalize()) {
		confidence += v;
		if chk.is_empty() {
			return Some(if confidence < 2 { 0 } else { confidence });
		}
	}

	// Using two threads — one for each direction — strikes a good balance
	// between performance and complexity. We do have to rewrap our variables,
	// though, to maintain mutability across threads.
	let chk = Arc::new(Mutex::new(chk));
	let confidence = AtomicU16::new(confidence);
	std::thread::scope(|s| {
		// Negative offsets shift into the previous track.
		s.spawn(|| {
			for shift in 1..=CTDB_WIGGLE_SAMPLES {
				// We're stepping in samples, but working in bytes.
				let shift = shift * usize::from(BYTES_PER_SAMPLE);
				let mut crc = Crc::new();
				crc.update(&start[CTDB_WIGGLE + ignore_first - shift..]);
				crc.combine(&middle);
				// The max shift won't include any end.
				if shift < CTDB_WIGGLE {
					crc.update(&end[..end.len() - CTDB_WIGGLE - ignore_last - shift]);
				}

				// Check it!
				if let Ok(mut tmp) = chk.lock() {
					if tmp.is_empty() { break; }
					else if let Some(v) = tmp.remove(&crc.finalize()) {
						drop(tmp); // Be a good neighbor and drop the borrow ASAP.
						confidence.fetch_add(v, Relaxed);
					}
				}
			}
		});

		// Positive offsets shift into the next track.
		s.spawn(|| {
			for shift in 1..=CTDB_WIGGLE_SAMPLES {
				// We're stepping in samples, but working in bytes.
				let shift = shift * usize::from(BYTES_PER_SAMPLE);

				let mut crc = Crc::new();
				// The max shift won't include any start.
				if shift < CTDB_WIGGLE {
					crc.update(&start[CTDB_WIGGLE + ignore_first + shift..]);
				}
				crc.combine(&middle);
				crc.update(&end[..end.len() - CTDB_WIGGLE - ignore_last + shift]);

				// Check it!
				if let Ok(mut tmp) = chk.lock() {
					if tmp.is_empty() { break; }
					else if let Some(v) = tmp.remove(&crc.finalize()) {
						drop(tmp); // Be a good neighbor and drop the borrow ASAP.
						confidence.fetch_add(v, Relaxed);
					}
				}
			}
		});
	});

	// As mentioned at the start, we shouldn't be confident in confidences less
	// than two, so to avoid confusion, we'll treat them as equivalent to no
	// matches at all.
	let confidence = confidence.into_inner();
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
