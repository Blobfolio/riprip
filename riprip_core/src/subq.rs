/*!
# Rip Rip Hooray: Formatted Q Subchannel Packets.
*/

use crate::CRC;



/// # Helper: Sub-Q.
macro_rules! subq {
	(
		$(
			$( #[doc = $doc:expr] )*
			$k:ident $adr:literal,
		)+
	) => (
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # Sub Q Packet.
		///
		/// This enum qualifies the different kinds of subchannel Q data
		/// packets of interest.
		pub(crate) enum SubQ {
			$(
				$( #[doc = $doc] )*
				$k([u8; 9]),
			)+
		}

		impl SubQ {
			#[must_use]
			/// # From Raw Packet.
			///
			/// Check and chop the payload from a raw 16-byte formatted
			/// subchannel packet. Returns `None` if the CRC validation fails
			/// or the ADR type is unsupported.
			pub(crate) const fn new(raw: &[u8; 16]) -> Option<Self> {
				let chk_actual = chk10(&raw);
				let chk_expected = u16::from_be_bytes([raw[10], raw[11]]);
				if chk_actual == chk_expected {
					let inner: [u8; 9] = [
						raw[1], raw[2], raw[3], raw[4], raw[5], raw[6],
						raw[7], raw[8], raw[9],
					];
					match raw[0] & 0b0000_1111 {
						$( $adr => Some(Self::$k(inner)), )+
						_ => None,
					}
				}
				else { None }
			}
		}
	);
}

subq! {
	/// # Timing/Position.
	Timing 0x01,

	/// # MCN.
	Mcn    0x02,

	/// # Track ISRC.
	Isrc   0x03,
}



#[must_use]
/// # Checksum 10 Bytes.
const fn chk10(raw: &[u8; 16]) -> u16 {
	let mut crc = 0_u16;
	let mut i = 0;
	while i < 10 {
		let idx = (((crc >> 8) ^ (raw[i] as u16)) & 0xFF) as usize;
		crc = CRC[idx] ^ (crc << 8);
		i += 1;
	}
	crc ^ 0xFFFF
}
