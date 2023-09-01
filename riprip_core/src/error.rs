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
	/// # Cache directory.
	Cache,

	/// # CDTOC passthrough.
	Cdtoc(TocError),

	/// # CD read error.
	CdRead(i32),

	/// # Invalid buffer for CD reading.
	CdReadBuffer,

	/// # CD read operation terminal failure.
	CdReadUnsupported,

	/// # File delete.
	Delete(String),

	/// # Invalid device.
	Device(String),

	/// # Unable to open device.
	DeviceOpen(Option<String>),

	/// # Unsupported Disc.
	DiscMode,

	/// # Unable to get first track number.
	FirstTrackNum,

	/// # User Abort.
	Killed,

	/// # Unable to get leadout.
	Leadout,

	/// # No tracks/empty disc.
	NoTracks,

	/// # Unable to obtain the number of tracks.
	NumTracks,

	/// # Unable to parse paranoia.
	Paranoia,

	/// # Read Offset.
	ReadOffset,

	/// # Reconfirm/Paranoia conflict.
	ReconfirmParanoia,

	/// # Unable to parse passes.
	Refine,

	/// # Numbers can't be converted to the necessary types.
	RipOverflow(u8),

	/// # Unable to parse rip tracks.
	RipTracks,

	/// # Invalid/unsupported track format.
	TrackFormat(u8),

	/// # Invalid track LBA.
	TrackLba(u8),

	/// # Invalid track number.
	TrackNumber(u8),

	/// # Writing to disk.
	Write(String),

	#[cfg(feature = "bin")]
	/// # CLI issues.
	Argue(argyle::ArgyleError),
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
			Self::Cache => f.write_str("Unable to establish a cache directory."),
			Self::CdRead(n) => write!(f, "Unable to read sector {n}."),
			Self::CdReadBuffer => f.write_str("BUG: Insufficient CD read buffer."),
			Self::CdReadUnsupported => f.write_str("Unable to read CD; settings are probably wrong."),
			Self::Cdtoc(s) => write!(f, "{s}"),
			Self::Delete(ref s) => write!(f, "Unable to delete {s}."),
			Self::Device(ref s) => write!(f, "Invalid device path {s}."),
			Self::DeviceOpen(ref s) =>
				if let Some(s) = s { write!(f, "Unable to open connection with {s}.") }
				else {
					f.write_str("Unable to open connection with default optical drive.")
				},
			Self::DiscMode => f.write_str("Missing or unsupported disc type."),
			Self::FirstTrackNum => f.write_str("Unable to obtain the first track index."),
			Self::Killed => f.write_str("Operations aborted."),
			Self::Leadout => f.write_str("Unable to obtain leadout."),
			Self::NoTracks => f.write_str("No tracks were found."),
			Self::NumTracks => f.write_str("Unable to obtain the track total."),
			Self::Paranoia => f.write_str("Invalid paranoia level."),
			Self::ReadOffset => f.write_str("Invalid read offset."),
			Self::ReconfirmParanoia => f.write_str("Reconfirmation requires a paranoia level of at least 2."),
			Self::Refine => f.write_str("Invalid number of refine passes."),
			Self::RipOverflow(n) => write!(f, "Track #{n} cannot be ripped on this system."),
			Self::RipTracks => f.write_str("Invalid rip track or range."),
			Self::TrackFormat(n) => write!(f, "Unsupported track type ({n})."),
			Self::TrackLba(n) => write!(f, "Unable to obtain LBA ({n})."),
			Self::TrackNumber(n) => write!(f, "Invalid track number ({n})."),
			Self::Write(ref s) => write!(f, "Unable to write to {s}."),

			#[cfg(feature = "bin")]
			Self::Argue(a) => f.write_str(a.as_str()),
		}
	}
}
