/*!
# Rip Rip Hooray: Quality Counts
*/

use crate::{
	COLOR_BAD,
	COLOR_CONFIRMED,
	COLOR_LIKELY,
	COLOR_MAYBE,
	RipSample,
};
use dactyl::{
	NiceFloat,
	NiceU32,
	traits::{
		Inflection,
		SaturatingFrom,
	},
};
use std::{
	borrow::Cow,
	num::NonZeroU32,
	ops::Add,
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
	bad: u32,
	maybe: u32,
	likely: u32,
	confirmed: u32,
	contentious: u32,
	confused: bool,
}

impl Add for TrackQuality {
	type Output = Self;
	fn add(self, other: Self) -> Self::Output {
		Self {
			bad: self.bad + other.bad,
			maybe: self.maybe + other.maybe,
			likely: self.likely + other.likely,
			confirmed: self.confirmed + other.confirmed,
			contentious: self.contentious + other.contentious,
			confused: self.confused || other.confused,
		}
	}
}

impl TrackQuality {
	/// # From Slice.
	///
	/// Count up all the different statuses in a given track slice.
	pub(super) fn new(src: &[RipSample], rereads: (u8, u8)) -> Self {
		// This should never happen, but will ensure there's never any
		// division-by-zero weirdness later on.
		if src.is_empty() {
			return Self {
				bad: 1,
				maybe: 0,
				likely: 0,
				confirmed: 0,
				contentious: 0,
				confused: false,
			};
		}

		let mut bad = 0;
		let mut maybe = 0;
		let mut likely = 0;
		let mut confirmed = 0;
		let mut contentious = 0;
		let mut confused = false;

		for v in src {
			match v {
				RipSample::Tbd | RipSample::Bad(_) => { bad += 1; },
				RipSample::Lead => { confirmed += 1; },
				RipSample::Maybe(_) => {
					if v.is_likely(rereads) { likely += 1; }
					else { maybe += 1; }

					if v.is_contentious() {
						contentious += 1;
						if v.is_confused() { confused = true; }
					}
				},
			}
		}

		Self { bad, maybe, likely, confirmed, contentious, confused }
	}

	/// # New Bad.
	///
	/// Mark num samples as bad.
	pub(super) const fn new_bad(num: NonZeroU32) -> Self {
		Self {
			bad: num.get(),
			maybe: 0,
			likely: 0,
			confirmed: 0,
			contentious: 0,
			confused: false,
		}
	}

	/// # New Confirmed.
	///
	/// Mark num samples as confirmed.
	pub(super) const fn new_confirmed(num: NonZeroU32) -> Self {
		Self {
			bad: 0,
			maybe: 0,
			likely: 0,
			confirmed: num.get(),
			contentious: 0,
			confused: false,
		}
	}
}

impl TrackQuality {
	/// # Bad.
	pub(super) const fn bad(&self) -> u32 { self.bad }

	/// # Maybe.
	pub(super) const fn maybe(&self) -> u32 { self.maybe }

	/// # Likely.
	pub(super) const fn likely(&self) -> u32 { self.likely }

	/// # Confirmed.
	pub(super) const fn confirmed(&self) -> u32 { self.confirmed }

	/// # Is Likely/Confirmed?
	pub(super) const fn is_likely(&self) -> bool {
		self.likely() + self.confirmed() == self.total().get()
	}

	/// # Is Confirmed?
	pub(super) const fn is_confirmed(&self) -> bool {
		self.confirmed() == self.total().get()
	}

	/// # Percent Maybe.
	pub(super) fn percent_maybe(&self) -> Option<f64> {
		let v = self.maybe + self.likely + self.confirmed;
		let total = self.total().get();
		if v == 0 { None }
		else if v == total { Some(100.0) }
		else {
			Some(
				f64::from(self.maybe + self.likely + self.confirmed)
				* 100.0
				/ f64::from(total)
			)
		}
	}

	/// # Percent Likely.
	pub(super) fn percent_likely(&self) -> Option<f64> {
		let v = self.likely + self.confirmed;
		let total = self.total().get();
		if v == 0 { None }
		else if v == total { Some(100.0) }
		else {
			Some(
				f64::from(self.likely + self.confirmed)
				* 100.0
				/ f64::from(total)
			)
		}
	}

	/// # Total.
	pub(super) const fn total(&self) -> NonZeroU32 {
		if let Some(total) = NonZeroU32::new(self.bad + self.maybe + self.likely + self.confirmed) {
			total
		}
		// This should never happen.
		else { NonZeroU32::MIN }
	}
}

