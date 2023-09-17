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



/// # Sync Flag.
const SYNCHED: u8 = 0b1000_0000;



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
	/*
	/// # Is Bad?
	pub(crate) const fn is_bad(&self) -> bool { matches!(self, Self::Bad(_)) }

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
			Self::Maybe(s) =>
				s.is_synched() &&
				{
					let (a, b) = s.contention();
					rereads.0 <= a && b.saturating_mul(rereads.1).min(126) < a
				},
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
	/// Confirmed stay the same.
	pub(crate) fn update(&mut self, new: Sample, err_c2: bool, err_sync: bool) {
		// Send bad samples to a different method to halve the onslaught of
		// conditional handling. Haha.
		if err_c2 {
			return self.update_bad(new, err_sync);
		}

		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => {
				*self = Self::Maybe(ContentiousSample::new(new, err_sync));
			},

			// Simple Maybes.
			Self::Maybe(s) => { s.add_good(new, err_sync); },

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
	fn update_bad(&mut self, new: Sample, err_sync: bool) {
		match self {
			// Always update a TBD.
			Self::Tbd | Self::Bad(_) => {
				*self = Self::Bad(new);
			},

			// Simple Maybes.
			Self::Maybe(s) =>
				if ! err_sync {
					if let Some(boo) = s.add_bad(new) {
						*self = boo;
					}
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
	/// # New.
	const fn new(new: Sample, err_sync: bool) -> Self {
		let count = join_count_sync(1, ! err_sync);
		Self::Maybe1((new, count))
	}
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
			Self::Maybe1((_, c)) => {
				(*c & ! SYNCHED, 0)
			},
			Self::Maybe2(set) => {
				(
					set[0].1 & ! SYNCHED,
					set[1].1 & ! SYNCHED,
				)
			},
			Self::Maybe3(set) => {
				(
					set[0].1 & ! SYNCHED,
					(set[1].1 & ! SYNCHED).saturating_add(set[2].1 & ! SYNCHED),
				)
			},
			Self::Maybe4((set, other)) => {
				let other = other.saturating_add(set[1].1 & ! SYNCHED)
					.saturating_add(set[2].1 & ! SYNCHED)
					.saturating_add(set[3].1 & ! SYNCHED);
				(
					set[0].1 & ! SYNCHED,
					other,
				)
			},
		}
	}

	/// # Is Synched?
	const fn is_synched(&self) -> bool {
		let raw = match self {
			Self::Maybe1((_, c)) => *c,
			Self::Maybe2(set) => set[0].1,
			Self::Maybe3(set) => set[0].1,
			Self::Maybe4((set, _)) => set[0].1,
		};
		SYNCHED == raw & SYNCHED
	}
}

impl ContentiousSample {
	#[allow(clippy::too_many_lines)] // That's why we're stopping at four. Haha.
	/// # Add (Bad) Sample.
	///
	/// If the value already exists, the count is _decreased_. If it reaches
	/// zero, it is removed, potentially downgrading the maybe number.
	///
	/// Returns a bad sample if the value can no longer be held (e.g. a `Maybe1`
	/// with a count of zero).
	fn add_bad(&mut self, new: Sample) -> Option<RipSample> {
		match self {
			Self::Maybe1((old, data)) =>
				if new.eq(old) {
					let (count, sync) = split_count_sync(*data);
					// We can't drop the only entry; go bad!
					if count == 1 { return Some(RipSample::Bad(new)); }
					*data = join_count_sync(count - 1, sync);
				},
			Self::Maybe2([(old1, data1), (old2, data2)]) =>
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					// Drop, keeping 2.
					if count == 1 {
						*self = Self::Maybe1((*old2, *data2));
					}
					// Decrement and resort.
					else {
						*data1 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					// Drop, keeping 1.
					if count == 1 {
						*self = Self::Maybe1((*old1, *data1));
					}
					// Decrement.
					else { *data2 = join_count_sync(count - 1, sync); }
				},
			Self::Maybe3([(old1, data1), (old2, data2), (old3, data3)]) =>
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					// Drop, keeping 2,3.
					if count == 1 {
						*self = Self::Maybe2([(*old2, *data2), (*old3, *data3)]);
					}
					// Decrement and resort.
					else {
						*data1 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					// Drop, keeping 1,3.
					if count == 1 {
						*self = Self::Maybe2([(*old1, *data1), (*old3, *data3)]);
					}
					// Decrement and resort.
					else {
						*data2 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old3) {
					let (count, sync) = split_count_sync(*data3);
					// Drop, keeping 1,2.
					if count == 1 {
						*self = Self::Maybe2([(*old1, *data1), (*old2, *data2)]);
					}
					// Decrement.
					else { *data3 = join_count_sync(count - 1, sync); }
				},
			Self::Maybe4(([(old1, data1), (old2, data2), (old3, data3), (old4, data4)], _)) =>
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					// Drop, keeping 2,3,4.
					if count == 1 {
						*self = Self::Maybe3([
							(*old2, *data2), (*old3, *data3), (*old4, *data4),
						]);
					}
					// Decrement and resort.
					else {
						*data1 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					// Drop, keeping 1,3,4.
					if count == 1 {
						*self = Self::Maybe3([
							(*old1, *data1), (*old3, *data3), (*old4, *data4),
						]);
					}
					// Decrement and resort.
					else {
						*data2 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old3) {
					let (count, sync) = split_count_sync(*data3);
					// Drop, keeping 1,2,4.
					if count == 1 {
						*self = Self::Maybe3([
							(*old1, *data1), (*old2, *data2), (*old4, *data4),
						]);
					}
					// Decrement and resort.
					else {
						*data3 = join_count_sync(count - 1, sync);
						self.sort();
					}
				}
				else if new.eq(old4) {
					let (count, sync) = split_count_sync(*data4);
					// Drop, keeping 1,2,3.
					if count == 1 {
						*self = Self::Maybe3([
							(*old1, *data1), (*old2, *data2), (*old3, *data3),
						]);
					}
					// Decrement.
					else { *data4 = join_count_sync(count - 1, sync); }
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
	fn add_good(&mut self, new: Sample, err_sync: bool) {
		match self {
			Self::Maybe1((old, data)) =>
				// Bump the count.
				if new.eq(old) {
					let (count, sync) = split_count_sync(*data);
					*data = join_count_sync(count + 1, sync || ! err_sync);
				}
				// Move to Maybe2.
				else {
					*self = Self::Maybe2([
						(*old, *data),
						(new, join_count_sync(1, ! err_sync)),
					]);
				},
			Self::Maybe2([(old1, data1), (old2, data2)]) =>
				// Bump the count.
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					*data1 = join_count_sync(count + 1, sync || ! err_sync);
				}
				// Bump and maybe resort.
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					*data2 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Move to Maybe3.
				else {
					*self = Self::Maybe3([
						(*old1, *data1),
						(*old2, *data2),
						(new, join_count_sync(1, ! err_sync)),
					]);
				},
			Self::Maybe3([(old1, data1), (old2, data2), (old3, data3)]) =>
				// Bump the count.
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					*data1 = join_count_sync(count + 1, sync || ! err_sync);
				}
				// Bump and maybe resort.
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					*data2 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Bump and maybe resort.
				else if new.eq(old3) {
					let (count, sync) = split_count_sync(*data3);
					*data3 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Move to Maybe4.
				else {
					*self = Self::Maybe4((
						[
							(*old1, *data1),
							(*old2, *data2),
							(*old3, *data3),
							(new, join_count_sync(1, ! err_sync)),
						],
						0
					));
				},
			Self::Maybe4(([(old1, data1), (old2, data2), (old3, data3), (old4, data4)], other)) =>
				// Bump the count.
				if new.eq(old1) {
					let (count, sync) = split_count_sync(*data1);
					*data1 = join_count_sync(count + 1, sync || ! err_sync);
				}
				// Bump and maybe resort.
				else if new.eq(old2) {
					let (count, sync) = split_count_sync(*data2);
					*data2 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Bump and maybe resort.
				else if new.eq(old3) {
					let (count, sync) = split_count_sync(*data3);
					*data3 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Bump and maybe resort.
				else if new.eq(old4) {
					let (count, sync) = split_count_sync(*data4);
					*data4 = join_count_sync(count + 1, sync || ! err_sync);
					self.sort();
				}
				// Increment other, unless there was a sync issue.
				else if ! err_sync {
					*other = other.saturating_add(1);
				},
		}
	}

	/// # Reset Counts.
	///
	/// Drop all counts back to one.
	pub(crate) fn reset_counts(&mut self) {
		match self {
			Self::Maybe1((_, data1)) => {
				let sync = SYNCHED == *data1 & SYNCHED;
				*data1 = join_count_sync(1, sync);
			},
			Self::Maybe2(set) => {
				let sync = SYNCHED == set[0].1 & SYNCHED;
				set[0].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[1].1 & SYNCHED;
				set[1].1 = join_count_sync(1, sync);
			},
			Self::Maybe3(set) => {
				let sync = SYNCHED == set[0].1 & SYNCHED;
				set[0].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[1].1 & SYNCHED;
				set[1].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[2].1 & SYNCHED;
				set[2].1 = join_count_sync(1, sync);
			},
			Self::Maybe4((set, other)) => {
				let sync = SYNCHED == set[0].1 & SYNCHED;
				set[0].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[1].1 & SYNCHED;
				set[1].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[2].1 & SYNCHED;
				set[2].1 = join_count_sync(1, sync);

				let sync = SYNCHED == set[3].1 & SYNCHED;
				set[3].1 = join_count_sync(1, sync);

				*other = 0;
			},
		}
	}

	/// # Sort.
	fn sort(&mut self) {
		match self {
			Self::Maybe1(_) => {},
			Self::Maybe2(set) => { set.sort_unstable_by(sort_sample_count_sync); },
			Self::Maybe3(set) => { set.sort_unstable_by(sort_sample_count_sync); },
			Self::Maybe4((set, _)) => { set.sort_unstable_by(sort_sample_count_sync); },
		}
	}
}



/// # Merge Count/Sync.
///
/// We group our counts and sync together to save space. This method merges the
/// separate values into one for storage.
const fn join_count_sync(mut count: u8, sync: bool) -> u8 {
	// We can't store counts higher than this.
	if 127 < count { count = 127; }

	if sync { count | SYNCHED }
	else { count }
}

#[allow(clippy::trivially_copy_pass_by_ref)] // This is a callback.
/// # Sort Sample Count Tuple.
///
/// Order by highest count, then sync.
fn sort_sample_count_sync(a: &(Sample, u8), b: &(Sample, u8)) -> Ordering {
	let (a_count, a_sync) = split_count_sync(a.1);
	let (b_count, b_sync) = split_count_sync(b.1);
	match b_count.cmp(&a_count) {
		Ordering::Equal => b_sync.cmp(&a_sync),
		cmp => cmp,
	}
}

/// # Split Count/Sync.
///
/// We group our counts and sync together to save space. This method splits and
/// returns the separate values.
const fn split_count_sync(raw: u8) -> (u8, bool) {
	let sync = SYNCHED == raw & SYNCHED;
	let count = raw & ! SYNCHED;
	(count, sync)
}




#[cfg(test)]
mod test {
	use super::*;

	#[test]
	fn t_join_split() {
		for i in 0..=127_u8 {
			for j in [true, false] {
				let joined = join_count_sync(i, j);
				assert_eq!(
					split_count_sync(joined),
					(i, j),
					"Join/split wrong with {i} {j}.",
				);
			}
		}
	}

	#[test]
	fn t_update() {
		// Start with TBD.
		let mut sample = RipSample::Tbd;
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(sample, RipSample::Bad(NULL_SAMPLE));

		// Bad + Good = Maybe.
		sample.update(NULL_SAMPLE, false, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1 | SYNCHED)))
		);

		// Maybe + Bad = no change.
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 1 | SYNCHED)))
		);

		// Maybe + Good = ++
		sample.update(NULL_SAMPLE, false, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 2 | SYNCHED)))
		);

		// Maybe + Good (different) = Contentious
		sample.update([1, 1, 1, 1], false, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2 | SYNCHED),
				([1, 1, 1, 1], 1 | SYNCHED),
			]))
		);

		// Contentious + Bad (different) = no change
		sample.update([1, 2, 1, 2], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2 | SYNCHED),
				([1, 1, 1, 1], 1 | SYNCHED),
			]))
		);

		// Bump the second.
		sample.update([1, 1, 1, 1], false, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				(NULL_SAMPLE, 2 | SYNCHED),
				([1, 1, 1, 1], 2 | SYNCHED),
			]))
		);

		// Second takes the lead!
		sample.update([1, 1, 1, 1], false, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3 | SYNCHED),
				(NULL_SAMPLE, 2 | SYNCHED),
			]))
		);

		// Contentious + Bad (existing) = --
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe2([
				([1, 1, 1, 1], 3 | SYNCHED),
				(NULL_SAMPLE, 1 | SYNCHED),
			]))
		);

		// Contentious + Bad (existing) = -- = Maybe
		sample.update(NULL_SAMPLE, true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 3 | SYNCHED),
			))
		);

		// Maybe + Bad (existing) = -- = empty = Bad.
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 2 | SYNCHED),
			))
		);
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(
			sample,
			RipSample::Maybe(ContentiousSample::Maybe1(
				([1, 1, 1, 1], 1 | SYNCHED),
			))
		);
		sample.update([1, 1, 1, 1], true, false);
		assert_eq!(sample, RipSample::Bad([1, 1, 1, 1]));
	}
}
