/*!
# Rip Rip Hooray: SCSI Multimedia Commands

Provides opcodes, profiles, and format constants for SCSI MMC and core SPC commands
used to inspect and read audio CDs.
*/

use crate::{
	CD_LEADIN,
	CD_LEADOUT,
	CddaDriverExt,
	DriveVendorModel,
	macros::log,
	RipRipError,
	TrackRange,
};
use dactyl::NiceElapsed;
use std::{
	num::NonZeroU8,
	range::legacy::Range,
	time::{
		Duration,
		Instant,
	},
};
use super::ReadCdOpts;



/// # Table of Contents Header Length.
pub(super) const TOC_HEADER_LEN: usize = 4;

/// # Table of Contents Track Descriptor Length.
const TOC_TRACK_DESCRIPTOR_LEN: usize = 8;

/// # Track Type Mask.
///
/// If set the type is data, otherwise audio.
pub(super) const CTRL_DATA_TRACK: u8 = 0x04;



/// # Transport Trait.
///
/// This trait provides a single method, `submit`, for sending an SCSI Command
/// Descriptor Block (CDB) to the device and reading its response.
pub(super) trait TransportExt {
	/// # Submit Command Descriptor Block.
	///
	/// Submit the CDB to the device and read the response into `buf`,
	/// returning the length written.
	///
	/// Note `ctx` is only used to help qualify trace logs.
	fn submit(
		&self,
		cdb: &CommandDescriptorBlock,
		buf: &mut [u8],
		ctx: &'static str,
	) -> Result<usize, RipRipError>;

	/// # Submit Command Descriptor Block (Checked).
	///
	/// Same as `submit`, but ensures at least `MIN_TRANSFERRED` bytes were written to
	/// `buf`, and returns an `Option` instead of a `Result`.
	fn submit_checked<const MIN_TRANSFERRED: usize>(
		&self,
		cdb: &CommandDescriptorBlock,
		buf: &mut [u8],
		ctx: &'static str,
	) -> Option<usize> {
		let transferred = self.submit(cdb, buf, ctx).ok()?;
		if transferred < MIN_TRANSFERRED {
			log!(
				@trace [ctx, cdb, buf]
				"CDB expected at least {MIN_TRANSFERRED} bytes, received {transferred}.",
			);
			None
		}
		else { Some(transferred) }
	}
}

impl<T: TransportExt + ?Sized> MmcDriverExt for T {}



/// # MMC CD Drive Operations.
///
/// This is a low-level API for interacting with a physical CD drive,
/// specifically to read audio data.
///
/// Relies on the underlying `TransportExt` trait to handle the hardware bus
/// communication (e.g. USB BOT or `/dev/sg`).
pub(super) trait MmcDriverExt: TransportExt {
	/// # Check Disc Mode.
	fn check_disc_mode__(&self) -> Result<(), RipRipError> {
		// Make sure the device is ready first.
		self.test_unit_ready__()?;

		// Asks only for enough bytes to discover how large the TOC is.
		let mut buf = [0_u8; TOC_HEADER_LEN];
		self.submit_checked::<TOC_HEADER_LEN>(
			&CommandDescriptorBlock::read_toc(TocFormatCode::Toc, 0x01, None),
			&mut buf,
			"check_disc_mode__",
		)
			.ok_or(RipRipError::DiscMode)?;

		let toc_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::DiscMode)?;

		// Patch the CDB with the expected allocation length and read again.
		let mut buf = vec![0_u8; toc_len.into()];
		let len = self.submit_checked::<TOC_HEADER_LEN>(
			&CommandDescriptorBlock::read_toc(TocFormatCode::Toc, 0x01, Some(toc_len)),
			&mut buf,
			"check_disc_mode__",
		).ok_or(RipRipError::DiscMode)?;

		let first_track = buf[2];
		let last_track = buf[3];
		let rng = TrackRange::new(first_track, last_track)
			.ok_or(RipRipError::DiscMode)?;
		let track_count = usize::from(rng.len());
		let total_count = track_count + 1; // Lead-out.

		// The actual length we should have is a bit more nuanced.
		let required_len = TOC_HEADER_LEN + total_count * TOC_TRACK_DESCRIPTOR_LEN;
		if len < required_len || buf.len() < required_len {
			return Err(RipRipError::DiscMode);
		}

