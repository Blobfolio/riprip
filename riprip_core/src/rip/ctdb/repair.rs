//! Interleaving, pressing offsets, and CRC verification for CTDB repair.

use super::{lookup::Entry, rs::{Field, MAX_PARITY}};
use crate::KillSwitch;
use crc32fast::Hasher;
use std::ops::Range;

/// One quality classification per stereo frame in a recovery snapshot.
#[derive(Clone, Copy, Eq, PartialEq)]
pub(super) enum Quality {
	/// Independently verified by the existing per-track verifier.
	Trusted,
	/// Repeated physical reads agreed.
	Likely,
	/// A physical candidate exists, but lacks a qualifying majority.
	Weak,
	/// The drive reported C2, or a frame could not be read.
	Erasure,
}

/// The interval that CTDB covers and its shortened RS dimensions.
#[derive(Clone, Copy)]
pub(super) struct Geometry {
	/// Stereo frames per row.
	pub(super) stride: usize,
	/// Number of complete protected rows.
	pub(super) rows: usize,
}

impl Geometry {
	/// Determine the number of full protected rows in the audio image.
	pub(super) fn new(frames: usize, stride: usize) -> Option<Self> {
		let rows = frames.checked_div(stride)?.checked_sub(2)?;
		if rows == 0 || rows + MAX_PARITY > 65_535 { return None; }
		Some(Self { stride, rows })
	}

	/// Two 16-bit lanes per stereo frame in a row.
	pub(super) const fn lanes(self) -> usize { self.stride * 2 }

	/// Protected range shifted in local audio coordinates.
	pub(super) const fn window(self, offset: isize) -> Range<usize> {
		let start = self.stride.checked_add_signed(offset).expect("offset within one stride");
		start..start + self.rows * self.stride
	}
}

/// Interleaved lane load for a particular pressing alignment.
#[derive(Default)]
pub(super) struct Capacity {
	/// Largest number of known locations among the lanes.
	pub(super) max_erasures: usize,
	/// Largest number of suspect locations, including the known erasures.
	pub(super) max_suspect: usize,
	/// Largest error/erasure budget assuming every suspect frame is wrong.
	pub(super) max_mixed_cost: usize,
	/// Lanes whose known erasures alone would exceed the parity budget.
	pub(super) lanes_over_erasures: usize,
	/// Lanes that would exceed the budget without erasure information.
	pub(super) lanes_over_unknowns: usize,
	/// Lanes that would exceed the budget even with the known locations.
	pub(super) lanes_over_mixed: usize,
	/// Suspect stereo frames beyond either edge of the protected window.
	pub(super) outside: usize,
}

/// A CRC-verified set of XOR corrections in original, drive-corrected coordinates.
#[derive(Clone, Eq, PartialEq)]
pub(super) struct Repair {
	/// Pressing alignment in stereo frames.
	pub(super) offset: isize,
	/// Word positions in the local audio image and their XOR changes.
	pub(super) corrections: Vec<(usize, u16)>,
}

/// Furthest validation reached without obtaining a unique repair.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum Rejection {
	/// No alignment passed the sampled codeword checks.
	Alignment,
	/// The candidate could not correct every lane while preserving trusted words.
	Codeword,
	/// A complete decoding disagreed with the published checksums.
	Checksum,
	/// Two alignments yielded distinct checksum-verified changes.
	Ambiguous,
	/// The caller cancelled the work.
	Cancelled,
}

impl Rejection {
	/// A report for the command-line assessment.
	pub(super) const fn description(self) -> &'static str {
		match self {
			Self::Alignment => "no supported offset passed the sampled codeword checks",
			Self::Codeword => "no alignment could correct every lane while preserving verified audio",
			Self::Checksum => "decoded candidate failed the published CRCs",
			Self::Ambiguous => "different corrections passed verification at multiple offsets",
			Self::Cancelled => "calculation cancelled",
		}
	}
}

