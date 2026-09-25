/*!
# Rip Rip Hooray: SCSI Multimedia Commands

Provides opcodes, profiles, and format constants for SCSI MMC and core SPC commands
used to inspect and read audio CDs.
*/

use crate::{
	Barcode,
	CD_LEADIN,
	CD_LEADOUT,
	CddaDriverExt,
	DriveVendorModel,
	macros::log,
	RipRipError,
	TrackRange,
};
use std::range::legacy::Range;



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
	fn submit<const N: usize>(&self, cdb: &[u8; N], buf: &mut [u8], ctx: &'static str)
	-> Result<usize, RipRipError>;

	/// # Submit Command Descriptor Block (Checked).
	///
	/// Same as `submit`, but ensures at least `MIN_TRANSFERRED` bytes were written to
	/// `buf`, and returns an `Option` instead of a `Result`.
	fn submit_checked<const N: usize, const MIN_TRANSFERRED: usize>(
		&self,
		cdb: &[u8; N],
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
	/// # MCN From (Leadin) Subchannel.
	fn mcn_subchannel__(&self) -> Option<Barcode> {
		const FORMAT: u8 = 0x02;
		const ALLOC_LEN: usize = 4 + 20; // Four bytes header, 20 bytes data.

		#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
		const CDB: [u8; 10] = {
			assert!(
				ALLOC_LEN <= (u16::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u16."
			);

			let mut cdb = [0; 10];
			cdb[0] = MmcCmd::ReadSubchannel as u8;
			cdb[1] = AddressFormat::Msf as u8;
			cdb[2] = 0x40; // Sub-Q Channel tracking bit
			cdb[3] = FORMAT;
			[cdb[7], cdb[8]] = (ALLOC_LEN as u16).to_be_bytes();
			cdb
		};

		let mut buf = [0_u8; ALLOC_LEN];
		if
			self.submit_checked::<_, 9>(&CDB, &mut buf, "mcn_subchannel__").is_some() &&
			buf[3] == FORMAT &&  // Expected format.
			buf[4] == 0x01 &&    // Sub-Q valid.
			(buf[8] & 0x80) != 0 // MCVAL/TCVAL bit indicates a valid response.
		{
			Barcode::try_from(&buf[9..]).ok()
		}
		else { None }
	}

	/// # Check Disc Mode.
	fn check_disc_mode__(&self) -> Result<(), RipRipError> {
		const FIRST_TRACK: u8 = 0x01;
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		// Asks only for enough bytes to discover how large the TOC is.
		let mut buf = [0_u8; ALLOC_LEN];
		let mut cdb = MmcCmd::toc_cdb::<ALLOC_LEN>(TocFormat::Toc, FIRST_TRACK);
		self.submit_checked::<_, TOC_HEADER_LEN>(&cdb, &mut buf, "check_disc_mode__")
			.ok_or(RipRipError::DiscMode)?;

		let toc_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::DiscMode)?;

		// Patch the CDB with the expected allocation length and read again.
		[cdb[7], cdb[8]] = toc_len.to_be_bytes();

		let mut buf = vec![0_u8; toc_len.into()];
		let len = self.submit_checked::<_, TOC_HEADER_LEN>(
			&cdb,
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
		if len < required_len {
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

		#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
		const CDB: [u8; 10] = {
			assert!(
				ALLOC_LEN <= (u16::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u16."
			);

			let mut cdb = [0; _];
			cdb[0] = MmcCmd::GetConfiguration as u8;
			cdb[1] = 0x02; // RT field = 0x02: Request only the specific feature specified in bytes 2-3.
			[cdb[2], cdb[3]] = FEATURE_CD_AUDIO_C2.to_be_bytes();
			[cdb[7], cdb[8]] = (ALLOC_LEN as u16).to_be_bytes();
			cdb
		};

		let mut buf = [0_u8; ALLOC_LEN];
		self.submit_checked::<_, ALLOC_LEN>(&CDB, &mut buf, "check_c2__")
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
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		// Asks only for enough bytes to discover how large the CD-Text is.
		let mut buf = [0_u8; ALLOC_LEN];
		let mut cdb = MmcCmd::toc_cdb::<ALLOC_LEN>(TocFormat::CdText, 0);
		self.submit_checked::<_, ALLOC_LEN>(&cdb, &mut buf, "read_cdtext")
			.ok_or(RipRipError::CdText)?;

		let cdtext_len = u16::from_be_bytes([buf[0], buf[1]])
			.checked_add(2) // Length excludes the 2-byte length field itself.
			.ok_or(RipRipError::CdText)?;

		// No CD-Text exists on this disc.
		if cdtext_len as usize == TOC_HEADER_LEN { return Ok(None); }

		// Patch the CDB with the expected allocation length and read again.
		[cdb[7], cdb[8]] = cdtext_len.to_be_bytes();
		let mut buf = vec![0_u8; cdtext_len.into()];
		self.submit(&cdb, &mut buf, "read_cdtext").map_err(|_| RipRipError::CdText)?;

		// The parser can validate the buffer later on.
		Ok(Some(buf))
	}

	/// # Get Table of Contents Header.
	fn get_toc_header(&self) -> Result<(u8, u8), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN;

		let mut buf = [0_u8; ALLOC_LEN];
		let cdb = MmcCmd::toc_cdb::<ALLOC_LEN>(TocFormat::Toc, 0);
		self.submit_checked::<_, ALLOC_LEN>(&cdb, &mut buf, "get_toc_header")
			.ok_or(RipRipError::FirstTrackNum)?;

		TrackRange::new(buf[2], buf[3])
			.map(|v| (v.first_track(), v.last_track()))
			.ok_or(RipRipError::FirstTrackNum)
	}

	/// # Get Track Descriptor.
	fn get_track_descriptor(&self, idx: u8) -> Result<(u8, u32), RipRipError> {
		const ALLOC_LEN: usize = TOC_HEADER_LEN + TOC_TRACK_DESCRIPTOR_LEN;

		let mut buf = [0_u8; ALLOC_LEN];
		let cdb = MmcCmd::toc_cdb::<ALLOC_LEN>(TocFormat::Toc, idx);
		self.submit_checked::<_, ALLOC_LEN>(&cdb, &mut buf, "get_track_descriptor")
			.ok_or(RipRipError::TrackLba(idx))?;

		let control_adr = buf[5];
		let lba = u32::from_be_bytes([buf[8], buf[9], buf[10], buf[11]]);

		Ok((control_adr, lba))
	}

	/// # Drive Vendor/Model.
	fn drive_vendor_model__(&self) -> Result<DriveVendorModel, RipRipError> {
		const INQUIRY_HEADER_LEN: usize = 8;
		const INQUIRY_VENDOR_ID_LEN: usize = 8;
		const INQUIRY_PRODUCT_ID_LEN: usize = 16;
		const INQUIRY_REVISION_LEVEL_LEN: usize = 4;

		const VENDOR_ID_RANGE: Range<usize> =
			INQUIRY_HEADER_LEN..(INQUIRY_HEADER_LEN + INQUIRY_VENDOR_ID_LEN);

		const PRODUCT_ID_RANGE: Range<usize> =
			VENDOR_ID_RANGE.end..VENDOR_ID_RANGE.end + INQUIRY_PRODUCT_ID_LEN;

		const ALLOC_LEN: usize = INQUIRY_HEADER_LEN +
			INQUIRY_VENDOR_ID_LEN +
			INQUIRY_PRODUCT_ID_LEN +
			INQUIRY_REVISION_LEVEL_LEN;

		#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
		const CDB: [u8; 6] = {
			assert!(
				ALLOC_LEN <= (u8::MAX as usize),
				"BUG: `ALLOC_LEN` must fit u16."
			);
			let mut cdb = [0; 6];
			cdb[0] = MmcCmd::Inquiry as u8;
			cdb[4] = ALLOC_LEN as u8;
			cdb
		};

		let mut buf = [0_u8; ALLOC_LEN];
		self.submit_checked::<_, ALLOC_LEN>(&CDB, &mut buf, "drive_vendor_model__")
			.ok_or(RipRipError::DriveModel)?;

		let vendor_id = &buf[VENDOR_ID_RANGE];
		let model_id = &buf[PRODUCT_ID_RANGE];
		let revision_level = &buf[PRODUCT_ID_RANGE.end..];

		if let Ok(revision_level_str) = std::str::from_utf8(revision_level) {
			log!(@debug "Drive revision: {revision_level_str}.");
		}

		// Convert the raw bytes into UTF-8 strings.
		let Ok(vendor_id) = std::str::from_utf8(vendor_id) else {
			log!(@trace [vendor_id] "Invalid drive vendor.");
			return Err(RipRipError::DriveVendor);
		};
		let Ok(model_id) = std::str::from_utf8(model_id) else {
			log!(@trace [model_id] "Invalid drive model.");
			return Err(RipRipError::DriveVendor);
		};

		DriveVendorModel::new(vendor_id, model_id)
	}

	/// # Execute Read Command.
	fn read_cd__(&self, buf: &mut [u8], lsn: i32, c2: bool, sub: u8) -> Result<usize, RipRipError> {
		const SECTOR_TYPE_CDDA: u8 = 0x04;

		const CDB: [u8; 12] = {
			let mut cdb = [0; _];
			cdb[0] = MmcCmd::ReadCd as u8;
			cdb[1] = SECTOR_TYPE_CDDA;

			// Transfer Length (24-bit BE integer): one sector.
			// cdb[6] = 0; // Already zero.
			// cdb[7] = 0; // Already zero.
			cdb[8] = 1;
			cdb
		};

		let mut cdb = CDB;
		// Rip Rip's addressing parameters are already absolute LBAs.
		let lba = u32::try_from(lsn).map_err(|_| RipRipError::CdRead)?;
		[cdb[2], cdb[3], cdb[4], cdb[5]] = lba.to_be_bytes();

		// Byte 9 is the Selection Field flag byte:
		// Bit 4: User Data Selection (Set to 1 to read the 2352 bytes audio payload)
		// Bit 2..1: C2 Error Flag selection allocation (Set to 1 to include 294 bytes C2 space)
		let user_data_flag = 0x10;
		let c2_flag = if c2 { 0x02 } else { 0x00 };
		cdb[9] = user_data_flag | c2_flag;

		// Byte 10 defines the Subchannel Selection configuration flags:
		// 0 = none, 1 = raw P-W, 2 = formatted Q, 4 = corrected R-W.
		cdb[10] = sub;

		self.submit(&cdb, buf, "read_cd__")
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

	/// # MCN From (Leadin) Subchannel.
	fn mcn_subchannel(&self) -> Option<Barcode> {
		let out = self.mcn_subchannel__();
		if out.is_none() { log!(@trace "Subchannel contains no MCN data."); }
		out
	}

	/// # Execute Read Command.
	fn read_cd(
		&self,
		buf: &mut [u8],
		lsn: i32,
		c2: bool,
		sub: u8,
		_block_size: u16,
	) -> Result<(), RipRipError> {
		if self.read_cd__(buf, lsn, c2, sub).is_ok() { Ok(()) }
		else {
			crate::drivers::set_bad_sector(lsn);
			Err(RipRipError::CdRead)
		}
	}
}



/// # Helper: Address Formats.
macro_rules! address_fmt {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # (Relevant) Subchannel/ToC Address Formats.
		///
		/// This enum holds the address format codes used for
		/// `ReadSubchannel` and `ReadToc` commands.
		enum AddressFormat {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k = $v,
			)+
		}
	);
}

address_fmt! {
	Lba 0x00 "LBA",
	Msf 0x02 "MSF",
}



/// # Helper: MMC Commands.
macro_rules! cmd {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # (Relevant) MMC Commands.
		///
		/// This enum holds the MMC commands used by Rip Rip. (There are oh so
		/// many more!)
		enum MmcCmd {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k = $v,
			)+
		}

		impl MmcCmd {
			#[expect(clippy::cast_possible_truncation, reason = "False positive.")]
			/// # TOC Command Descriptor Block.
			///
			/// Return a `ReadToc` Command Descriptor Block for the given
			/// size, format, and track number.
			///
			/// Note the address format is always LBA.
			const fn toc_cdb<const ALLOC_LEN: usize>(
				toc_format: TocFormat,
				track: u8,
			) -> [u8; 10] {
				assert!(
					ALLOC_LEN <= (u16::MAX as usize),
					"BUG: `ALLOC_LEN` must fit u16."
				);

				let mut cdb = [0; 10];
				cdb[0] = MmcCmd::ReadToc as u8;
				cdb[1] = AddressFormat::Lba as u8;
				cdb[2] = toc_format as u8;
				cdb[6] = track;
				[cdb[7], cdb[8]] = (ALLOC_LEN as u16).to_be_bytes();
				cdb
			}
		}
	);
}

cmd! {
	GetConfiguration 0x46 "Get Configuration",
	Inquiry          0x12 "Inquiry",
	ReadCd           0xBE "Read CD",
	ReadSubchannel   0x42 "Read Subchannel",
	ReadToc          0x43 "Read TOC",
}



/// # Helper: ToC Formats.
macro_rules! toc_fmt {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, PartialEq)]
		/// # (Relevant) Table of Contents Format Codes.
		///
		/// This enum holds the output formats for `ReadToc` commands.
		enum TocFormat {
			$(
				#[doc = concat!("# ", $str, ".")]
				$k = $v,
			)+
		}
	);
}

toc_fmt! {
	Toc    0x00 "Standard TOC",
	CdText 0x05 "CD-Text",
}