impl TrackQuality {
	/// # As Array.
	///
	/// Return all the values ordered worst to best in an array. (This is
	/// ofte ncomputationally easier to work with than individual variables.)
	pub(super) const fn as_array(&self) -> [u32; 4] {
		[
			self.bad,
			self.maybe,
			self.likely,
			self.confirmed,
		]
	}

	#[allow(clippy::cast_precision_loss)]
	/// # Colored Bar.
	///
	/// Return a pretty bar representing the different states in relative
	/// proportion.
	pub(super) fn bar(&self) -> String {
		let q_total = self.total().get();
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
						(f64::from(v) / f64::from(q_total) * b_len).floor()
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
		let start = start.as_array().map(|n| if n == 0 { None } else { Some(NiceU32::from(n)) });
		let end = self.as_array().map(|n| if n == 0 { None } else { Some(NiceU32::from(n)) });

		// Do we have any start values different from the end?
		let start_any = start.iter().zip(end.iter()).any(|(a, b)| a.is_some() && a != b);

		// Hold the final, used values, which will probably be less than four.
		let mut list1 = Vec::new();
		let mut list2 = Vec::new();

		// Compare the start and end, if any, and pad entries in both lists
		// to the maximum length when either is worth printing.
		for ((a, b), color) in start.into_iter().zip(end).zip([COLOR_BAD, COLOR_MAYBE, COLOR_LIKELY, COLOR_CONFIRMED]) {
			if a.is_some() || b.is_some() {
				let len = usize::max(
					a.as_ref().map_or(1, |v| v.len()),
					b.as_ref().map_or(1, |v| v.len()),
				);

				// Only include starts if there's at least one.
				if start_any {
					// If unchanged, don't cross it out.
					if a == b {
						list1.push(format!(
							"\x1b[2;{color}m{:>len$}\x1b[0m",
							a.as_ref().map_or("0", |v| v.as_str()),
						));
					}
					// Otherwise strike!
					else {
						let v = a.as_ref().map_or("0", |v| v.as_str());
						let extra = " ".repeat(len - v.len());
						list1.push(format!("{extra}\x1b[2;9;{color}m{v}\x1b[0m"));
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

	/// # Summary.
	///
	/// Summarize the state of the track rip in one line.
	pub(crate) fn summarize(&self) -> Cow<'static, str> {
		// Perfect!
		if self.is_confirmed() {
			Cow::Borrowed("All data has been accurately recovered!")
		}
		// Likely but maybe also not.
		else if self.is_likely() {
			if self.contentious == 0 {
				Cow::Borrowed("Recovery is likely complete!")
			}
			else if self.confused {
				Cow::Owned(format!(
					"Recovery is likely complete, but with {} confused/contentious {}.",
					NiceU32::from(self.contentious),
					self.contentious.inflect("sample", "samples"),
				))
			}
			else {
				Cow::Owned(format!(
					"Recovery is likely complete, but with {} contentious {}.",
					NiceU32::from(self.contentious),
					self.contentious.inflect("sample", "samples"),
				))
			}
		}
		// The progress is probably more granular.
		else {
			let low = self.percent_likely().map(NiceFloat::from).filter(|v| v.precise_str(3) != "0.000");
			let mut high = self.percent_maybe().map(NiceFloat::from).filter(|v| v.precise_str(3) != "0.000");

			// Remove the second percentage if it matches the first when
			// rounded.
			if low.zip(high).filter(|(l, h)| l.precise_str(3) == h.precise_str(3)).is_some() {
				high = None;
			}

			match (low, high) {
				(None, None) => Cow::Borrowed("The road to recovery may be a long one."),
				(Some(p), None) | (None, Some(p)) => {
					let qualifier =
						if self.maybe() < self.likely() { "likely" }
						else { "maybe" };
					Cow::Owned(format!(
						"Recovery is \x1b[2m({qualifier})\x1b[0m {}{}complete.",
						if p.compact_str() == "100" { "" } else { p.compact_str() },
						if p.compact_str() == "100" { "" } else { "% " },
					))
				},
				(Some(p1), Some(p2)) if p2.precise_str(3) == "100.000" => Cow::Owned(format!(
					"Recovery is \x1b[2m(likely)\x1b[0m at least {}% complete.",
					p1.compact_str(),
				)),
				(Some(p1), Some(p2)) => Cow::Owned(format!(
					"Recovery is \x1b[2m(roughly)\x1b[0m {}% – {}%s complete.",
					p1.precise_str(3),
					p2.precise_str(3),
				)),
			}
		}
	}
}