		// Search the descriptors. If an audio track is found, early exit,
		// otherwise default to a DiscMode error.
		let has_audio = buf[TOC_HEADER_LEN..]
			.as_chunks::<TOC_TRACK_DESCRIPTOR_LEN>()
			.0
			.iter()
			.take(track_count)
			.any(|desc| (desc[1] & CTRL_DATA_TRACK) == 0);

		if has_audio { Ok(()) }
		else { Err(RipRipError::DiscMode) }
	}

	/// # Check C2.
	fn check_c2__(&self) -> Result<(), RipRipError> {
		const CONFIGURATION_FEATURE_DESCRIPTOR_LEN: usize = 8;
		const CONFIGURATION_HEADER_LEN: usize = 8;
		const FEATURE_CD_AUDIO_C2: u16 = 0x001E;
		const ALLOC_LEN: usize = CONFIGURATION_HEADER_LEN + CONFIGURATION_FEATURE_DESCRIPTOR_LEN;

		let mut buf = [0_u8; ALLOC_LEN];
		self.submit_checked::<ALLOC_LEN>(
			&CommandDescriptorBlock::get_config_feature::<ALLOC_LEN>(ConfigFeature::AudioC2),
			&mut buf,
			"check_c2__",
		)
			.ok_or(RipRipError::C2Mode296)?;

		// Byte 4 houses the Feature-Specific configuration flags. Bit 0 is
		// the C2 Validity flag, indicating the drive can deliver C2 data over
		// bus pipelines.
		if
			u16::from_be_bytes([
				buf[CONFIGURATION_HEADER_LEN],
				buf[CONFIGURATION_HEADER_LEN + 1],
			]) == FEATURE_CD_AUDIO_C2 &&
			(buf[CONFIGURATION_HEADER_LEN + 4] & 0x01) != 0
		{
			Ok(())
		}
		// Boo!
		else { Err(RipRipError::C2Mode296) }
	}

	/// # CD-Text (Raw).
	fn read_cdtext(&self) -> Result<Option<Vec<u8>>, RipRipError> {
		// Asks only for enough bytes to discover how large the CD-Text is.
		let mut buf = [0_u8; TOC_HEADER_LEN];
		self.submit_checked::<TOC_HEADER_LEN>(
			&CommandDescriptorBlock::read_toc(TocFormatCode::CdText, 0, None),
			&mut buf,
			"read_cdtext",
		)
			.ok_or(RipRipError::CdText)?;

		let cdtext_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::CdText)?;

		// No CD-Text exists on this disc.
		if cdtext_len as usize == TOC_HEADER_LEN { return Ok(None); }

		// Patch the CDB with the expected allocation length and read again.
		let mut buf = vec![0_u8; usize::from(cdtext_len)];
		self.submit(
			&CommandDescriptorBlock::read_toc(TocFormatCode::CdText, 0, Some(cdtext_len)),
			&mut buf,
			"read_cdtext",
		)
			.map_err(|_| RipRipError::CdText)
			.map(|_| Some(buf))
	}

	/// # Get Table of Contents Header.
	fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
		let mut buf = [0_u8; TOC_HEADER_LEN];
		self.submit_checked::<TOC_HEADER_LEN>(
			&CommandDescriptorBlock::read_toc(TocFormatCode::Toc, 0, None),
			&mut buf,
			"get_toc_header",
		)
			.ok_or(RipRipError::FirstTrackNum)?;

		TrackRange::new(buf[2], buf[3])
			.map(|v| (v.first_track(), v.last_track()))
			.ok_or(RipRipError::FirstTrackNum)
	}

	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	/// # Get Track Descriptor.
	fn get_track_descriptor(&self, idx: u8) -> Result<(u8, u32), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN + TOC_TRACK_DESCRIPTOR_LEN;
		const {
			assert!(
				ALLOC_LEN <= (u16::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u16.",
			);
		}

		let mut buf = [0_u8; ALLOC_LEN];
		self.submit_checked::<ALLOC_LEN>(
			&CommandDescriptorBlock::read_toc(TocFormatCode::Toc, idx, Some(ALLOC_LEN as u16)),
			&mut buf,
			"get_track_descriptor",
		)
			.ok_or(RipRipError::TrackLba(idx))?;

		let control_adr = buf[5];
		let lba = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

		Ok((control_adr, lba))
	}

	/// # Drive Vendor/Model.
	fn drive_vendor_model__(&self) -> Result<DriveVendorModel, RipRipError> {
		const INQUIRY_HEADER_LEN: usize = 8;

		const VENDOR_ID_RANGE: Range<usize> =
			INQUIRY_HEADER_LEN..(INQUIRY_HEADER_LEN + DriveVendorModel::VENDOR_LEN);

		const PRODUCT_ID_RANGE: Range<usize> =
			VENDOR_ID_RANGE.end..(VENDOR_ID_RANGE.end + DriveVendorModel::MODEL_LEN);

		const ALLOC_LEN: usize = INQUIRY_HEADER_LEN +
			DriveVendorModel::VENDOR_LEN +
			DriveVendorModel::MODEL_LEN +
			DriveVendorModel::REVISION_LEN;

		let mut buf = [0_u8; ALLOC_LEN];
		self.submit_checked::<ALLOC_LEN>(
			&CommandDescriptorBlock::inquiry::<ALLOC_LEN>(),
			&mut buf,
			"drive_vendor_model__",
		)
			.ok_or(RipRipError::DriveModel)?;

		let vendor_id = &buf[VENDOR_ID_RANGE];
		let model_id = &buf[PRODUCT_ID_RANGE];
		let revision_level = &buf[PRODUCT_ID_RANGE.end..];

		DriveVendorModel::new(vendor_id, model_id, revision_level)
	}

	/// # Execute Read Command.
	fn read_cd__(&self, buf: &mut [u8], lsn: i32, opts: ReadCdOpts)
	-> Result<usize, RipRipError> {
		let num_blocks = match opts.num_blocks(buf) {
			Ok(v) => v,
			Err(e) => {
				log!(@trace [buf.len(), opts] "{e}");
				return Err(RipRipError::Bug(e));
			},
		};

		// Rip Rip's addressing parameters are already absolute LBAs.
		let lba = u32::try_from(lsn).map_err(|_| RipRipError::CdRead)?;
		let cdb = CommandDescriptorBlock::read_cd(lba, num_blocks, opts);
		self.submit(&cdb, buf, "read_cd__")
	}

	/// # Test Unit Ready.
	///
	/// Poll the device to find out if/when it's ready.
	fn test_unit_ready__(&self) -> Result<(), RipRipError> {
		// Timings.
		const RETRY_DELAY: Duration = Duration::from_millis(250);
		const RETRY_TIMEOUT: Duration = Duration::from_secs(15);

		let now = Instant::now();
		let mut retried = false;
		let mut limited_retry = 0;
		loop {
			match self.submit(&CommandDescriptorBlock::TEST_UNIT_READY, &mut [], "test_unit_ready__") {
				// Ready!
				Ok(_) => {
					if retried {
						log!(
							@trace
							"Device ready after {}.",
							NiceElapsed::from(now.elapsed()),
						);
					}

					return Ok(())
				},

				// Don't know. Let's see.
				Err(RipRipError::TestUnitNotReady) => {
					let mut buf = [0_u8; 18];
					self.submit(&CommandDescriptorBlock::REQUEST_SENSE, &mut buf, "test_unit_ready__")?;

					// Only three of those bytes are relevant.
					let key = buf[2] & 0x0f;
					let asc = buf[12];
					let ascq = buf[13];

					// If it's getting ready, wait and loop back around.
					match [key, asc, ascq] {
						// Not there yet, wait and retry.
						[ 0x02, 0x04, 0x00 | 0x01 | 0x07 ] |
						[ 0x06, 0x28, 0x00 ] |
						[ 0x06, 0x29, 0x00..=0x04 ] => {},

						// Conditionally retry.
						[ 0x02, 0x04, 0x02 ] |
						[ 0x06, 0x2A, 0x00..=0x02 ] if limited_retry < 2 => {
							limited_retry += 1;
						},

						// An actual error, probably.
						_ => return Err(RipRipError::DiscMode),
					}

					// Mention that we're gonna be polling, but only once.
					if ! retried {
						log!(@trace "Waiting for device to become ready.");
						retried = true;
					}
					// If it's taking forever, return an error.
					else if RETRY_TIMEOUT < now.elapsed() {
						log!(
							@trace
							"Device still not ready after {}; giving up.",
							NiceElapsed::from(now.elapsed()),
						);
						return Err(RipRipError::TestUnitTimeout);
					}

					// Wait a bit before looping back around.
					std::thread::sleep(RETRY_DELAY);
				},

				// Something is wrong.
				Err(e) => return Err(e),
			}
		}
	}
}

