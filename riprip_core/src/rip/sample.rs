/*!
# Rip Rip Hooray: Samples
*/

use crate::{
	NULL_SAMPLE,
	Sample,
	SAMPLES_PER_SECTOR,
};
use std::{
	cmp::Ordering,
	io::{
		Read,
		Write,
	},
	ops::Range,
};



/// # `RipSample` Variant ID range.
const DATA_KIND_RNG: Range<u8> = 1..9;



#[derive(Debug, Clone, Default, Eq, Hash, PartialEq)]
/// # Rip Sample.
///
/// This enum combines sample value(s) and status(es).
pub(crate) enum RipSample {
	/// # Leadin/out.
	Lead,

	#[default]
	/// # Unread samples.
	Tbd,

	/// Samples that came down with C2 or read errors.
	Bad(Sample),

	/// Allegedly good sample(s).
	Maybe(ContentiousSample),
}

impl RipSample {
	/// # As Array.
	///
	/// Return the most appropriate single sample 4-byte value as an array.
	pub(crate) const fn as_array(&self) -> Sample {
		match self {
			Self::Tbd | Self::Lead => NULL_SAMPLE,
			Self::Bad(s) => *s,
			Self::Maybe(s) => s.as_array(),
		}
	}

	/// # As Slice.
	///
	/// Return the most appropriate single sample 4-byte value as a slice.
	pub(crate) const fn as_slice(&self) -> &[u8] {
		match self {
			Self::Tbd | Self::Lead => NULL_SAMPLE.as_slice(),
			Self::Bad(s) => s.as_slice(),
			Self::Maybe(s) => s.as_slice(),
		}
	}

	/// # Data Kind.
	///
	/// Return the variant ID used for de/serialization.
	const fn data_kind(&self) -> u8 {
		match self {
			Self::Lead => 1,
			Self::Tbd => 2,
			Self::Bad(_) => 3,
			Self::Maybe(ContentiousSample::Maybe1((_, count))) =>
				if 1 == *count { 4 } // Implicit count of one.
				else { 5 },          // Explicit other count.
			Self::Maybe(ContentiousSample::Maybe2(_)) => 6,
			Self::Maybe(ContentiousSample::Maybe3(_)) => 7,
			Self::Maybe(ContentiousSample::Strict(_)) => 8,
		}
	}
}

impl RipSample {
	/// # Is Bad?
	pub(crate) const fn is_bad(&self) -> bool { matches!(self, Self::Tbd | Self::Bad(_)) }

	/// # Is Confused.
	///
	/// Returns true if the data has been so inconsistent as to warrant strict
	/// handling.
	pub(crate) const fn is_confused(&self) -> bool {
		matches!(self, Self::Maybe(ContentiousSample::Strict(_)))
	}

	/// # Is Contentious?
	///
	/// Only applies to a maybe with more than one value.
	pub(crate) const fn is_contentious(&self) -> bool {
		matches!(
			self,
			Self::Maybe(
				ContentiousSample::Maybe2(_) |
				ContentiousSample::Maybe3(_) |
				ContentiousSample::Strict(_)
			)
		)
	}

	/// # Likeliness.
	///
	/// Return the minimum reread abs/mul values to make the sample likely.
	pub(crate) const fn is_likely(&self, rereads: (u8, u8)) -> bool {
		match self {
			// Leadin/out is always likely.
			Self::Lead => true,
			Self::Maybe(s) => {
				let (a, mut b) = s.contention();
				b = b.saturating_mul(rereads.1);
				if b == u8::MAX { b -= 1; }
				rereads.0 <= a && b <= a
			}
			// Never likely.
			_ => false,
		}
	}
}

