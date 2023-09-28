/*!
# Rip Rip Hooray: De/Serialization
*/

use cdtoc::Toc;
use crate::{
	NULL_SAMPLE,
	RipSample,
	Sample,
};
use super::{
	RipState,
	sample::ContentiousSample,
};
use std::io::{
	Read,
	Write,
};



/// # Magic Bytes.
///
/// This is used to identify `RipState` files, as well as the format "version"
/// used at the time of their construction, making sure we don't waste time
/// trying to shove bytes into the wrong format.
const MAGIC: [u8; 8] = *b"RRip0001";

/// # Size of Sample.
const SIZE_SAMPLE: usize = std::mem::size_of::<Sample>();

/// # Size of Sample + Count.
const SIZE_SAMPLE_COUNT: usize = SIZE_SAMPLE + SIZE_U8;

/// # Size Of u8.
const SIZE_U8: usize = std::mem::size_of::<u8>();

/// # Size Of u32.
const SIZE_U32: usize = std::mem::size_of::<u32>();

/// # Sample + Count Pair.
type SampleCount = (Sample, u8);



/// # Read/Write Binary Serialization.
///
/// This trait is used for basic binary de/serialization support. All
/// implementations ultimately serve to allow the `RipState` struct to be
/// saved to and reloaded from disk.
///
/// This is used instead of `serde`/`bincode` primarily for quality-control
/// reasons. We want to provide some basic sanity protections against changes
/// in formatting, byte drift, etc.
///
/// All operations are `Read`/`Write`-based to allow for flexible chaining.
pub(super) trait DeSerialize: Sized {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self>;

	/// # Serialized Length.
	fn serialized_len(&self) -> usize { std::mem::size_of::<Self>() }

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()>;
}



/// # De/Serialize Primitive Integer Types.
macro_rules! int {
	($ty:ty) => (
		impl DeSerialize for $ty {
			/// # Deserialize From Reader.
			fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
				let mut buf = [0_u8; std::mem::size_of::<Self>()];
				r.read_exact(&mut buf).ok()?;
				Some(Self::from_le_bytes(buf))
			}

			/// # Serialize Into Writer.
			fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
				w.write_all(self.to_le_bytes().as_slice()).ok()
			}
		}
	);
}

int!(u8);
int!(u32);

impl DeSerialize for bool {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		u8::deserialize_from(r).map(|v| 1 == v)
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize { SIZE_U8 }

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		u8::from(*self).serialize_into(w)
	}
}

impl RipSample {
	/// # Serialization Variant ID.
	///
	/// Return the numerical ID associated with the variant, used for
	/// de/serialization.
	const fn variant_id(&self) -> u8 {
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

impl DeSerialize for RipSample {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		let kind = u8::deserialize_from(r)?;
		match kind {
			1 => Some(Self::Lead),
			2 => Some(Self::Tbd),
			3 => Sample::deserialize_from(r).map(Self::Bad),
			4 => Sample::deserialize_from(r).map(|s|
				Self::Maybe(ContentiousSample::Maybe1((s, 1)))
			),
			5 => SampleCount::deserialize_from(r).map(|p|
				Self::Maybe(ContentiousSample::Maybe1(p))
			),
			6 => {
				let set = [
					SampleCount::deserialize_from(r)?,
					SampleCount::deserialize_from(r)?,
				];
				Some(Self::Maybe(ContentiousSample::Maybe2(set)))
			},
			7 | 8 => {
				let set = [
					SampleCount::deserialize_from(r)?,
					SampleCount::deserialize_from(r)?,
					SampleCount::deserialize_from(r)?,
				];
				if kind == 7 {
					Some(Self::Maybe(ContentiousSample::Maybe3(set)))
				}
				else {
					Some(Self::Maybe(ContentiousSample::Strict(set)))
				}
			},
			_ => None,
		}
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize {
		match self {
			Self::Lead | Self::Tbd => SIZE_U8,
			Self::Bad(_) => SIZE_U8 + SIZE_SAMPLE,
			Self::Maybe(ContentiousSample::Maybe1((_, count))) =>
				if 1 == *count { SIZE_U8 + SIZE_SAMPLE }
				else { SIZE_U8 + SIZE_SAMPLE_COUNT },
			Self::Maybe(ContentiousSample::Maybe2(_)) =>
				SIZE_U8 + SIZE_SAMPLE_COUNT * 2,
			Self::Maybe(ContentiousSample::Maybe3(_) | ContentiousSample::Strict(_)) =>
				SIZE_U8 + SIZE_SAMPLE_COUNT * 3,
		}
	}

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		// Start with the type.
		self.variant_id().serialize_into(w)?;

		// Write the data, if any.
		match self {
			Self::Bad(s) => s.serialize_into(w)?,
			Self::Maybe(ContentiousSample::Maybe1(pair)) =>
				if 1 == pair.1 { pair.0.serialize_into(w)?; }
				else { pair.serialize_into(w)?; },
			Self::Maybe(ContentiousSample::Maybe2(set)) => {
				set[0].serialize_into(w)?;
				set[1].serialize_into(w)?;
			}
			Self::Maybe(ContentiousSample::Maybe3(set) | ContentiousSample::Strict(set)) => {
				set[0].serialize_into(w)?;
				set[1].serialize_into(w)?;
				set[2].serialize_into(w)?;
			},
			_ => {},
		}

		Some(())
	}
}

impl DeSerialize for RipState {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		// Magic header.
		let mut buf = [0u8; MAGIC.len()];
		r.read_exact(&mut buf).ok()?;
		if buf != MAGIC { return None; }

