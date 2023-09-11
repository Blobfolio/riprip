/*!
# Rip Rip Hooray: Quality Counts
*/

use crate::RipSample;
use dactyl::{
	NiceU64,
	traits::SaturatingFrom,
};
use super::{
	COLOR_BAD,
	COLOR_CONFIRMED,
	COLOR_LIKELY,
	COLOR_MAYBE,
};



/// # Quality Bar.
const QUALITY_BAR: &str = "########################################################################";



#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// # Rip Sample Quality.
///
/// This holds the counts-by-status for the samples in a track, mostly to
/// separate out a lot of simple but verbose-looking code from the modules that
/// do _important_ things.
pub(super) struct TrackQuality {
	bad: usize,
	maybe: usize,
	likely: usize,
	confirmed: usize,
}

impl TrackQuality {
	/// # New.
	pub(super) fn new(src: &[RipSample], cutoff: u8) -> Self {
		// This should never happen.
		if src.is_empty() {
			return Self {
				bad: 1,
				maybe: 0,
				likely: 0,
				confirmed: 0,
			};
		}

		let mut bad = 0;
		let mut maybe = 0;
		let mut likely = 0;
		let mut confirmed = 0;

		for v in src {
			match v {
				RipSample::Tbd | RipSample::Bad(_) => { bad += 1; },
				RipSample::Confirmed(_) => { confirmed += 1; },
				RipSample::Maybe(_) =>
					if v.is_likely(cutoff) { likely += 1; }
					else { maybe += 1; },
			}
		}

		Self { bad, maybe, likely, confirmed }
	}
}

impl TrackQuality {
	/// # Bad.
	pub(super) const fn bad(&self) -> usize { self.bad }

	/// # Maybe.
	pub(super) const fn maybe(&self) -> usize { self.maybe }

	/// # Likely.
	pub(super) const fn likely(&self) -> usize { self.likely }

	/// # Confirmed.
	pub(super) const fn confirmed(&self) -> usize { self.confirmed }

	/// # Is Bad?
	pub(super) const fn is_bad(&self) -> bool {
		self.bad() == self.total()
	}

	/// # Is Confirmed?
	pub(super) const fn is_confirmed(&self) -> bool {
		self.confirmed() == self.total()
	}

	/// # Percent Maybe.
	pub(super) fn percent_maybe(&self) -> f64 {
		dactyl::int_div_float(
			(self.maybe + self.likely + self.confirmed) * 100,
			self.total()
		).unwrap_or(0.0)
	}

	/// # Percent Likely.
	pub(super) fn percent_likely(&self) -> f64 {
		dactyl::int_div_float(
			(self.likely + self.confirmed) * 100,
			self.total()
		).unwrap_or(0.0)
	}

	/// # Total.
	pub(super) const fn total(&self) -> usize {
		self.bad + self.maybe + self.likely + self.confirmed
	}
}

impl TrackQuality {
	/// # As Array.
	///
	/// Return all the values ordered worst to best in an array. (This is
	/// ofte ncomputationally easier to work with than individual variables.)
	pub(super) const fn as_array(&self) -> [usize; 4] {
		[self.bad, self.maybe, self.likely, self.confirmed]
	}

	#[allow(clippy::cast_precision_loss)]
	/// # Colored Bar.
	///
	/// Return a pretty bar representing the different states in relative
	/// proportion.
	pub(super) fn bar(&self) -> String {
		let q_total = self.total();
		let b_len = QUALITY_BAR.len() as f64;
		let mut b_total = 0;
		let mut b_max = 0;

		// Divvy up the portions of the bar between the counts, ensuring each
		// non-zero value has at least one block.
		let mut b_chunks = self.as_array().map(|v|
			if v == 0 { 0 }
			else {
				let tmp = usize::max(
					1,
					usize::saturating_from(
						(dactyl::int_div_float(v, q_total).unwrap_or(0.0) * b_len)
							.floor()
					)
				);
				b_total += tmp;
				if tmp > b_max { b_max = tmp; }
				tmp
			}
		);

		// If we're over or under the intended length, adjust the largest value
		// accordingly.
		if b_total != QUALITY_BAR.len() {
			let b_diff = b_total.abs_diff(QUALITY_BAR.len());
			if b_total < QUALITY_BAR.len() {
				for v in &mut b_chunks {
					if *v == b_max {
						*v += b_diff;
						break;
					}
				}
			}
			else {
				for v in &mut b_chunks {
					if *v == b_max {
						*v -= b_diff;
						break;
					}
				}
			}
		}

		format!(
			"\x1b[{COLOR_BAD}m{}\x1b[0;{COLOR_MAYBE}m{}\x1b[0;{COLOR_LIKELY}m{}\x1b[0;{COLOR_CONFIRMED}m{}\x1b[0m",
			&QUALITY_BAR[..b_chunks[0]],
			&QUALITY_BAR[..b_chunks[1]],
			&QUALITY_BAR[..b_chunks[2]],
			&QUALITY_BAR[..b_chunks[3]],
		)
	}

	/// # Legend.
	///
	/// Generate before and after legend(s) to go along with the bar. For the
	/// first one, only differences will be included, so if there are none,
	/// it will be returned as `None`.
	pub(super) fn legend(&self, start: &Self) -> (Option<String>, String) {
		let mut start = start.as_array().map(|n| if n == 0 { None } else { Some(NiceU64::from(n)) });
		let end = self.as_array().map(|n| if n == 0 { None } else { Some(NiceU64::from(n)) });

		// Clear the samey values.
		for (a, b) in start.iter_mut().zip(end.iter()) {
			if b.eq(a) { *a = None; }
		}

		// Do we have any start values?
		let start_any = start.iter().any(Option::is_some);

		// Hold the final, used values, which will probably be less than four.
		let mut list1 = Vec::new();
		let mut list2 = Vec::new();

		// Compare the start and end, if any, and pad entries in both lists
		// to the maximum length when either is worth printing.
		for ((a, b), color) in start.into_iter().zip(end).zip([COLOR_BAD, COLOR_MAYBE, COLOR_LIKELY, COLOR_CONFIRMED]) {
			let len = usize::max(
				a.as_ref().map_or(0, |v| v.len()),
				b.as_ref().map_or(0, |v| v.len()),
			);
			if len != 0 {
				// Only include starts if there's at least one.
				if start_any {
					// The strikethrough adds some complication for the empty
					// case…
					if let Some(v) = a {
						list1.push(format!("\x1b[2;9;{color}m{v:>len$}\x1b[0m"));
					}
					else {
						list1.push(format!(
							"{:>len$}\x1b[2;9;{color}m0\x1b[0m",
							"",
							len=len - 1,
						));
					}
				}
				list2.push(format!(
					"\x1b[{color}m{:>len$}\x1b[0m",
					b.as_ref().map_or("0", |v| v.as_str()),
				));
			}
		}

		// Done!
		(
			if list1.is_empty() { None } else { Some(list1.join("   ")) },
			list2.join("\x1b[2m + \x1b[0m"),
		)
	}
}