impl RipSample {
	/// # Update Sample.
	///
	/// See `update_bad` for what happens if there's a C2 error. Otherwise,
	/// this method changes things as follows:
	///
	/// TBD and Bad samples are simply replaced.
	///
	/// Maybe samples are incremented or augmented.
	///
	/// Contentious values are incremented if the new value matches, or the
	/// new value is added to the end of the list. (If the only reason for
	/// contention was a sync error and that is fixed by the new read, it is
	/// changed to Maybe.)
	///
	/// Leadin/out stays the same.
	pub(crate) fn update(&mut self, new: Sample, err_c2: bool, all_good: bool) {
		// Send bad samples to a different method to halve the onslaught of
		// conditional handling. Haha.
		if err_c2 { return self.update_bad(new); }

		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => {
				*self = Self::Maybe(ContentiousSample::new(new));
			},

			// Maybes
			Self::Maybe(s) =>
				// Strict samples can only be updated if all good.
				if all_good || ! matches!(s, ContentiousSample::Strict(_)) {
					s.add_good(new);
				},

			// Leave leadin/out samples alone.
			Self::Lead => {},
		}
	}

	/// # Update New Bad Sample.
	///
	/// TBD and Bad samples are simply replaced.
	///
	/// Maybe are decremented/downgraded if the value matches and there is no
	/// sync weirdness.
	///
	/// Leadin/out stays the same.
	fn update_bad(&mut self, new: Sample) {
		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => { *self = Self::Bad(new); },

			// Simple Maybes.
			Self::Maybe(s) => if let Some(boo) = s.add_bad(new) {
				*self = boo;
			},

			// Leave leadin/out samples alone.
			Self::Lead => {},
		}
	}
}



#[derive(Debug)]
/// # Sector of Samples.
///
/// Sample de/serialization sucks because they're tiny and variable in size. To
/// improve the efficiency a little bit — and reduce the disk space a touch —
/// they're read/written in sector-sized bocks.
///
/// This struct serves as a buffer for both use cases.
///
/// Variant identifiers are stored and read/written separately from the actual
/// data because they have a fixed size. The data portion has a variable
/// length, but it can be derived from the identifiers, allowing us to reduce
/// the total in/out operations to two instead of 588. Haha.
///
/// Lastly, because there are so many goddamn samples in a typical song, the
/// variant IDs are stored in a packed `u4`-esque format to halve their size.
pub(super) struct RipSector {
	/// # Kinds.
	kind: [u8; (SAMPLES_PER_SECTOR as usize).wrapping_div(2)],

	/// # Data.
	data: [u8; SAMPLES_PER_SECTOR as usize * 15],
}

impl RipSector {
	/// # Deserialize From Reader.
	///
	/// Fill the buffers with data from a reader, then return an iterator over
	/// those samples.
	pub(super) fn deserialize_from<R: Read>(&mut self, r: &mut R) -> Option<RipSectorSamples> {
		// Read and validate the type IDs, and calculate the expected data length.
		r.read_exact(&mut self.kind).ok()?;
		let mut len = 0;
		for &v in &self.kind {
			let (a, b) = u4_unpack(v);
			if DATA_KIND_RNG.contains(&a) && DATA_KIND_RNG.contains(&b) {
				len += usize::from(data_len_by_kind(a));
				len += usize::from(data_len_by_kind(b));
			}
			else { return None; }
		}

		// Read the data, then return an iterator over the samples.
		r.read_exact(&mut self.data[..len]).ok()?;
		Some(RipSectorSamples {
			kind: &self.kind,
			data: &self.data,
			pos: 0,
		})
	}