/// A codeword's known erasures and all uncertain positions, capped for decoding.
#[derive(Default)]
struct LaneHints {
	/// Leading known-erasure rows, kept up to the decoding budget.
	erased: [usize; MAX_PARITY + 2],
	/// Leading suspect rows, including possible erasures.
	suspect: [usize; MAX_PARITY + 2],
	/// Total known-erasure count including positions omitted above.
	erasures: usize,
	/// Total suspect count including positions omitted above.
	suspects: usize,
}

/// Ordering for credible location hints when a lane has several decodes.
#[derive(Clone, Copy)]
enum Hints {
	/// Prefer C2 and unread positions.
	Known,
	/// Prefer correcting without erasures, in case C2 over-reported.
	Unknown,
	/// Prefer treating all low-quality positions as located erasures.
	Suspect,
}

impl LaneHints {
	/// Add evidence from one row without growing the decoding arrays.
	fn remember(&mut self, row: usize, quality: Quality) {
		if quality == Quality::Erasure {
			if let Some(slot) = self.erased.get_mut(self.erasures) { *slot = row; }
			self.erasures += 1;
		}
		if matches!(quality, Quality::Weak | Quality::Erasure) {
			if let Some(slot) = self.suspect.get_mut(self.suspects) { *slot = row; }
			self.suspects += 1;
		}
	}

	/// Replace the edge row that rolls into or out of a shifted lane.
	fn shifted(&self, geometry: Geometry, rollover: isize, before: Quality, first: Quality,
		last: Quality, after: Quality) -> Self {
		// A sub-stride offset preserves each lane's order and can replace
		// at most its first or last element.
		let map = |rows: &[usize], count: usize, drop: bool, add: bool, out: &mut [usize]| {
			let mut len = if add && rollover < 0 { out[0] = 0; 1 } else { 0 };
			for &row in &rows[..count.min(rows.len())] {
				if let Some(row) = row.checked_add_signed(-rollover) &&
					row < geometry.rows &&
					let Some(slot) = out.get_mut(len)
				{
					*slot = row;
					len += 1;
				}
			}
			if add && rollover > 0 && let Some(slot) = out.get_mut(len) {
				*slot = geometry.rows - 1;
			}
			count - usize::from(drop) + usize::from(add)
		};
		let (drop, add) = if rollover > 0 { (first, after) } else { (last, before) };
		let mut out = Self::default();
		out.erasures = map(&self.erased, self.erasures, drop == Quality::Erasure,
			add == Quality::Erasure, &mut out.erased);
		let suspect = |q| matches!(q, Quality::Weak | Quality::Erasure);
		out.suspects = map(&self.suspect, self.suspects, suspect(drop), suspect(add),
			&mut out.suspect);
		out
	}
}

/// The reusable unshifted syndromes and quality distribution of one snapshot.
pub(super) struct Snapshot<'a> {
	/// Field shared by syndrome preparation and RS decoding.
	field: &'a Field,
	/// Image dimensions and protected window.
	geometry: Geometry,
	/// The original PCM, referenced for the checksums and offset scan.
	pcm: &'a [u16],
	/// Evidence per stereo frame, including the excluded edges.
	quality: &'a [Quality],
	/// Calculated unshifted syndromes, in wire order.
	syndromes: Vec<u16>,
	/// Unshifted quality information, one entry per stereo frame in a row.
	hints: Vec<LaneHints>,
}

