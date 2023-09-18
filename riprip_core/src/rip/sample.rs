/*!
# Rip Rip Hooray: Samples
*/

use crate::{
	NULL_SAMPLE,
	Sample,
};
use serde::{
	Deserialize,
	Serialize,
};
use std::cmp::Ordering;



#[derive(Debug, Clone, Default, Deserialize, Eq, Hash, PartialEq, Serialize)]
/// # Rip Sample.
///
/// This enum combines sample value(s) and their status.
pub(crate) enum RipSample {
	#[default]
	/// # Unread samples.
	Tbd,

	/// Samples that came down with C2 or read errors.
	Bad(Sample),

	/// Allegedly good sample(s).
	Maybe(ContentiousSample),

	/// Samples in the leadin/leadout — that we can't access and thus have to
	/// assume are null — or ones that have been independently verified by
	/// AccurateRip and/or CUETools.
	Confirmed(Sample),
}

impl RipSample {
	/// # As Array.
	///
	/// Return the most appropriate single sample 4-byte value as an array.
	pub(crate) const fn as_array(&self) -> Sample {
		match self {
			Self::Tbd => NULL_SAMPLE,
			Self::Bad(s) | Self::Confirmed(s) => *s,
			Self::Maybe(s) => s.as_array(),
		}
	}

	/// # As Slice.
	///
	/// Return the most appropriate single sample 4-byte value as a slice.
	pub(crate) const fn as_slice(&self) -> &[u8] {
		match self {
			Self::Tbd => NULL_SAMPLE.as_slice(),
			Self::Bad(s) | Self::Confirmed(s) => s.as_slice(),
			Self::Maybe(s) => s.as_slice(),
		}
	}
}

impl RipSample {
	/// # Is Bad?
	pub(crate) const fn is_bad(&self) -> bool { matches!(self, Self::Tbd | Self::Bad(_)) }

	/*
	/// # Is Maybe?
	pub(crate) const fn is_maybe(&self) -> bool { matches!(self, Self::Contentious(_)) }
	*/

	/// # Is Contentious?
	///
	/// Only applies to a maybe with more than one value.
	pub(crate) const fn is_contentious(&self) -> bool {
		matches!(
			self,
			Self::Maybe(
				ContentiousSample::Maybe2(_) |
				ContentiousSample::Maybe3(_) |
				ContentiousSample::Maybe4(_)
			)
		)
	}

	/// # Is Confirmed?
	pub(crate) const fn is_confirmed(&self) -> bool { matches!(self, Self::Confirmed(_)) }

	/// # Is Likely?
	///
	/// A "maybe" is "likely" if it has been returned at least `cutoff` times
	/// and twice as much as any other competing value.
	///
	/// If this is called on `RipSample::Confirmed`, it will also return `true`.
	pub(crate) fn is_likely(&self, rereads: (u8, u8)) -> bool {
		match self {
			Self::Tbd | Self::Bad(_) => false,
			Self::Confirmed(_) => true,
			Self::Maybe(s) => {
				let (a, b) = s.contention();
				rereads.0 <= a && b.saturating_mul(rereads.1).min(254) < a
			},
		}
	}

	/// # Is Wishywashy.
	///
	/// Returns true if the data is so inconsistent as to exceed the maximum
	/// contention slots.
	pub(crate) const fn is_wishywashy(&self) -> bool {
		matches!(self, Self::Maybe(ContentiousSample::Maybe4((_, other))) if *other != 0)
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
	/// Confirmed stay the same.
	pub(crate) fn update(&mut self, new: Sample, err_c2: bool) {
		// Send bad samples to a different method to halve the onslaught of
		// conditional handling. Haha.
		if err_c2 { return self.update_bad(new); }

		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => {
				*self = Self::Maybe(ContentiousSample::new(new));
			},

			// Simple Maybes.
			Self::Maybe(s) => { s.add_good(new); },

			// Leave confirmed samples alone.
			Self::Confirmed(_) => {},
		}
	}