	#[expect(unused_assignments, reason = "We're initializing?")]
	/// # Serialize Into Writer.
	///
	/// Fill the buffers with the sample data from `src`, then write them into
	/// the writer.
	///
	/// The type information is written first in a packed format — two `u4`
	/// per byte — followed by the data, whose length will be variable
	/// depending on the sample variants used.
	pub(super) fn serialize_into<W: Write>(&mut self, src: &[RipSample], w: &mut W)
	-> Option<()> {
		// Sanity check: this should never fail, but just in case.
		if src.len() != usize::from(SAMPLES_PER_SECTOR) { return None; }

		// Set the type headers.
		for (k, v) in self.kind.iter_mut().zip(src.chunks_exact(2)) {
			*k = u4_pack(v[0].data_kind(), v[1].data_kind());
		}

		// Set the data. We might as well just work with subslices for this
		// instead of Read/Write, since we've already got the buffer handy.
		// While we're at it, let's also find out how much data we've populated
		// so we can write the right amount.
		let len = self.data.len() - {
			let mut data: &mut [u8] = &mut self.data;
			let mut current: &mut [u8] = &mut [];
			for v in src {
				match v {
					RipSample::Bad(s) => {
						(current, data) = data.split_at_mut(4);
						current.copy_from_slice(s.as_slice());
					},
					RipSample::Maybe(ContentiousSample::Maybe1(pair)) =>
						if 1 == pair.1 {
							(current, data) = data.split_at_mut(4);
							current.copy_from_slice(pair.0.as_slice());
						}
						else {
							(current, data) = data.split_at_mut(5);
							current[..4].copy_from_slice(pair.0.as_slice());
							current[4] = pair.1;
						},
					RipSample::Maybe(ContentiousSample::Maybe2(set)) => {
						(current, data) = data.split_at_mut(10);
						current[..4].copy_from_slice(set[0].0.as_slice());
						current[4] = set[0].1;
						current[5..9].copy_from_slice(set[1].0.as_slice());
						current[9] = set[1].1;
					},
					RipSample::Maybe(ContentiousSample::Maybe3(set) | ContentiousSample::Strict(set)) => {
						(current, data) = data.split_at_mut(15);
						current[..4].copy_from_slice(set[0].0.as_slice());
						current[4] = set[0].1;
						current[5..9].copy_from_slice(set[1].0.as_slice());
						current[9] = set[1].1;
						current[10..14].copy_from_slice(set[2].0.as_slice());
						current[14] = set[2].1;
					},
					_ => {},
				}
			}

			// Return the leftover data so we can figure out how much was
			// written to.
			data.len()
		};

		// Write the types and data!
		w.write_all(self.kind.as_slice()).ok()?;
		w.write_all(&self.data[..len]).ok()
	}
}

impl RipSector {
	/// # New.
	pub(super) const fn new() -> Self {
		Self {
			kind: [0_u8; (SAMPLES_PER_SECTOR as usize).wrapping_div(2)],
			data: [0_u8; SAMPLES_PER_SECTOR as usize * 15],
		}
	}
}



#[derive(Debug)]
/// # Sector Sample Iterator.
///
/// This iterator returns each sample stored in the `RipSector`. There should
/// always be exactly `588` of them.
pub(super) struct RipSectorSamples<'a> {
	/// # Kinds.
	kind: &'a [u8],

	/// # Data.
	data: &'a [u8],

	/// # Current Index.
	pos: u16,
}

impl<'a> Iterator for RipSectorSamples<'a> {
	type Item = RipSample;