		// CRC. We'll check this out later.
		let hash = u32::deserialize_from(r)?;

		// The TOC.
		let toc = Toc::deserialize_from(r)?;

		// The track.
		let track = u8::deserialize_from(r)?;
		let track =
			if track == 0 { toc.htoa()? }
			else { toc.audio_track(usize::from(track))? };

		// Derive the rip range.
		let rip_rng = super::data::track_rng_to_rip_range(track)?;

		// The data should match the range count.
		let len = rip_rng.len();
		let mut data = Vec::new();
		data.try_reserve_exact(len).ok()?;
		for _ in 0..len {
			let v = RipSample::deserialize_from(r)?;
			data.push(v);
		}

		// Put it together.
		let out = Self {
			toc, track, rip_rng, data,
			new: false,
		};

		// Check that hash from earlier, and return it if good.
		if out.quick_hash() == hash { Some(out) }
		else { None }
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize {
		MAGIC.len() +                 // Magic bytes.
		SIZE_U32 +                    // CRC32 rip slice hash.
		self.toc().serialized_len() + // Table of Contents.
		SIZE_U8 +                     // Track number.
		self.rip_slice()              // All the data.
			.iter()
			.fold(0_usize, |acc, v| acc.saturating_add(v.serialized_len()))
	}

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		w.write_all(MAGIC.as_slice()).ok()?;
		self.quick_hash().serialize_into(w)?;
		self.toc().serialize_into(w)?;
		self.track().number().serialize_into(w)?;
		for v in self.rip_slice() {
			v.serialize_into(w)?;
		}
		Some(())
	}
}

impl DeSerialize for Sample {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		let mut buf = NULL_SAMPLE;
		r.read_exact(&mut buf).ok()?;
		Some(buf)
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize { SIZE_SAMPLE }

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		w.write_all(self.as_slice()).ok()
	}
}

impl DeSerialize for SampleCount {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		let mut buf = [0_u8; SIZE_SAMPLE_COUNT];
		r.read_exact(&mut buf).ok()?;
		Some((
			[buf[0], buf[1], buf[2], buf[3]],
			buf[4],
		))
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize { SIZE_SAMPLE_COUNT }

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		w.write_all(&[
			self.0[0], self.0[1], self.0[2], self.0[3],
			self.1,
		]).ok()
	}
}

impl DeSerialize for Toc {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		// Read the audio parts.
		let audio_len = u8::deserialize_from(r)?;
		if audio_len == 0 || 99 < audio_len { return None; }
		let mut audio: Vec<u32> = Vec::new();
		audio.reserve_exact(usize::from(audio_len));
		for _ in 0..audio_len {
			let v = u32::deserialize_from(r)?;
			audio.push(v);
		}

		// Read the data part.
		let data = Option::<u32>::deserialize_from(r)?;

		// Leadout.
		let leadout = u32::deserialize_from(r)?;

		// Put it all together!
		Self::from_parts(audio, data, leadout).ok()
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize {
		SIZE_U8                               // Number of audio tracks.
		+ self.audio_len() * SIZE_U32         // Starting sector for each audio track.
		+ self.data_sector().serialized_len() // Data starting sector, if any.
		+ SIZE_U32                            // Leadout sector.
	}

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		// Audio first: length, each value.
		let audio = self.audio_sectors();
		u8::try_from(audio.len()).ok()?.serialize_into(w)?;
		for a in audio { a.serialize_into(w)?; }

		// Then data.
		self.data_sector().serialize_into(w)?;

		// And finally the leadout.
		self.leadout().serialize_into(w)
	}
}

