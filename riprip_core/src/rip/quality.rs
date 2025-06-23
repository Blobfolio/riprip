/*!
# Rip Rip Hooray: Quality Counts
*/

use crate::RipSample;
use dactyl::{
	NiceFloat,
	NiceU32,
	traits::{
		Inflection,
		SaturatingFrom,
	},
};
use fyi_msg::fyi_ansi::{
	ansi,
	csi,
	dim,
};
use std::{
	borrow::Cow,
	fmt,
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
	/// # Bad Samples.
	bad: u32,

	/// # Allegedly Good Samples.
	maybe: u32,

	/// # Likely Good Samples.
	likely: u32,

	/// # Confirmed Good Samples.
	confirmed: u32,

	/// # Contentious Samples.
	contentious: u32,

	/// # Confused?
	///
	/// This is true when the drive returns different values from read-to-read
	/// without admitting any errors have occurred.
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

	#[expect(clippy::cast_precision_loss, reason = "False positive.")]
	/// # Colored Bar.
	///
	/// Return a pretty bar representing the different states in relative
	/// proportion.
	pub(super) fn bar(&self) -> TrackQualityBar {
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

		TrackQualityBar(b_chunks)
	}

	/// # Legend.
	///
	/// Generate before and after legend(s) to go along with the bar. For the
	/// first one, only differences will be included, so if there are none,
	/// it will be returned as `None`.
	pub(super) fn legend(&self, start: &Self) -> TrackQualityLegend {
		let start = start.as_array().map(|n| if n == 0 { None } else { Some(NiceU32::from(n)) });
		let end = self.as_array().map(|n| if n == 0 { None } else { Some(NiceU32::from(n)) });
		TrackQualityLegend { start, end }
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
						concat!("Recovery is ", dim!("({})"), " {}{}complete."),
						qualifier,
						if p.compact_str() == "100" { "" } else { p.compact_str() },
						if p.compact_str() == "100" { "" } else { "% " },
					))
				},
				(Some(p1), Some(p2)) if p2.precise_str(3) == "100.000" => Cow::Owned(format!(
					concat!("Recovery is ", dim!("(likely)"), " at least {}% complete."),
					p1.compact_str(),
				)),
				(Some(p1), Some(p2)) => Cow::Owned(format!(
					concat!("Recovery is ", dim!("(roughly)"), " {}% – {}% complete."),
					p1.precise_str(3),
					p2.precise_str(3),
				)),
			}
		}
	}
}



/// # Track Quality Bar.
pub(super) struct TrackQualityBar([usize; 4]);

impl fmt::Display for TrackQualityBar {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		write!(
			f,
			concat!(
				csi!(light_red), "{}",
				csi!(dark_orange), "{}",
				csi!(light_yellow), "{}",
				ansi!((light_green) "{}"),
			),
			&QUALITY_BAR[..self.0[0]],
			&QUALITY_BAR[..self.0[1]],
			&QUALITY_BAR[..self.0[2]],
			&QUALITY_BAR[..self.0[3]],
		)
	}
}



/// # Track Quality Legend.
///
/// This is used to format the legend for the initial and/or final rip states.
pub(super) struct TrackQualityLegend {
	/// # Original Values.
	start: [Option<NiceU32>; 4],

	/// # Final Values.
	end: [Option<NiceU32>; 4],
}

impl TrackQualityLegend {
	/// # Colors.
	const COLORS: [&str; 4] = [csi!(light_red), csi!(dark_orange), csi!(light_yellow), csi!(light_green)];

	/// # Padding.
	///
	/// Note: this is the maximum length of a NiceU32.
	const PADDING: &str = "             ";

	/// # Iterate!
	const fn iter(&self) -> TrackQualityLegendIter {
		TrackQualityLegendIter {
			start: &self.start,
			end: &self.end,
			pos: 0,
		}
	}

	/// # Start Line.
	///
	/// Return a `Display`-friendly object representing the legend for the
	/// initial state.
	///
	/// Returns `None` if there was no initial state or it is identical to the
	/// final one.
	pub(super) fn start(&self) -> Option<TrackQualityLegendStart> {
		if self.start.iter().zip(self.end.iter()).any(|(a, b)| a.is_some() && a != b) {
			Some(TrackQualityLegendStart(self))
		}
		else { None }
	}
}

impl fmt::Display for TrackQualityLegend {
	/// # Print Final Legend.
	///
	/// Print the legend for the final state.
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Entry Spacer.
		const GLUE: &str = dim!(" + ");

		let mut any = false;
		for (a, b, color) in self.iter() {
			// Column length.
			let len = usize::max(a.len(), b.len());

			if any { f.write_str(GLUE)?;}
			f.write_str(&Self::PADDING[..len - b.len()])?;
			f.write_str(color)?;
			f.write_str(b)?;
			f.write_str(csi!())?;
			any = true;
		}

		Ok(())
	}
}



/// # Track Quality Legend Iterator.
///
/// Iterate through triples containing the initial and final states, and
/// corresponding color, filtering out empty values.
struct TrackQualityLegendIter<'a> {
	/// # Initial State.
	start: &'a [Option<NiceU32>; 4],

	/// # Final State.
	end: &'a [Option<NiceU32>; 4],

	/// # Next Iter Position.
	pos: usize,
}

impl<'a> Iterator for TrackQualityLegendIter<'a> {
	type Item = (&'a str, &'a str, &'static str);

	fn next(&mut self) -> Option<Self::Item> {
		for idx in self.pos..4 {
			// Values at the current index.
			let start = self.start[idx].as_ref();
			let end = self.end[idx].as_ref();

			// Return both if either exist.
			if start.is_some() || end.is_some() {
				self.pos = idx + 1;
				return Some((
					start.map_or("0", NiceU32::as_str),
					end.map_or("0", NiceU32::as_str),
					TrackQualityLegend::COLORS[idx],
				));
			}
		}

		// We're done.
		self.pos = 4;
		None
	}

	fn size_hint(&self) -> (usize, Option<usize>) {
		let len = self.len();
		(len, Some(len))
	}
}

impl ExactSizeIterator for TrackQualityLegendIter<'_> {
	fn len(&self) -> usize {
		let mut total = 0;
		for idx in self.pos..4 {
			if self.start[idx].is_some() || self.end[idx].is_some() { total += 1; }
		}
		total
	}
}

impl std::iter::FusedIterator for TrackQualityLegendIter<'_> {}



/// # Initial Track Quality Legend.
///
/// This is used to format the legend for the initial rip state.
pub(super) struct TrackQualityLegendStart<'a>(&'a TrackQualityLegend);

impl fmt::Display for TrackQualityLegendStart<'_> {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		/// # Entry Spacer.
		const GLUE: &str = "   ";

		let mut any = false;
		for (a, b, color) in self.0.iter() {
			// Column length.
			let len = usize::max(a.len(), b.len());

			if any { f.write_str(GLUE)?; }
			f.write_str(&TrackQualityLegend::PADDING[..len - a.len()])?;
			if a == b { f.write_str(csi!(dim))?; }
			else { f.write_str(csi!(dim, strike))?; }
			f.write_str(color)?;
			f.write_str(a)?;
			f.write_str(csi!())?;
			any = true;
		}

		Ok(())
	}
}