	fn next(&mut self) -> Option<Self::Item> {
		let idx = usize::from(self.pos.wrapping_div(2));
		if self.kind.len() <= idx { None }
		else {
			// Tease out the kind.
			let kind =
				if 0 == self.pos & 1 { u4_unpack_lhs(self.kind[idx]) }
				else { u4_unpack_rhs(self.kind[idx]) };
			self.pos += 1;

			// Parse it!
			match kind {
				1 => Some(RipSample::Lead),
				2 => Some(RipSample::Tbd),
				3 =>
					if 4 <= self.data.len() {
						let (data, rest) = self.data.split_at(4);
						self.data = rest;
						Some(RipSample::Bad([data[0], data[1], data[2], data[3]]))
					}
					else { None },
				4 =>
					if 4 <= self.data.len() {
						let (data, rest) = self.data.split_at(4);
						self.data = rest;
						Some(RipSample::Maybe(ContentiousSample::Maybe1((
							[data[0], data[1], data[2], data[3]],
							1,
						))))
					}
					else { None },
				5 =>
					if 5 <= self.data.len() {
						let (data, rest) = self.data.split_at(5);
						self.data = rest;
						Some(RipSample::Maybe(ContentiousSample::Maybe1((
							[data[0], data[1], data[2], data[3]],
							data[4],
						))))
					}
					else { None },
				6 =>
					if 10 <= self.data.len() {
						let (data, rest) = self.data.split_at(10);
						self.data = rest;
						Some(RipSample::Maybe(ContentiousSample::Maybe2([
							([data[0], data[1], data[2], data[3]], data[4]),
							([data[5], data[6], data[7], data[8]], data[9]),
						])))
					}
					else { None },
				7 | 8 =>
					if 15 <= self.data.len() {
						let (data, rest) = self.data.split_at(15);
						self.data = rest;
						let set = [
							([data[0],  data[1],  data[2],  data[3]],  data[4]),
							([data[5],  data[6],  data[7],  data[8]],  data[9]),
							([data[10], data[11], data[12], data[13]], data[14]),
						];
						if kind == 7 {
							Some(RipSample::Maybe(ContentiousSample::Maybe3(set)))
						}
						else {
							Some(RipSample::Maybe(ContentiousSample::Strict(set)))
						}
					}
					else { None },
				// This shouldn't be reachable.
				_ => None,
			}
		}
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl<'a> ExactSizeIterator for RipSectorSamples<'a> {
	fn len(&self) -> usize {
		usize::from(SAMPLES_PER_SECTOR.saturating_sub(self.pos))
	}
}



#[expect(clippy::missing_docs_in_private_items, reason = "Self-explanatory.")]
#[derive(Debug, Clone, Eq, Hash, PartialEq)]
/// # Contentious Rip Sample.
///
/// This structure holds up to 3 sample values sorted by popularity and age.
/// This is used instead of a straight vector because there shouldn't be _that_
/// many contradictory samples, and since the counts are fixed, we don't have
/// to worry about silly bounds checks or non-const indexing constraints.
///
/// This is also eight bytes smaller than a `Vec<(Sample, u8)>`, which isn't
/// anything to sneeze at given the scale of data we're working with. Haha.
pub(crate) enum ContentiousSample {
	Maybe1((Sample, u8)),
	Maybe2([(Sample, u8); 2]),
	Maybe3([(Sample, u8); 3]),
	Strict([(Sample, u8); 3]),
}

impl ContentiousSample {
	#[inline]
	/// # New.
	const fn new(new: Sample) -> Self { Self::Maybe1((new, 1)) }
}

impl ContentiousSample {
	/// # As Array.
	const fn as_array(&self) -> Sample {
		match self {
			Self::Maybe1((s, _)) => *s,
			Self::Maybe2(set) => set[0].0,
			Self::Maybe3(set) | Self::Strict(set) => set[0].0,
		}
	}

	/// # As Slice.
	const fn as_slice(&self) -> &[u8] {
		match self {
			Self::Maybe1((s, _)) => s.as_slice(),
			Self::Maybe2(set) => set[0].0.as_slice(),
			Self::Maybe3(set) | Self::Strict(set) => set[0].0.as_slice(),
		}
	}

	/// # Contention.
	///
	/// Return the first (best) total, and the total of all the rest.
	const fn contention(&self) -> (u8, u8) {
		match self {
			Self::Maybe1((_, c)) => (*c, 0),
			Self::Maybe2(set) => (set[0].1, set[1].1),
			Self::Maybe3(set) | Self::Strict(set) => (
				set[0].1,
				set[1].1.saturating_add(set[2].1),
			),
		}
	}
}

impl ContentiousSample {
	/// # Add (Bad) Sample.
	///
	/// If the value already exists, the count is _decreased_. If it reaches
	/// zero, it is removed, potentially downgrading the maybe number.
	///
	/// Returns a bad sample if the value can no longer be held (e.g. a `Maybe1`
	/// with a count of zero).
	fn add_bad(&mut self, new: Sample) -> Option<RipSample> {
		match self {
			Self::Maybe1((old, count)) =>
				if new.eq(old) {
					// We can't drop the only entry; go bad!
					if *count == 1 { return Some(RipSample::Bad(new)); }
					*count -= 1;
				},
			Self::Maybe2([(old1, count1), (old2, count2)]) =>
				if new.eq(old1) {
					// Drop, keeping 2.
					if *count1 == 1 { *self = Self::Maybe1((*old2, *count2)); }
					// Decrement and maybe resort.
					else {
						*count1 -= 1;
						if *count1 < *count2 { self.sort(); }
					}
				}
				else if new.eq(old2) {
					// Drop, keeping 1.
					if *count2 == 1 { *self = Self::Maybe1((*old1, *count1)); }
					// Decrement.
					else { *count2 -= 1; }
				},
			Self::Maybe3([(old1, count1), (old2, count2), (old3, count3)]) =>
				if new.eq(old1) {
					// Drop to Maybe2, keeping 2,3.
					if *count1 == 1 {
						*self = Self::Maybe2([(*old2, *count2), (*old3, *count3)]);
					}
					// Decrement and maybe resort.
					else {
						*count1 -= 1;
						if *count1 < *count2 { self.sort(); }
					}
				}
				else if new.eq(old2) {
					// Drop to Maybe2, keeping 1,3.
					if *count2 == 1 {
						*self = Self::Maybe2([(*old1, *count1), (*old3, *count3)]);
					}
					// Decrement and maybe resort.
					else {
						*count2 -= 1;
						if *count2 < *count3 { self.sort(); }
					}
				}
				else if new.eq(old3) {
					// Drop to Maybe2, keeping 1,2.
					if *count3 == 1 {
						*self = Self::Maybe2([(*old1, *count1), (*old2, *count2)]);
					}
					// Decrement.
					else { *count3 -= 1; }
				},
			Self::Strict(set) =>
				if new == set[0].0 {
					// Keep the count, but shift it to the end.
					if set[0].1 == 1 { set.rotate_left(1); }
					// Decrement and maybe resort.
					else {
						set[0].1 -= 1;
						if set[0].1 < set[1].1 { self.sort(); }
					}
				}
				else if new == set[1].0 {
					// Keep the count, but shift it to the end.
					if set[1].1 == 1 { set.swap(1, 2); }
					// Decrement and maybe resort.
					else {
						set[1].1 -= 1;
						if set[1].1 < set[2].1 { set.swap(1, 2); }
					}
				}
				// Lower the count, but only as far as one.
				else if new == set[2].0 && set[2].1 != 1 { set[2].1 -= 1; },
		}

		None
	}

	/// # Add (Good) Sample.
	///
	/// If the value already exists, the count will be incremented and if the
	/// latest version is synced and the original wasn't, that will be made
	/// happy too.
	///
	/// If the value is different, it will be added, unless we're already at
	/// Strict level, in which case it will either be swapped in or ignored.
	fn add_good(&mut self, new: Sample) {
		let strict = matches!(self, Self::Strict(_));
		match self {
			Self::Maybe1((old, count)) =>
				// Bump the count.
				if new.eq(old) { *count = count.saturating_add(1); }
				// Move to Maybe2.
				else {
					*self = Self::Maybe2([(*old, *count), (new, 1)]);
				},
			Self::Maybe2([(old1, count1), (old2, count2)]) =>
				// Bump the count.
				if new.eq(old1) { *count1 = count1.saturating_add(1); }
				else if new.eq(old2) {
					*count2 = count2.saturating_add(1);
					if *count2 > *count1 { self.sort(); }
				}
				// Move to Maybe3.
				else {
					*self = Self::Maybe3([
						(*old1, *count1),
						(*old2, *count2),
						(new, 1),
					]);
				},
			Self::Maybe3([(old1, count1), (old2, count2), (old3, count3)]) |
			Self::Strict([(old1, count1), (old2, count2), (old3, count3)]) =>
				// Bump the count.
				if new.eq(old1) { *count1 = count1.saturating_add(1); }
				else if new.eq(old2) {
					*count2 = count2.saturating_add(1);
					if *count2 > *count1 { self.sort(); }
				}
				else if new.eq(old3) {
					*count3 = count3.saturating_add(1);
					if *count3 > *count2 { self.sort(); }
				}
				// Strict can't get any stricter, but we can swap the worst
				// sample if its count is one. Enough already!
				else if strict {
					if *count3 == 1 { *old3 = new; }
				}
				// Move to Strict.
				else {
					*self = Self::Strict([(*old1, 1), (*old2, 1), (*old3, 1)]);
				},
		}
	}

	/// # Reset Counts.
	///
	/// Drop all counts back to one.
	pub(crate) fn reset(&mut self) {
		match self {
			Self::Maybe1((_, count1)) => { *count1 = 1; },
			Self::Maybe2(set) => {
				set[0].1 = 1;
				set[1].1 = 1;
			},
			Self::Maybe3(set) | Self::Strict(set) => {
				set[0].1 = 1;
				set[1].1 = 1;
				set[2].1 = 1;
			},
		}
	}

	/// # Sort.
	///
	/// Multi-sample sets are ordered by popularity and age.
	fn sort(&mut self) {
		match self {
			Self::Maybe1(_) => {},
			Self::Maybe2(set) => { set.sort_unstable_by(sort_sample_count); },
			Self::Maybe3(set) |
			Self::Strict(set) => { set.sort_unstable_by(sort_sample_count); },
		}
	}
}



/// # Data Length by ID.
///
/// Return the length of the data portion of the `RipSample` corresponding to
/// this de/serialization ID.
const fn data_len_by_kind(kind: u8) -> u8 {
	match kind {
		3 | 4 => 4,
		5 => 5,
		6 => 10,
		7 | 8 => 15,
		_ => 0,
	}
}

#[expect(clippy::trivially_copy_pass_by_ref, reason = "This is a callback.")]
#[inline]
/// # Sort Sample Count Tuple.
///
/// Order by highest count.
///
/// This is an explicit method only because it has to be called for arrays of
/// different sizes.
fn sort_sample_count(a: &(Sample, u8), b: &(Sample, u8)) -> Ordering { b.1.cmp(&a.1) }

/// # Pack Two `u4` into `u8`.
///
/// Pack two (small) integers into a single byte.
///
/// This is used for our sample type de/serialization, which codes the variant
/// identifiers as a number between `1..=8`.
const fn u4_pack(a: u8, b: u8) -> u8 { a | b << 4 }

/// # Unpack `u8` into Two `u4`.
const fn u4_unpack(c: u8) -> (u8, u8) { (u4_unpack_lhs(c), u4_unpack_rhs(c)) }

/// # Unpack First `u4` in `u8`.
const fn u4_unpack_lhs(c: u8) -> u8 { c & 0b0000_1111 }

/// # Unpack Second `u4` in `u8`.
const fn u4_unpack_rhs(c: u8) -> u8 { (c >> 4) & 0b0000_1111 }



#[cfg(test)]
mod test {
	use super::*;

	/// # Random Sample.
	fn random_sample() -> Sample {
		let a = fastrand::u8(..);
		let b = fastrand::u8(..);
		let c = fastrand::u8(..);
		let d = fastrand::u8(..);
		[a, b, c, d]
	}

	#[test]
	fn t_pack() {
		// Test all combinations of left/right variant pairs we expect to see.
		for i in DATA_KIND_RNG {
			for j in DATA_KIND_RNG {
				let c = u4_pack(i, j);
				let (a, b) = u4_unpack(c);
				assert_eq!(i, a, "Left side unpacking failed.");
				assert_eq!(i, u4_unpack_lhs(c), "Left side unpacking failed.");
				assert_eq!(j, b, "Right side unpacking failed.");
				assert_eq!(j, u4_unpack_rhs(c), "Right side unpacking failed.");
			}
		}
	}

	#[test]
	fn t_ripsector() {
		// Build up four randomish sectors worth of data using all the
		// different sample types.
		let mut data = Vec::with_capacity(usize::from(SAMPLES_PER_SECTOR * 4));
		for _ in 0..SAMPLES_PER_SECTOR.wrapping_div(3) {
			// Eight doesn't divide evenly into 588, so let's use the zero-size
			// entries for padding.
			data.push(RipSample::Lead);
			data.push(RipSample::Lead);
			data.push(RipSample::Lead);
			data.push(RipSample::Tbd);
			data.push(RipSample::Tbd);
			data.push(RipSample::Tbd);

			data.push(RipSample::Bad(random_sample()));
			data.push(RipSample::Maybe(ContentiousSample::Maybe1((random_sample(), 1))));
			data.push(RipSample::Maybe(ContentiousSample::Maybe1((random_sample(), fastrand::u8(1..)))));


			let mut set = [
				(random_sample(), fastrand::u8(1..)),
				(random_sample(), fastrand::u8(1..)),
			];
			set.sort_unstable_by(sort_sample_count);
			data.push(RipSample::Maybe(ContentiousSample::Maybe2(set)));

			let mut set = [
				(random_sample(), fastrand::u8(1..)),
				(random_sample(), fastrand::u8(1..)),
				(random_sample(), fastrand::u8(1..)),
			];
			set.sort_unstable_by(sort_sample_count);
			data.push(RipSample::Maybe(ContentiousSample::Maybe3(set)));

			let mut set = [
				(random_sample(), fastrand::u8(1..)),
				(random_sample(), fastrand::u8(1..)),
				(random_sample(), fastrand::u8(1..)),
			];
			set.sort_unstable_by(sort_sample_count);
			data.push(RipSample::Maybe(ContentiousSample::Strict(set)));
		}
		assert_eq!(data.len(), usize::from(SAMPLES_PER_SECTOR * 4), "You fucked up the sector length.");

		// Test the sector de/serialization.
		let mut sector = RipSector::new();
		for v in data.chunks_exact_mut(usize::from(SAMPLES_PER_SECTOR)) {
			// Make sure there isn't any bias in our population ordering.
			fastrand::shuffle(v);

			// Serialize it.
			let mut buf = Vec::new();
			sector.serialize_into(v, &mut buf).expect("Serialization failed.");

			// Deserialize it.
			let de = sector.deserialize_from(&mut buf.as_slice())
				.expect("Deserialization failed.")
				.collect::<Vec<_>>();

			// The output should have us back where we started.
			assert_eq!(v, de, "Deserialized samples do not match the original.");
		}
	}

	#[test]
	fn t_update() {
		// Start with TBD.
		let mut sample = RipSample::Tbd;
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(sample, RipSample::Bad(NULL_SAMPLE));

		// Bad + Good = Maybe.
		sample.update(NULL_SAMPLE, false, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1)))
		);

		// Maybe + Bad = no change.
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1)))
		);

		// Maybe + Good = ++
		sample.update(NULL_SAMPLE, false, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 2)))
		);

		// Maybe + Good (different) = Contentious
		sample.update([1, 1, 1, 1], false, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 1),
			]))
		);

		// Contentious + Bad (different) = no change
		sample.update([1, 2, 1, 2], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 1),
			]))
		);

		// Bump the second.
		sample.update([1, 1, 1, 1], false, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 2),
			]))
		);

		// Second takes the lead!
		sample.update([1, 1, 1, 1], false, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3),
				(NULL_SAMPLE, 2),
			]))
		);

		// Contentious + Bad (existing) = --
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3),
				(NULL_SAMPLE, 1),
			]))
		);

		// Contentious + Bad (existing) = -- = Maybe
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 3),
			))
		);

		// Maybe + Bad (existing) = -- = empty = Bad.
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 2),
			))
		);
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 1),
			))
		);
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(sample, RipSample::Bad([1, 1, 1, 1]));
	}
}