impl<'a> Snapshot<'a> {
	/// Evaluate each unshifted lane once, independent of the lookup entries.
	pub(super) fn new(field: &'a Field, geometry: Geometry, pcm: &'a [u16],
		quality: &'a [Quality], roots: usize, killed: KillSwitch) -> Option<Self> {
		let lanes = geometry.lanes();
		let range = geometry.window(0);
		let samples = &pcm[2 * range.start..2 * range.end];
		let mut syndromes = vec![0_u16; lanes * roots];
		for (root, sums) in syndromes.chunks_exact_mut(lanes).enumerate() {
			if killed.killed() { return None; }
			let [low, high] = field.multiplier(field.alpha(root));
			for row in samples.chunks_exact(lanes) {
				for (sum, &symbol) in sums.iter_mut().zip(row) {
					let [lo, hi] = sum.to_le_bytes();
					*sum = low[usize::from(lo)] ^ high[usize::from(hi)] ^ symbol;
				}
			}
		}
		let mut hints: Vec<_> = (0..geometry.stride).map(|_| LaneHints::default()).collect();
		for (row, quality) in quality[range].chunks_exact(geometry.stride).enumerate() {
			for (hint, &quality) in hints.iter_mut().zip(quality) { hint.remember(row, quality); }
		}
		Some(Self { field, geometry, pcm, quality, syndromes, hints })
	}

	/// Assess the hard erasure and unknown-error bounds separately.
	pub(super) fn capacity(&self, offset: isize, roots: usize) -> Capacity {
		let mut out = Capacity::default();
		for lane in 0..self.geometry.stride {
			self.with_hints(lane, offset, |hint| {
				out.max_erasures = out.max_erasures.max(hint.erasures);
				out.max_suspect = out.max_suspect.max(hint.suspects);
				let mixed_cost = hint.erasures + 2 * (hint.suspects - hint.erasures);
				out.max_mixed_cost = out.max_mixed_cost.max(mixed_cost);
				out.lanes_over_erasures += 2 * usize::from(hint.erasures > roots);
				out.lanes_over_unknowns += 2 * usize::from(hint.suspects > roots / 2);
				out.lanes_over_mixed += 2 * usize::from(mixed_cost > roots);
			});
		}
		let window = self.geometry.window(offset);
		out.outside = self.quality[..window.start].iter().chain(&self.quality[window.end..])
			.filter(|&&quality| matches!(quality, Quality::Weak | Quality::Erasure)).count();
		out
	}

	/// Find an alignment whose corrections satisfy the published checksums.
	pub(super) fn find(&self, entry: &Entry, reference: &[u16], killed: KillSwitch)
	-> Result<Repair, Rejection> {
		let lanes = self.geometry.lanes();
		let limit = isize::try_from(self.geometry.stride).map_err(|_| Rejection::Alignment)? - 1;
		let offsets = std::iter::once(0).chain((1..=limit).flat_map(|n| [n, -n]));
		let mut verified: Option<Repair> = None;
		let mut reason = Rejection::Alignment;
		for offset in offsets {
			if killed.killed() { return Err(Rejection::Cancelled); }
			let roots = reference.len() / lanes;
			// Any valid alignment has decodable syndromes in these lanes.
			// They span the stride so a shifted or silent first lane does
			// not cause a full decode for every offset.
			let anchors = [0, 1, lanes / 4, lanes / 2, 3 * lanes / 4, lanes - 1];
			if !anchors.into_iter().all(|lane| {
				self.with_hints(lane / 2, offset, |hint| {
					self.decode_lane(lane, offset, reference, roots, hint, Hints::Known).is_some()
				})
			}) { continue; }

			if reason == Rejection::Alignment { reason = Rejection::Codeword; }
			let window = self.geometry.window(offset);
			for hints in [Hints::Known, Hints::Unknown, Hints::Suspect] {
				let Some(corrections) = self.solve(offset, reference, hints, killed) else { break; };
				// Identical patches give exactly the same local PCM, even
				// when more than one pressing alignment can explain it.
				if verified.as_ref().is_some_and(|repair| repair.corrections == corrections) { break; }
				reason = Rejection::Checksum;
				if checksum(self.pcm, window.clone(), &corrections) == entry.disc_crc &&
					matches_tracks(self.pcm, entry, window.clone(), offset, &corrections)
				{
					if corrections.is_empty() { return Ok(Repair { offset, corrections }); }
					if verified.is_some() { return Err(Rejection::Ambiguous); }
					verified = Some(Repair { offset, corrections });
					break;
				}
			}
		}
		if killed.killed() { return Err(Rejection::Cancelled); }
		verified.ok_or(reason)
	}