	/// # Update New Bad Sample.
	///
	/// TBD and Bad samples are simply replaced.
	///
	/// Maybe are decremented/downgraded if the value matches and there is no
	/// sync weirdness.
	///
	/// Confirmed stay the same.
	fn update_bad(&mut self, new: Sample) {
		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => { *self = Self::Bad(new); },

			// Simple Maybes.
			Self::Maybe(s) => if let Some(boo) = s.add_bad(new) {
				*self = boo;
			},

			// Leave confirmed samples alone.
			Self::Confirmed(_) => {},
		}
	}
}



#[derive(Debug, Clone, Deserialize, Eq, Hash, PartialEq, Serialize)]
/// # Contentious Rip Sample.
///
/// This structure holds up to 4 samples sorted by popularity and age. This
/// is used instead of a straight vector because there shouldn't be _that_
/// many contradictory samples, and since the counts are fixed, we don't have
/// to worry about silly bounds checks or non-const indexing constraints.
///
/// This is also two bytes smaller than a `Vec<(Sample, u8)>`, handy at the
/// scale of a CD track with hundreds of thousands of samples.
pub(crate) enum ContentiousSample {
	Maybe1((Sample, u8)),
	Maybe2([(Sample, u8); 2]),
	Maybe3([(Sample, u8); 3]),
	Maybe4(([(Sample, u8); 4], u8)),
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
			Self::Maybe3(set) => set[0].0,
			Self::Maybe4((set, _)) => set[0].0,
		}
	}

	/// # As Slice.
	const fn as_slice(&self) -> &[u8] {
		match self {
			Self::Maybe1((s, _)) => s.as_slice(),
			Self::Maybe2(set) => set[0].0.as_slice(),
			Self::Maybe3(set) => set[0].0.as_slice(),
			Self::Maybe4((set, _)) => set[0].0.as_slice(),
		}
	}

	/// # Contention.
	///
	/// Return the first (best) total, and the total of all the rest.
	const fn contention(&self) -> (u8, u8) {
		match self {
			Self::Maybe1((_, c)) => (*c, 0),
			Self::Maybe2(set) => (set[0].1, set[1].1),
			Self::Maybe3(set) => (
				set[0].1,
				set[1].1.saturating_add(set[2].1),
			),
			Self::Maybe4((set, other)) => (
				set[0].1,
				other.saturating_add(set[1].1)
					.saturating_add(set[2].1)
					.saturating_add(set[3].1),
			),
		}
	}
}

