/*!
# Rip Rip Hooray: Errors
*/

use cdtoc::TocError;
use fyi_msg::Msg;
use std::{
	error::Error,
	fmt,
};



#[allow(missing_docs)]
#[derive(Debug, Clone, Eq, PartialEq)]
/// # Errors.
pub enum RipRipError {
	/// # Invalid barcode.
	Barcode,

	/// # Bug!
	Bug(&'static str),

	/// # Cache directory.
	Cache,

	/// # Cache Path.
	CachePath(String),

	/// # CDTOC passthrough.
	Cdtoc(TocError),

	/// # CD read error.
	CdRead(i32),

	/// # CD read operation terminal failure.
	CdReadUnsupported,

	/// # Invalid device.
	Device(String),

	/// # Unable to open device.
	DeviceOpen(Option<String>),

	/// # Unsupported Disc.
	DiscMode,

	/// # Invalid drive model.
	DriveModel,

	/// # Invalid drive vendor.
	DriveVendor,

	/// # Unable to get first track number.
	FirstTrackNum,

	/// # User Abort.
	Killed,

	/// # Unable to get leadout.
	Leadout,

	/// # Unable to obtain the number of tracks.
	NumTracks,

	/// # Read Offset.
	ReadOffset,

	/// # Numbers can't be converted to the necessary types.
	RipOverflow(u8),

	/// # State Corruption.
	StateCorrupt(u8),

	/// # State Save.
	StateSave(u8),

	/// # Invalid/unsupported track format.
	TrackFormat(u8),

	/// # Invalid track LBA.
	TrackLba(u8),

	/// # Invalid track number.
	TrackNumber(u8),

	/// # Writing to disk.
	Write(String),

	#[cfg(feature = "bin")]
	/// # General CLI issues.
	Argue(argyle::ArgyleError),

	#[cfg(feature = "bin")]
	/// # CLI Parsing failure.
	CliParse(&'static str),
}

impl Error for RipRipError {}

#[cfg(feature = "bin")]
impl From<argyle::ArgyleError> for RipRipError {
	#[inline]
	fn from(err: argyle::ArgyleError) -> Self { Self::Argue(err) }
}

impl From<TocError> for RipRipError {
	#[inline]
	fn from(err: TocError) -> Self { Self::Cdtoc(err) }
}

impl From<RipRipError> for Msg {
	#[inline]
	fn from(src: RipRipError) -> Self { Self::error(src.to_string()) }
}

impl fmt::Display for RipRipError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		match self {
			Self::Barcode => f.write_str("Invalid/unsupported barcode."),
			Self::Bug(s) => write!(f, "Bug: {s}."),
			Self::Cache => f.write_str("Unable to establish a cache directory."),
			Self::CachePath(ref s) => write!(f, "Invalid cache path {s}."),
			Self::CdRead(n) => write!(f, "Unable to read sector {n}."),
			Self::CdReadUnsupported => f.write_str("Unable to read CD; settings are probably wrong."),
			Self::Cdtoc(s) => write!(f, "{s}"),
			Self::Device(ref s) => write!(f, "Invalid device path {s}."),
			Self::DeviceOpen(ref s) =>
				if let Some(s) = s { write!(f, "Unable to open connection with {s}.") }
				else {
					f.write_str("Unable to open connection with default optical drive.")
				},
			Self::DiscMode => f.write_str("Missing or unsupported disc type."),
			Self::DriveModel => f.write_str("Invalid drive model."),
			Self::DriveVendor => f.write_str("Invalid drive vendor."),
			Self::FirstTrackNum => f.write_str("Unable to obtain the first track index."),
			Self::Killed => f.write_str("User abort."),
			Self::Leadout => f.write_str("Unable to obtain leadout."),
			Self::NumTracks => f.write_str("Unable to obtain the track total."),
			Self::ReadOffset => f.write_str("Invalid read offset."),
			Self::RipOverflow(n) => write!(f, "Track #{n} cannot be ripped on this system."),
			Self::StateCorrupt(n) => write!(f, "The state data for track #{n} is corrupt."),
			Self::StateSave(n) => write!(f, "Unable to save the state data for track #{n}."),
			Self::TrackFormat(n) => write!(f, "Unsupported track type ({n})."),
			Self::TrackLba(n) => write!(f, "Unable to obtain LBA ({n})."),
			Self::TrackNumber(n) => write!(f, "Invalid track number ({n})."),
			Self::Write(ref s) => write!(f, "Unable to write to {s}."),

			#[cfg(feature = "bin")]
			Self::Argue(a) => f.write_str(a.as_str()),

			#[cfg(feature = "bin")]
			Self::CliParse(s) => write!(f, "Unable to parse {s}."),
		}
	}
}