	/// Assemble a complete candidate at one alignment.
	fn solve(&self, offset: isize, reference: &[u16], hints: Hints, killed: KillSwitch)
	-> Option<Vec<(usize, u16)>> {
		let lanes = self.geometry.lanes();
		let start = self.geometry.window(offset).start * 2;
		let roots = reference.len() / lanes;
		let mut corrections = Vec::new();
		for frame in 0..self.geometry.stride {
			if killed.killed() { return None; }
			self.with_hints(frame, offset, |hint| {
				for lane in [frame * 2, frame * 2 + 1] {
					let changes = self.decode_lane(lane, offset, reference, roots, hint, hints)?;
					corrections.extend(changes.into_iter().map(|(row, magnitude)| {
						(start + lane + row * lanes, magnitude)
					}));
				}
				Some(())
			})?;
		}
		corrections.sort_unstable_by_key(|&(word, _)| word);
		Some(corrections)
	}

	/// Re-index a lane at a pressing offset using the unshifted recovery statistics.
	fn with_hints<T>(&self, frame: usize, offset: isize, f: impl FnOnce(&LaneHints) -> T) -> T {
		let stride = self.geometry.stride;
		let shifted = stride.checked_add_signed(offset).expect("offset within one stride") + frame;
		let lane = shifted % stride;
		let hint = &self.hints[lane];
		let rollover = (shifted / stride).cast_signed() - 1;
		if rollover == 0 { return f(hint); }
		let end = self.geometry.rows * stride + lane;
		f(&hint.shifted(self.geometry, rollover, self.quality[lane],
			self.quality[stride + lane], self.quality[end], self.quality[end + stride]))
	}

	/// Recover one lane without modifying the underlying samples.
	fn decode_lane(&self, lane: usize, offset: isize, reference: &[u16],
		roots: usize, hint: &LaneHints, order: Hints) -> Option<Vec<(usize, u16)>> {
		let lanes = self.geometry.lanes();
		let window = self.geometry.window(offset);
		let start = window.start * 2 + lane;
		let first_frame = start / 2;
		let mut delta = [0; MAX_PARITY];
		for (root, diff) in delta[..roots].iter_mut().enumerate() {
			*diff = self.shifted_syndrome(root, start) ^ reference[root * lanes + lane];
		}

		let erased = hint.erased.get(..hint.erasures);
		let suspect = if hint.suspects == hint.erasures { None }
		else { hint.suspect.get(..hint.suspects) };
		let unknown = Some(&[][..]);
		let orders = match order {
			Hints::Known => [erased.filter(|_| hint.erasures != 0), unknown, suspect],
			Hints::Unknown => [unknown, erased.filter(|_| hint.erasures != 0), suspect],
			Hints::Suspect => [suspect, erased.filter(|_| hint.erasures != 0), unknown],
		};
		orders.into_iter().flatten().find_map(|rows| {
			self.field.decode(&delta[..roots], self.geometry.rows, rows)
				.filter(|patch| patch.iter().all(|&(row, _)| {
					self.quality[first_frame + row * self.geometry.stride] != Quality::Trusted
				}))
		})
	}

	/// Evaluate the shifted lane by dropping and adding its edge terms.
	fn shifted_syndrome(&self, root: usize, start: usize) -> u16 {
		let lanes = self.geometry.lanes();
		let lane = start % lanes;
		let base = self.syndromes[root * lanes + lane];
		let last = self.geometry.rows * lanes + lane;
		let leading_power = root * (self.geometry.rows - 1);
		if start < lanes {
			self.field.mul_alpha(base ^ self.pcm[last], 65_535 - root) ^
				self.field.mul_alpha(self.pcm[lane], leading_power)
		}
		else if start >= 2 * lanes {
			self.field.mul_alpha(base ^ self.field.mul_alpha(self.pcm[lanes + lane], leading_power), root) ^
				self.pcm[last + lanes]
		}
		else { base }
	}
}