impl<T: MmcDriverExt> CddaDriverExt for T {
	/// # First Track Number.
	fn first_track_num(&self) -> Result<u8, RipRipError> {
		let (first, _) = self.get_toc_header()?;

		if first == 0 { Err(RipRipError::FirstTrackNum) }
		else { Ok(first) }
	}

	/// # Leadout.
	fn leadout_lba(&self) -> Result<u32, RipRipError> {
		// In the SCSI MMC specification, the leadout track information is
		// explicitly queried using the standard magic track index 0xAA.
		self.track_lba_start(CD_LEADOUT)
	}

	/// # Get the Number of Tracks.
	fn num_tracks(&self) -> Result<u8, RipRipError> {
		let (first, last) = self.get_toc_header()?;

		if last == 0 { Err(RipRipError::NumTracks) }
		else {
			// Handles discs that might not explicitly start at track 1.
			Ok(last - first + 1)
		}
	}

	/// # Track Format.
	fn track_format(&self, idx: u8) -> Result<bool, RipRipError> {
		let (control_adr, _) = self
			.get_track_descriptor(idx)
			.map_err(|_| RipRipError::TrackFormat(idx))?;

		// In SCSI MMC TOC structures, the 4-bit CONTROL field dictates data types.
		// Bit 2 (0x04) is set if the track is a data track, and clear if it's audio.
		let is_data = (control_adr & CTRL_DATA_TRACK) > 0;

		Ok(! is_data)
	}

