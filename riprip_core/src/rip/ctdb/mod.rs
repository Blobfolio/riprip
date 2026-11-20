//! Opt-in CTDB assessment and recovery of saved, drive-corrected PCM.

mod lookup;
pub(super) mod output;
mod repair;
mod rs;
#[cfg(test)]
mod tests;

use super::verify_track;
use crate::{KillSwitch, RipOptions, RipSample, RipState, SAMPLES_PER_SECTOR, SavedRips};
use cdtoc::{Toc, Track};
use repair::{Geometry, Quality, Rejection, Repair, Snapshot, checksum, matches_tracks, unchanged_crcs};
use rs::Field;
use std::{collections::BTreeMap, ops::Range};

/// The standard CTDB stride in stereo frames.
const STRIDE: usize = 10 * SAMPLES_PER_SECTOR as usize;

/// The caller's authorization to install verified repairs.
#[derive(Clone, Copy)]
pub(super) enum Mode<'a> {
	/// Calculate and report results without writing any files.
	Assess,
	/// Publish verified results, excluding stale states from this rip.
	Repair {
		/// Selected tracks with unavailable or stale state files.
		unavailable: &'a [u8],
		/// Tracks whose outside-window drive observations changed.
		refresh: &'a [u8],
	},
}

impl Mode<'_> {
	/// Choose sources consistent with the normal rip's resume setting.
	fn allows(self, track: Track, opts: &RipOptions) -> bool {
		match self {
			Self::Assess => true,
			Self::Repair { unavailable, .. } => !unavailable.contains(&track.number()) &&
				(opts.resume() || opts.tracks().any(|n| n == track.number())),
		}
	}
}

/// Confirmed coverage for one track, in drive-corrected disc coordinates.
#[derive(Clone)]
pub(super) struct Coverage {
	/// Track samples reachable by the current drive.
	track: Range<i32>,
	/// The interval confirmed by CTDB.
	verified: Range<i32>,
	/// Reachable guard samples still needing physical evidence.
	pub(super) needs_reads: bool,
	/// Verified PCM and the remaining drive evidence.
	pub(super) quality: super::TrackQuality,
}

impl Coverage {
	/// Whether a sample belongs to this track and lies in a guard window.
	pub(super) fn needs_sample(&self, position: i32) -> bool {
		self.track.contains(&position) && !self.verified.contains(&position)
	}
}

/// Publication and the work that can be skipped after the verification.
#[derive(Default)]
pub(super) struct Outcome {
	/// WAVs installed from the verified snapshot.
	pub(super) tracks: SavedRips,
	/// Sample coverage of the accepted snapshot.
	pub(super) coverage: BTreeMap<u8, Coverage>,
	/// Confidence of the accepted reference.
	pub(super) confidence: u16,
}

/// Keep lookup results and downloads for one rip so successive passes reuse them.
pub(super) struct Session {
	/// Arithmetic reused across checkpoints.
	field: Field,
	/// Compatible records from the first lookup.
	entries: Option<Vec<lookup::Entry>>,
	/// Download results retained for this rip.
	parity: BTreeMap<u64, Result<Vec<u16>, String>>,
	/// Recheck the previous correction if only the guard windows were read.
	last_repair: Option<(u64, Repair)>,
}

impl Session {
	/// Start an independent CTDB session for the current disc.
	pub(super) fn new() -> Self {
		Self { field: Field::new(), entries: None, parity: BTreeMap::new(), last_repair: None }
	}