/// Check every published track CRC in the pressing's track coordinates.
pub(super) fn matches_tracks(pcm: &[u16], entry: &Entry, window: Range<usize>, offset: isize,
	corrections: &[(usize, u16)]) -> bool {
	entry.checks.iter().all(|(track, expected)| {
		let start = if track.start == 0 { Some(window.start) }
		else { track.start.checked_add_signed(offset) };
		let end = if track.end == pcm.len() / 2 { Some(window.end) }
		else { track.end.checked_add_signed(offset) };
		start.zip(end).is_some_and(|(start, end)| {
			start < end && end <= pcm.len() / 2 && checksum(pcm, start..end, corrections) == *expected
		})
	})
}

/// Calculate the clean-audio CRC for each alignment without rescanning the disc.
pub(super) fn unchanged_crcs(pcm: &[u16], geometry: Geometry, killed: KillSwitch)
-> Vec<(isize, u32)> {
	let limit = isize::try_from(geometry.stride).expect("CD stride fits isize") - 1;
	let early = geometry.window(-limit);
	let late = geometry.window(limit);
	let offsets = std::iter::once(0).chain((1..=limit).flat_map(|n| [n, -n]));
	if late.start >= early.end {
		return offsets.take_while(|_| !killed.killed()).map(|offset| {
			(offset, checksum(pcm, geometry.window(offset), &[]))
		}).collect();
	}
	let bytes = |range: Range<usize>| -> Vec<_> {
		pcm[2 * range.start..2 * range.end].iter().flat_map(|word| word.to_le_bytes()).collect()
	};
	let head = bytes(early.start..late.start);
	let tail = bytes(early.end..late.end);
	let mut middle = Hasher::new();
	for block in pcm[2 * late.start..2 * early.end].chunks(4096) {
		let mut packed = [0; 8192];
		for (&word, out) in block.iter().zip(packed.as_chunks_mut::<2>().0) {
			*out = word.to_le_bytes();
		}
		middle.update(&packed[..block.len() * 2]);
		if killed.killed() { return Vec::new(); }
	}
	offsets.take_while(|_| !killed.killed()).map(|offset| {
		let edge = usize::try_from(offset + limit).expect("offset in scan range") * 4;
		let mut hasher = Hasher::new();
		hasher.update(&head[edge..]);
		hasher.combine(&middle);
		hasher.update(&tail[..edge]);
		(offset, hasher.finalize())
	}).collect()
}

/// Stream a checksummed interval with the proposed corrections applied.
pub(super) fn checksum(pcm: &[u16], range: Range<usize>, corrections: &[(usize, u16)]) -> u32 {
	let mut crc = Hasher::new();
	let mut buffer = [0; 8192];
	let first = range.start * 2;
	let end = range.end * 2;
	let mut changes = corrections[corrections.partition_point(|&(word, _)| word < first)..]
		.iter().peekable();
	for (block, symbols) in pcm[first..end].chunks(buffer.len() / 2).enumerate() {
		let start = first + block * (buffer.len() / 2);
		for (i, (&symbol, bytes)) in symbols.iter().zip(buffer.as_chunks_mut::<2>().0).enumerate() {
			let mut symbol = symbol;
			if changes.peek().is_some_and(|&&(word, _)| word == start + i) {
				symbol ^= changes.next().expect("checked correction").1;
			}
			*bytes = symbol.to_le_bytes();
		}
		crc.update(&buffer[..symbols.len() * 2]);
	}
	crc.finalize()
}