impl ContentiousSample {
	#[allow(clippy::cognitive_complexity)]
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
					// Drop, keeping 2,3.
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
					// Drop, keeping 1,3.
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
					// Drop, keeping 1,2.
					if *count3 == 1 {
						*self = Self::Maybe2([(*old1, *count1), (*old2, *count2)]);
					}
					// Decrement.
					else { *count3 -= 1; }
				},
			Self::Maybe4(([(old1, count1), (old2, count2), (old3, count3), (old4, count4)], _)) =>
				if new.eq(old1) {
					// Drop, keeping 2,3,4.
					if *count1 == 1 {
						*self = Self::Maybe3([
							(*old2, *count2), (*old3, *count3), (*old4, *count4),
						]);
					}
					// Decrement and maybe resort.
					else {
						*count1 -= 1;
						if *count1 < *count2 { self.sort(); }
					}
				}
				else if new.eq(old2) {
					// Drop, keeping 1,3,4.
					if *count2 == 1 {
						*self = Self::Maybe3([
							(*old1, *count1), (*old3, *count3), (*old4, *count4),
						]);
					}
					// Decrement and maybe resort.
					else {
						*count2 -= 1;
						if *count2 < *count3 { self.sort(); }
					}
				}
				else if new.eq(old3) {
					// Drop, keeping 1,2,4.
					if *count3 == 1 {
						*self = Self::Maybe3([
							(*old1, *count1), (*old2, *count2), (*old4, *count4),
						]);
					}
					// Decrement and maybe resort.
					else {
						*count3 -= 1;
						if *count3 < *count4 { self.sort(); }
					}
				}
				else if new.eq(old4) {
					// Drop, keeping 1,2,3.
					if *count4 == 1 {
						*self = Self::Maybe3([
							(*old1, *count1), (*old2, *count2), (*old3, *count3),
						]);
					}
					// Decrement.
					else { *count4 -= 1; }
				},
		}

		None
	}

	/// # Add (Good) Sample.
	///
	/// If the value already exists, the count will be incremented and if the
	/// latest version is synced and the original wasn't, that will be made
	/// happy too.
	///
	/// If the value is different, it will be added, unless we're already a
	/// Maybe4, in which case we'll just bump the "other" count.
	fn add_good(&mut self, new: Sample) {
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
			Self::Maybe3([(old1, count1), (old2, count2), (old3, count3)]) =>
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
				// Move to Maybe4.
				else {
					*self = Self::Maybe4((
						[
							(*old1, *count1),
							(*old2, *count2),
							(*old3, *count3),
							(new, 1),
						],
						0
					));
				},
			Self::Maybe4(([(old1, count1), (old2, count2), (old3, count3), (old4, count4)], other)) =>
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
				else if new.eq(old4) {
					*count4 = count4.saturating_add(1);
					if *count4 > *count3 { self.sort(); }
				}
				// Increment other.
				else { *other = other.saturating_add(1); },
		}
	}

	/// # Reset Counts.
	///
	/// Drop all counts back to one.
	pub(crate) fn reset_counts(&mut self) {
		match self {
			Self::Maybe1((_, count1)) => { *count1 = 1; },
			Self::Maybe2(set) => {
				set[0].1 = 1;
				set[1].1 = 1;
			},
			Self::Maybe3(set) => {
				set[0].1 = 1;
				set[1].1 = 1;
				set[2].1 = 1;
			},
			Self::Maybe4((set, other)) => {
				set[0].1 = 1;
				set[1].1 = 1;
				set[2].1 = 1;
				set[3].1 = 1;

				*other = 0;
			},
		}
	}

	/// # Sort.
	fn sort(&mut self) {
		match self {
			Self::Maybe1(_) => {},
			Self::Maybe2(set) => { set.sort_unstable_by(sort_sample_count); },
			Self::Maybe3(set) => { set.sort_unstable_by(sort_sample_count); },
			Self::Maybe4((set, _)) => { set.sort_unstable_by(sort_sample_count); },
		}
	}
}



#[allow(clippy::trivially_copy_pass_by_ref)] // This is a callback.
#[inline]
/// # Sort Sample Count Tuple.
///
/// Order by highest count.
fn sort_sample_count(a: &(Sample, u8), b: &(Sample, u8)) -> Ordering { b.1.cmp(&a.1) }



#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_update() {
		// Start with TBD.
		let mut sample = RipSample::Tbd;
		sample.update(NULL_SAMPLE, true);
		assert_eq!(sample, RipSample::Bad(NULL_SAMPLE));

		// Bad + Good = Maybe.
		sample.update(NULL_SAMPLE, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1)))
		);

		// Maybe + Bad = no change.
		sample.update([1, 1, 1, 1], true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1)))
		);

		// Maybe + Good = ++
		sample.update(NULL_SAMPLE, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 2)))
		);

		// Maybe + Good (different) = Contentious
		sample.update([1, 1, 1, 1], false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 1),
			]))
		);

		// Contentious + Bad (different) = no change
		sample.update([1, 2, 1, 2], true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 1),
			]))
		);

		// Bump the second.
		sample.update([1, 1, 1, 1], false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2),
				([1, 1, 1, 1], 2),
			]))
		);

		// Second takes the lead!
		sample.update([1, 1, 1, 1], false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3),
				(NULL_SAMPLE, 2),
			]))
		);

		// Contentious + Bad (existing) = --
		sample.update(NULL_SAMPLE, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3),
				(NULL_SAMPLE, 1),
			]))
		);

		// Contentious + Bad (existing) = -- = Maybe
		sample.update(NULL_SAMPLE, true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 3),
			))
		);

		// Maybe + Bad (existing) = -- = empty = Bad.
		sample.update([1, 1, 1, 1], true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 2),
			))
		);
		sample.update([1, 1, 1, 1], true);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 1),
			))
		);
		sample.update([1, 1, 1, 1], true);
		assert_eq!(sample, RipSample::Bad([1, 1, 1, 1]));
	}
}