	/// Validate and publish the currently saved evidence, if possible.
	pub(super) fn check(&mut self, toc: &Toc, opts: &RipOptions, killed: KillSwitch, mode: Mode<'_>)
	-> Outcome {
		if killed.killed() { return Outcome::default(); }
		match run_inner(self, toc, opts, killed, mode) {
			Ok(outcome) => outcome,
			Err(message) => {
				eprintln!("CTDB repair — {message}");
				Outcome::default()
			},
		}
	}
}

/// Run a standalone, read-only assessment.
pub(crate) fn assess(toc: &Toc, opts: &RipOptions, killed: KillSwitch) {
	let _ = Session::new().check(toc, opts, killed, Mode::Assess);
}

/// One consistent, contiguous audio image from the saved track states.
struct Audio {
	/// Interleaved 16-bit left and right audio words.
	pcm: Vec<u16>,
	/// Recovery evidence for each stereo frame.
	quality: Vec<Quality>,
	/// Starts of the local tracks, followed by audio lead-out.
	boundaries: Vec<usize>,
	/// Audio tracks with no saved evidence.
	missing: usize,
	/// Positions restored using agreeing overreads from adjacent tracks.
	overlap_changes: Vec<usize>,
}

impl Audio {
	/// Assemble INDEX 01 through audio lead-out. RipState has already applied
	/// the drive offset. For unread boundaries, reuse strong adjacent overreads.
	fn load(toc: &Toc, opts: &RipOptions, lengths: &[usize], mode: Mode<'_>, killed: KillSwitch)
	-> Result<Self, String> {
		let frames = lengths.iter().sum();
		let mut pcm = Vec::new();
		let mut quality = Vec::new();
		pcm.try_reserve_exact(2 * frames).map_err(|_| "not enough memory for disc PCM")?;
		quality.try_reserve_exact(frames).map_err(|_| "not enough memory for read quality")?;
		let mut boundaries = Vec::with_capacity(lengths.len() + 1);
		let mut missing = 0;
		let mut overlaps = Vec::new();
		let rereads = opts.rereads();
		let trim_end = STRIDE + frames % STRIDE;
		let min_conf = opts.confidence();

		for (idx, (track, &len)) in toc.audio_tracks().zip(lengths).enumerate() {
			if killed.killed() { return Err("calculation cancelled".into()); }
			let start = quality.len();
			boundaries.push(start);
			let state = if mode.allows(track, opts) {
				RipState::read_existing(toc, track, opts).map_err(|err| err.to_string())?
			}
			else { None };
			let Some(state) = state else {
				missing += 1;
				pcm.resize(pcm.len() + 2 * len, 0);
				quality.resize(start + len, Quality::Erasure);
				continue;
			};
			let samples = state.track_slice();
			if samples.len() != len { return Err("saved track length differs from the disc".into()); }
			let (ar, ctdb) = verify_track(track, &state, true);
			let trusted_ar = ar.is_some_and(|(a, b)| a.max(b) >= min_conf);
			let trusted_ctdb = ctdb.is_some_and(|confidence| confidence >= u16::from(min_conf));
			for (at, sample) in samples.iter().enumerate() {
				pcm.extend(sample.as_array().as_chunks::<2>().0.iter().copied().map(u16::from_le_bytes));
				let ar_margin = 5 * usize::from(SAMPLES_PER_SECTOR) + 1;
				let in_ar = (idx != 0 || at >= ar_margin) &&
					(idx + 1 != lengths.len() || at + ar_margin < len);
				let ctdb_left = if idx == 0 { STRIDE * 2 } else { STRIDE };
				let ctdb_right = if idx + 1 == lengths.len() { trim_end + STRIDE } else { STRIDE };
				let in_ctdb = at >= ctdb_left && at + ctdb_right < len;
				quality.push(if (trusted_ar && in_ar) || (trusted_ctdb && in_ctdb) {
					Quality::Trusted
				}
				else if sample.is_bad() || matches!(sample, RipSample::Lead) { Quality::Erasure }
				else if sample.is_likely(rereads) { Quality::Likely }
				else { Quality::Weak });
			}
			let padded = state.rip_slice();
			let padding = (padded.len() - len) / 2;
			for (at, sample) in padded[..padding].iter().enumerate()
				.chain(padded[padding + len..].iter().enumerate().map(|(at, s)| (at + padding + len, s)))
			{
				if let Some(frame) = (start + at).checked_sub(padding) &&
					frame < frames &&
					matches!(sample, RipSample::Maybe(_)) && sample.is_likely(rereads)
				{
					overlaps.push((frame, sample.as_array()));
				}
			}
		}
		boundaries.push(quality.len());

		let mut overlap_changes = Vec::new();
		overlaps.sort_unstable_by_key(|&(frame, _)| frame);
		for candidates in overlaps.chunk_by(|a, b| a.0 == b.0) {
			let (frame, bytes) = candidates[0];
			// Keep alternate reads inside the region every supported
			// alignment can verify before exporting the reconstructed copy.
			if frame >= 2 * STRIDE && frame + trim_end + STRIDE < frames &&
				quality[frame] == Quality::Erasure && candidates.iter().all(|&(_, x)| x == bytes)
			{
				if pcm_frame([pcm[2 * frame], pcm[2 * frame + 1]]) != bytes {
					overlap_changes.push(frame);
				}
				pcm[2 * frame] = u16::from_le_bytes([bytes[0], bytes[1]]);
				pcm[2 * frame + 1] = u16::from_le_bytes([bytes[2], bytes[3]]);
				quality[frame] = Quality::Likely;
			}
		}
		Ok(Self { pcm, quality, boundaries, missing, overlap_changes })
	}
}

/// Install a unique verified repair at the normal rip paths.
fn run_inner(session: &mut Session, toc: &Toc, opts: &RipOptions, killed: KillSwitch, mode: Mode<'_>)
-> Result<Outcome, String> {
	let assess = matches!(mode, Mode::Assess);
	let lengths: Vec<_> = toc.audio_tracks().map(|track| {
		usize::try_from(track.duration().samples()).map_err(|_| "unsupported track length")
	}).collect::<Result<_, _>>()?;
	let frames = lengths.iter().try_fold(0_usize, |sum, &len| sum.checked_add(len))
		.ok_or("unsupported audio length")?;
	let geometry = Geometry::new(frames, STRIDE).ok_or("unsupported audio length")?;
	let minimum = u16::from(opts.confidence());
	if session.entries.is_none() {
		session.entries = Some(Vec::new());
		let mut entries = lookup::lookup(toc, &lengths, assess)?;
		let available = entries.iter().filter(|entry| entry.parity_url.is_some()).count();
		let low = entries.iter().filter(|e| e.confidence < minimum && e.parity_url.is_some()).count();
		eprintln!("CTDB repair — {available} compatible records advertise parity, {low} below confidence {minimum}");
		entries.retain(|entry| entry.confidence >= minimum);
		if entries.is_empty() { return Err(format!("no CTDB records at confidence {minimum} or higher")); }
		session.entries = Some(entries);
	}
	let Some(entries) = session.entries.as_deref().filter(|entries| !entries.is_empty())
	else { return Ok(Outcome::default()); };
	if assess && toc.htoa().is_some() {
		eprintln!("CTDB repair — the first-track pregap is outside CTDB coverage");
	}
	if !toc.audio_tracks().any(|track| {
		mode.allows(track, opts) &&
			crate::cache::state_read_path(toc, track).is_ok_and(|path| path.is_file())
	}) {
		let max = entries.iter().filter(|entry| entry.parity_url.is_some())
			.map(|entry| entry.reference.len()).max().unwrap_or(0);
		return Err(format!("no saved audio to assess. At most {max} parity rows are advertised, with {} unread frames per lane",
			geometry.rows));
	}

	let audio = Audio::load(toc, opts, &lengths, mode, killed)?;
	if audio.missing != 0 {
		eprintln!("CTDB repair — {} audio tracks have no saved state", audio.missing);
	}
	let max_roots = entries.iter().filter(|e| e.parity_url.is_some() &&
		!session.parity.get(&e.id).is_some_and(Result::is_err))
		.map(|e| e.reference.len()).max().unwrap_or(0);
	let previous = session.last_repair.as_ref().and_then(|(id, repair)| {
		let entry = entries.iter().find(|entry| entry.id == *id)?;
		let window = geometry.window(repair.offset);
		(checksum(&audio.pcm, window.clone(), &repair.corrections) == entry.disc_crc &&
			matches_tracks(&audio.pcm, entry, window, repair.offset, &repair.corrections))
			.then(|| (entry, repair.clone()))
	});
	let (entry, repair) = if let Some(candidate) = previous {
		candidate
	}
	else {
		let crcs = unchanged_crcs(&audio.pcm, geometry, killed);
		let already_verified = crcs.iter().find_map(|&(offset, crc)| {
			entries.iter().find(|e| e.disc_crc == crc &&
				matches_tracks(&audio.pcm, e, geometry.window(offset), offset, &[]))
				.map(|entry| (offset, entry))
		});
		if !assess && let Some((offset, entry)) = already_verified {
			(entry, Repair { offset, corrections: Vec::new() })
		}
		else {
			if !assess && max_roots == 0 { return Err("no usable CTDB parity".into()); }
			let snapshot = Snapshot::new(&session.field, geometry, &audio.pcm, &audio.quality, max_roots, killed)
				.ok_or("calculation cancelled")?;
			let Some(candidate) = find_repair(&snapshot, entries, &mut session.parity, assess,
				already_verified.is_some(), killed)?
			else { return Ok(Outcome::default()); };
			candidate
		}
	};
	let window = geometry.window(repair.offset);
	let outside = audio.quality[..window.start].iter().chain(&audio.quality[window.end..])
		.filter(|&&q| matches!(q, Quality::Weak | Quality::Erasure)).count();
	if outside != 0 {
		eprintln!("CTDB repair — {outside} uncertain stereo frames lie outside verified coverage");
	}
	let Mode::Repair { refresh, .. } = mode else { return Ok(Outcome::default()); };
	let tracks = output::publish(toc, killed, &audio, &repair, window.clone(), entry.confidence, refresh)?;
	eprintln!("CTDB repair — verified {} corrected channel words at offset {:+} frames",
		repair.corrections.len(), repair.offset);
	let coverage = track_coverage(toc, opts, &audio, window);
	session.last_repair = Some((entry.id, repair));
	Ok(Outcome { tracks, coverage, confidence: entry.confidence })
}

/// Exclude reachable, uncertain guard samples from the early-stop decision.
fn track_coverage(toc: &Toc, opts: &RipOptions, audio: &Audio, window: Range<usize>)
-> BTreeMap<u8, Coverage> {
	let Some(accessible) = super::data::accessible_range(toc, opts.offset()) else { return BTreeMap::new(); };
	let origin = i32::try_from(toc.audio_leadin_normalized()).expect("disc geometry")
		* i32::from(SAMPLES_PER_SECTOR);
	let to_disc = |at| origin + i32::try_from(at).expect("disc geometry");
	toc.audio_tracks().zip(audio.boundaries.windows(2)).filter_map(|(track, bounds)| {
		let [start, end] = [bounds[0], bounds[1]];
		let verified = start.max(window.start)..end.min(window.end);
		if verified.is_empty() { return None; }
		let mut needs_reads = false;
		let mut confirmed = u32::try_from(verified.len()).expect("disc geometry");
		let (mut likely, mut maybe, mut bad) = (0, 0, 0);
		for at in (start..verified.start).chain(verified.end..end) {
			let quality = audio.quality[at];
			needs_reads |= accessible.contains(&to_disc(at)) && matches!(quality, Quality::Weak | Quality::Erasure);
			match quality {
				Quality::Trusted => confirmed += 1,
				Quality::Likely => likely += 1,
				Quality::Weak => maybe += 1,
				Quality::Erasure => bad += 1,
			}
		}
		Some((track.number(), Coverage {
			track: to_disc(start).max(accessible.start)..to_disc(end).min(accessible.end),
			verified: to_disc(verified.start)..to_disc(verified.end),
			needs_reads,
			quality: super::TrackQuality::from_ctdb(confirmed, likely, maybe, bad),
		}))
	}).collect()
}

/// Evaluate eligible parity records and require agreement on the corrected PCM.
fn find_repair<'a>(snapshot: &Snapshot<'_>, entries: &'a [lookup::Entry],
	parity_cache: &mut BTreeMap<u64, Result<Vec<u16>, String>>,
	assess: bool, mut unchanged: bool, killed: KillSwitch)
-> Result<Option<(&'a lookup::Entry, Repair)>, String> {
	let mut verified: Option<(&lookup::Entry, Repair)> = None;
	let mut ambiguous = false;
	for entry in entries {
		if killed.killed() { return Err("calculation cancelled".into()); }
		let advertised = entry.reference.len();
		let label = format!("{:08x} (record {}, confidence {})", entry.disc_crc, entry.id, entry.confidence);
		if entry.parity_url.is_none() {
			if assess { eprintln!("  {label} — no supported full parity record"); }
			continue;
		}
		let parity = match parity_cache.entry(entry.id).or_insert_with(|| lookup::parity(entry)) {
			Ok(data) => data,
			Err(err) => {
				eprintln!("  {label} — {err}");
				if assess {
					eprintln!("    CTDB advertised {advertised} syndrome rows.");
					report_capacity(snapshot, advertised, 0, "assuming zero offset");
				}
				continue;
			},
		};
		let npar = parity.len() / (STRIDE * 2);
		let repair = match snapshot.find(entry, parity, killed) {
			Ok(repair) => repair,
			Err(Rejection::Cancelled) => return Err("calculation cancelled".into()),
			Err(reason) => {
				eprintln!("  {label} — {}", reason.description());
				ambiguous |= reason == Rejection::Ambiguous;
				if assess { report_capacity(snapshot, npar, 0, "assuming zero offset"); }
				continue;
			},
		};

		if assess {
			eprintln!("  {label} — CRC-verified at offset {:+} frames, {} changed channel words",
				repair.offset, repair.corrections.len());
			report_capacity(snapshot, npar, repair.offset, "verified alignment");
		}
		if repair.corrections.is_empty() && !assess { return Ok(Some((entry, repair))); }
		if repair.corrections.is_empty() {
			unchanged = true;
		}
		else if let Some((_, current)) = &verified {
			ambiguous |= current.corrections != repair.corrections;
		}
		else { verified = Some((entry, repair)); }
	}

	if unchanged {
		eprintln!("CTDB repair — saved audio already matches a qualifying CTDB record");
		return Ok(None);
	}
	if ambiguous { return Err("CTDB records or offsets imply different verified corrections".into()); }
	verified.map(Some).ok_or_else(|| "no CRC-verified repair at the requested confidence".into())
}

/// Report the Reed–Solomon bound alongside the measured distribution.
fn report_capacity(snapshot: &Snapshot<'_>, roots: usize, offset: isize, alignment: &str) {
	let limits = snapshot.capacity(offset, roots);
	let unknown = roots / 2;
	eprintln!("    {roots} syndrome rows per lane, up to {unknown} unknown errors or {roots} erasures");
	eprintln!("    {alignment}: peak {} suspected and {} known erasures in one lane",
		limits.max_suspect, limits.max_erasures);
	eprintln!("    peak mixed cost {} (2 × unknowns + erasures), {} lanes exceed {roots}",
		limits.max_mixed_cost, limits.lanes_over_mixed);
	eprintln!("    {} lanes exceed the all-unknown bound, {} exceed the known-erasure bound, {} uncertain frames outside coverage",
		limits.lanes_over_unknowns, limits.lanes_over_erasures, limits.outside);
	if limits.lanes_over_erasures != 0 {
		eprintln!("    Recovery cannot be guaranteed if every reported erasure is wrong.");
	}
	else if limits.lanes_over_mixed != 0 {
		eprintln!("    Recovery depends on some suspect frames being correct or being located as erasures.");
	}
	else if limits.lanes_over_unknowns != 0 {
		eprintln!("    The observed suspects fit when the reported erasure positions are used.");
	}
	else {
		eprintln!("    The observed suspect positions fit the theoretical unknown-error bound.");
	}
}

/// Encode one stereo frame in WAV byte order.
const fn pcm_frame(words: [u16; 2]) -> [u8; 4] {
	let [a, b] = words[0].to_le_bytes();
	let [c, d] = words[1].to_le_bytes();
	[a, b, c, d]
}