impl<T: DeSerialize> DeSerialize for Option<T> {
	/// # Deserialize From Reader.
	fn deserialize_from<R: Read>(r: &mut R) -> Option<Self> {
		let any = bool::deserialize_from(r)?;
		if any {
			let t = T::deserialize_from(r)?;
			Some(Some(t))
		}
		else { Some(None) }
	}

	/// # Serialized Length.
	fn serialized_len(&self) -> usize {
		// One byte if None, otherwise one + T.
		self.as_ref().map_or(SIZE_U8, |t| SIZE_U8 + t.serialized_len())
	}

	/// # Serialize Into Writer.
	fn serialize_into<W: Write>(&self, w: &mut W) -> Option<()> {
		match self {
			None => false.serialize_into(w),
			Some(ref t) => {
				true.serialize_into(w)?;
				t.serialize_into(w)
			}
		}
	}
}



#[cfg(test)]
mod test {
	use super::*;
	use crate::RipOptions;
	use std::io::Cursor;

	const SAMPLE1: Sample = [1, 2, 3, 4];
	const SAMPLE2: Sample = [5, 6, 7, 8];
	const SAMPLE3: Sample = [9, 10, 11, 12];

	#[test]
	fn t_deserial_ripsample() {
		for v in [
			RipSample::Lead,
			RipSample::Tbd,
			RipSample::Bad(NULL_SAMPLE),
			RipSample::Bad(SAMPLE1),
			RipSample::Maybe(ContentiousSample::Maybe1((SAMPLE1, 1))),
			RipSample::Maybe(ContentiousSample::Maybe1((NULL_SAMPLE, 55))),
			RipSample::Maybe(ContentiousSample::Maybe2([
				(SAMPLE2, 2),
				(SAMPLE1, 1),
			])),
			RipSample::Maybe(ContentiousSample::Maybe3([
				(SAMPLE3, 3),
				(SAMPLE2, 2),
				(SAMPLE1, 1),
			])),
			RipSample::Maybe(ContentiousSample::Strict([
				(SAMPLE3, 3),
				(SAMPLE2, 2),
				(SAMPLE1, 1),
			])),
		] {
			// Test serialization.
			let mut buf = Vec::new();
			v.serialize_into(&mut buf);
			assert_eq!(buf.len(), v.serialized_len(), "RipSample serialize length mismatch.");

			// Test deserialization.
			let mut r = Cursor::new(buf.as_slice());
			let de = RipSample::deserialize_from(&mut r).expect("Unable to deserialize RipSample.");
			assert_eq!(v, de, "Input/output RipSample mismatch.");
		}
	}

	#[test]
	fn t_deserial_ripstate() {
		let opts = RipOptions::default().with_resume(false);
		let toc = Toc::from_cdtoc("12+B6+2161+454E+6D15+A8DB+D3C4+DFB8+F359+10E3C+1461F+154B4+1782E+18D71+1AF86+1C78C+1F498+203DE+22015+36231")
			.expect("Bad TOC.");

		// Test two very small tracks, including one in the HTOA.
		for t in [0_u8, 16] {
			let track =
				if t == 0 { toc.htoa() }
				else { toc.audio_track(usize::from(t)) }
				.expect("Bad track.");

			let mut state = RipState::from_track(&toc, track, &opts).expect("Bad state.");
			assert!(state.is_new(), "Expected a new state.");
			state.new = false; // Reset this since serialization will change it.

			// Test serialization.
			let len = usize::try_from(state.serialized_len()).expect("State length not usizeable.");
			let mut buf = Vec::with_capacity(len);
			state.serialize_into(&mut buf).expect("Unable to serialize state.");
			assert_eq!(buf.len(), len, "State serialize length mismatch.");

			// Test deserialization.
			let mut r = Cursor::new(buf.as_slice());
			let de = RipState::deserialize_from(&mut r).expect("Unable to deserialize RipState.");
			assert_eq!(state, de, "Input/output State mismatch.");
		}
	}

	#[test]
	fn t_deserial_toc() {
		// Test with and without data.
		for v in ["3+96+2D2B+6256+B327+D84A", "4+96+2D2B+6256+B327+D84A"] {
			let toc = Toc::from_cdtoc(v).expect("Bad TOC.");

			// Test serialization.
			let mut buf = Vec::new();
			toc.serialize_into(&mut buf).expect("Unable to serialize Toc.");
			assert_eq!(buf.len(), toc.serialized_len(), "Toc serialize length mismatch.");

			// Test deserialization.
			let mut r = Cursor::new(buf.as_slice());
			let de = Toc::deserialize_from(&mut r).expect("Unable to deserialize Toc.");
			assert_eq!(toc, de, "Input/output Toc mismatch.");
		}
	}
}