	/// # Track LBA Start.
	fn track_lba_start(&self, idx: u8) -> Result<u32, RipRipError> {
		if idx == 0 { return Err(RipRipError::TrackNumber(0)); }

		let (_, lba) = self
			.get_track_descriptor(idx)
			.map_err(|_| RipRipError::TrackLba(idx))?;

		Ok(lba + u32::from(CD_LEADIN))
	}

	/// # CD-Text (Raw).
	fn cdtext(&self) -> Option<Vec<u8>> {
		if let Ok(Some(mut buf)) = self.read_cdtext() && TOC_HEADER_LEN < buf.len() {
			// Skip the header.
			buf.copy_within(TOC_HEADER_LEN.., 0);
			buf.truncate(buf.len() - TOC_HEADER_LEN);
			Some(buf)
		}
		else { None }
	}

	/// # Drive Vendor/Model.
	fn drive_vendor_model(&self) -> Option<DriveVendorModel> {
		self.drive_vendor_model__().ok()
	}

	/// # Execute Read Command.
	fn read_cd(&self, buf: &mut [u8], lsn: i32, opts: ReadCdOpts)
	-> Result<(), RipRipError> {
		if self.read_cd__(buf, lsn, opts).is_ok() { Ok(()) }
		else {
			crate::drivers::set_bad_sector(lsn);
			Err(RipRipError::CdRead)
		}
	}
}



#[derive(Debug, Clone, Copy, Eq, PartialEq)]
/// # Command Descriptor Block.
///
/// This struct is used to construct raw MMC commands (that can be sent to the
/// device).
pub(super) enum CommandDescriptorBlock {
	/// # Six Bytes.
	Six([u8; 6]),

	/// # Ten Bytes.
	Ten([u8; 10]),

	/// # Twelve Bytes.
	Twelve([u8; 12]),
}

impl CommandDescriptorBlock {
	#[expect(clippy::many_single_char_names, reason = "It's fine…")]
	#[must_use]
	/// # Fixed Array.
	///
	/// Return the raw command, padded on the right for a consistent twelve
	/// bytes.
	pub(super) const fn as_fixed(self) -> [u8; 12] {
		match self {
			Self::Six([a, b, c, d, e, f]) =>             [a, b, c, d, e, f, 0, 0, 0, 0, 0, 0],
			Self::Ten([a, b, c, d, e, f, g, h, i, j]) => [a, b, c, d, e, f, g, h, i, j, 0, 0],
			Self::Twelve(inner) => inner,
		}
	}

	#[must_use]
	/// # Is Test Unit Ready Command?
	///
	/// Returns `true` if the command is for testing unit readiness.
	pub(super) const fn is_test_unit_ready(self) -> bool {
		matches!(self, Self::TEST_UNIT_READY)
	}

	#[must_use]
	/// # Length.
	pub(super) const fn len(&self) -> u8 {
		match self {
			Self::Six(_) =>     6,
			Self::Ten(_) =>    10,
			Self::Twelve(_) => 12,
		}
	}
}

/// ## Six-Byte Commands.
impl CommandDescriptorBlock {
	/// # Request Sense.
	const REQUEST_SENSE: Self = Self::Six([
		0x03,
		0x00, // DESC bit: 0 fixed, 1 descriptor.
		0x00, // Reserved.
		0x00, // Reserved.
		0x12, // Eighteen bytes.
		0x00, // Control.
	]);

	/// # Test Unit Ready.
	const TEST_UNIT_READY: Self = Self::Six([0_u8; 6]);

	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	#[must_use]
	/// # Inquiry.
	const fn inquiry<const ALLOC_LEN: usize>() -> Self {
		const {
			assert!(
				ALLOC_LEN <= (u8::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u8.",
			);
		}

		Self::Six([
			0x12,
			0x00, // EVPD.
			0x00, // Page code.
			0x00, // Reserved.
			ALLOC_LEN as u8,
			0x00, // Control.
		])
	}
}

/// ## Ten-Byte Commands.
impl CommandDescriptorBlock {
	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	#[must_use]
	/// # Get Configuration Feature.
	const fn get_config_feature<const ALLOC_LEN: usize>(feature: ConfigFeature)
	-> Self {
		const {
			assert!(
				ALLOC_LEN <= (u16::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u16.",
			);
		}

		let [len_a, len_b] = (ALLOC_LEN as u16).to_be_bytes();
		let [feat_a, feat_b] = (feature as u16).to_be_bytes();

		Self::Ten([
			0x46,
			0x02, // Request type, feature.
			feat_a,
			feat_b,
			0x00, // Reserved.
			0x00, // Reserved.
			0x00, // Reserved.
			len_a,
			len_b,
			0x00, // Control.
		])
	}

	#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
	#[must_use]
	/// # Read TOC.
	const fn read_toc(
		format_code: TocFormatCode,
		track: u8,
		alloc_len: Option<u16>,
	) -> Self {
		const {
			assert!(
				TOC_HEADER_LEN <= (u16::MAX as usize),
				"BUG: `TOC_HEADER_LEN` must fit u16.",
			);
		}

		let alloc_len =
			if let Some(v) = alloc_len { v }
			else { TOC_HEADER_LEN as u16 };

		let [len_a, len_b] = alloc_len.to_be_bytes();
		Self::Ten([
			0x43,
			0x00, // Address format, 0 LBA, 1 MSF.
			format_code as u8,
			0x00, // Reserved.
			0x00, // Reserved.
			0x00, // Reserved.
			track,
			len_a,
			len_b,
			0x00, // Control.
		])
	}
}

/// ## Twelve-Byte Commands.
impl CommandDescriptorBlock {
	#[must_use]
	/// # Read CD.
	const fn read_cd(lba: u32, num_blocks: NonZeroU8, opts: ReadCdOpts)
	-> Self {
		let [lba_a, lba_b, lba_c, lba_d] = lba.to_be_bytes();

		let flag_cdda = if opts.cdda() { 0x10 } else { 0x00 };
		let flag_c2   = if opts.c2()   { 0x02 } else { 0x00 };

		Self::Twelve([
			0xBE,
			0x04,                     // Sector type, CDDA.
			lba_a,
			lba_b,
			lba_c,
			lba_d,
			0x00,                     // Number of sectors to read.
			0x00,                     // Number of sectors to read.
			num_blocks.get(),         // Number of sectors to read.
			flag_cdda | flag_c2,      // Data selection.
			opts.subchannel_format(), // Subchannel.
			0x00, // Control.
		])
	}
}



/// # Helper: CDB argument enums.
macro_rules! cdb_args {
	(
		$(
			#[repr($repr:ty)]
			$( #[doc = $doc:expr] )*
			$enum:ident
			$( $k:ident $v:literal $title:literal, )+
		)+
	) => (
		$(
			#[repr($repr)]
			$( #[doc = $doc] )*
			enum $enum {
				$(
					#[doc = concat!("# ", $title, ".")]
					$k = $v,
				)+
			}
		)+
	);
}

cdb_args! {
	#[repr(u16)]
	/// # Configuration Features.
	///
	/// We're only checking for C2 support at the moment, but might want more
	/// later.
	ConfigFeature
	AudioC2 0x001E "C2 Support",

	#[repr(u8)]
	/// # TOC Format Code.
	TocFormatCode
	Toc    0x00 "Standard TOC",
	CdText 0x05 "CD-Text",
}
