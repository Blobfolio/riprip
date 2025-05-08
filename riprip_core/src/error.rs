/*!
# Rip Rip Hooray: Errors
*/

use cdtoc::TocError;
use fyi_ansi::{
	ansi,
	csi,
};
use fyi_msg::Msg;
use std::{
	error::Error,
	fmt,
};



#[cfg(feature = "bin")]
/// # Help Text.
const HELP: &str = concat!(r#"
    n__n_
   /  = =\     "#, csi!(199), "Rip Rip Hooray!", ansi!((cornflower_blue) " v", env!("CARGO_PKG_VERSION")), r#"
  /   ._Y_)    Accurate, incremental audio
 /      "\     CD ripping and recovery.
(_/  (_,  \
  \      ( \_,--""""--.
 ,-`.___,-` )-.______.'
 `-,'   `-_-'

USAGE:
    riprip [OPTIONS]

BASIC SETTINGS:
    -r, --rereads <[ABS],[MUL]>
                      Re-read sectors on subsequent passes until A) they have
                      been independently verified with AccurateRip or CUETools;
                      or B) the same allegedly-good values have been read at
                      least <ABS> times, and <MUL> times more often than any
                      contradictory "good" values. The value may omit the
                      number on either side of the comma to keep the default,
                      or be a single number to alter only the <ABS>.
                      [default: 2,2; range: 1..=20,1..=10]
    -p, --passes <NUM>
                      Automate re-ripping by executing up to <NUM> passes for
                      each track while any samples remain unread or
                      unconfirmed. [default: 1; max: 16]
    -t, --tracks <NUM(s),RNG>
                      Rip one or more specific tracks (rather than the whole
                      disc). Multiple tracks can be separated by commas (2,3),
                      specified as an inclusive range (2-3), and/or given their
                      own -t/--track (-t 2 -t 3). Track 0 can be used to rip
                      the HTOA, if any. [default: the whole disc]

WHEN ALL ELSE FAILS:
        --backwards   Reverse the sector read order when ripping a track,
                      starting at end, and ending at the start.
        --flip-flop   Alternate the sector read order between passes, forwards
                      then backwards then forwards then backwards… This has no
                      effect unless -p/--passes is at least two.
        --no-resume   Ignore any previous rip states, starting over from
                      scratch.
        --reset       Flip "likely" samples back to "maybe", keeping their
                      values, but resetting all counts to one. This is a softer
                      alternative to --no-resume, and will not affect tracks
                      confirmed by AccurateRip/CUETools.
        --strict      Consider C2 errors an all-or-nothing proposition for the
                      sector as a whole, marking all samples bad if any of them
                      are bad. This is most effective when applied consistently
                      from the initial rip and onward.

DRIVE SETTINGS:
    -c, --cache <NUM> Drive cache can interfere with re-read accuracy. If your
                      drive caches data, use this option to specify its buffer
                      size so Rip Rip can try to mitigate it. Values with an
                      M suffix are treated as MiB, otherwise KiB are assumed.
                      [default: auto or 0; max: 65,535]
    -d, --dev <PATH>  The device path for the optical drive containing the CD
                      of interest, like /dev/cdrom. [default: auto]
    -o, --offset <SAMPLES>
                      The AccurateRip, et al, sample read offset to apply to
                      data retrieved from the drive.
                      [default: auto or 0; range: ±5880]

UNUSUAL SETTINGS:
        --confidence <NUM>
                      Consider a track accurately ripped — i.e. stop working on
                      it — AccurateRip and/or CUETools matches are found with a
                      confidence of at least <NUM>. Raise this value if you
                      personally fucked up the database(s) with prior bad rips,
                      otherwise the default should be fine. Haha.
                      [default: 3; range: 1..=10]
        --sync        Confirm sector positioning with subchannel data (when
                      available) to make sure the drive is actually reading
                      from the right place, and ignore the data if not. This is
                      prone to false-positives — subchannel data is easily
                      corrupted — so only recommended when disc rot, rather
                      than wear-and-tear, is the sole cause of your woes.

MISCELLANEOUS:
    -h, --help        Print help information to STDOUT and exit.
    -v, --verbose     Print detailed sector quality information to STDOUT, so
                      it can e.g. be piped to a file for review, like:
                      riprip -v > issues.log
    -V, --version     Print version information to STDOUT and exit.
        --no-rip      Print the basic drive and disc information to STDERR and
                      exit (without ripping anything).
        --no-summary  Skip the drive and disc summary and jump straight to
                      ripping.
        --status      Print the status of the individual track rips (that you
                      presumably already started) to STDERR and exit. Note that
                      only the --no-summary, --confidence, and -r/--rereads
                      options have any meaning in this mode.

EARLY EXIT:
    If you don't have time to let a rip finish naturally, press "#, ansi!((dark_orange) "CTRL"), "+", ansi!((dark_orange) "C"), " to stop
    it early. Your progress will still be saved, there just won't be as much of
    it. Haha.
");



#[derive(Debug, Clone, Eq, PartialEq)]
/// # Errors.
pub enum RipRipError {
	/// # Invalid barcode.
	Barcode,

	/// # Bug!
	Bug(&'static str),

	/// # C2 296 Isn't Supported.
	C2Mode296,

	/// # Cache directory.
	Cache,

	/// # Cache Path.
	CachePath(String),

	/// # CDTOC passthrough.
	Cdtoc(TocError),

	/// # CD read error.
	CdRead,

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

	/// # Noop.
	Noop,

	/// # No Track.
	NoTrack(u8),

	/// # Unable to obtain the number of tracks.
	NumTracks,

	/// # Read Offset.
	ReadOffset,

	/// # Numbers can't be converted to the necessary types.
	RipOverflow,

	/// # State Corruption.
	StateCorrupt(u8),

	/// # State Save.
	StateSave(u8),

	/// # Subchannel Desync.
	SubchannelDesync,

	/// # Invalid/unsupported track format.
	TrackFormat(u8),

	/// # Invalid track LBA.
	TrackLba(u8),

	/// # Invalid track number.
	TrackNumber(u8),

	/// # Writing to disk.
	Write(String),

	#[cfg(feature = "bin")]
	/// # Invalid CLI arg.
	CliArg(String),

	#[cfg(feature = "bin")]
	/// # CLI Parsing failure.
	CliParse(&'static str),

	#[cfg(feature = "bin")]
	/// # Print Help (Not an Error).
	PrintHelp,

	#[cfg(feature = "bin")]
	/// # Print Version (Not an Error).
	PrintVersion,
}

impl Error for RipRipError {}

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
			Self::C2Mode296 => f.write_str("This drive does not seem to support 296-byte C2 blocks."),
			Self::Cache => f.write_str("Unable to establish a cache directory."),
			Self::CachePath(s) => write!(f, "Invalid cache path {s}."),
			Self::CdRead => f.write_str("Read error."),
			Self::CdReadUnsupported => f.write_str("Unable to read CD; settings are probably wrong."),
			Self::Cdtoc(s) => write!(f, "{s}"),
			Self::Device(s) => write!(f, "Invalid device path {s}."),
			Self::DeviceOpen(s) =>
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
			Self::Noop => f.write_str("There's nothing to do!"),
			Self::NoTrack(n) =>
				if *n == 0 { f.write_str("There is no HTOA on this disc.") }
				else { write!(f, "There is no track #{n} on this disc.") },
			Self::NumTracks => f.write_str("Unable to obtain the track total."),
			Self::ReadOffset => f.write_str("Invalid read offset."),
			Self::RipOverflow => f.write_str("The numbers are too big for this system architecture."),
			Self::StateCorrupt(n) => write!(f, "The state data for track #{n} is corrupt; rerip this track with --no-resume to start over."),
			Self::StateSave(n) => write!(f, "Unable to save the state data for track #{n}."),
			Self::SubchannelDesync => f.write_str("Subchannel desync."),
			Self::TrackFormat(n) => write!(f, "Unsupported track type ({n})."),
			Self::TrackLba(n) => write!(f, "Unable to obtain LBA ({n})."),
			Self::TrackNumber(n) => write!(f, "Invalid track number ({n})."),
			Self::Write(s) => write!(f, "Unable to write to {s}."),

			#[cfg(feature = "bin")]
			Self::CliArg(s) => write!(f, "Invalid CLI option: {s}"),

			#[cfg(feature = "bin")]
			Self::CliParse(s) => write!(f, "Unable to parse {s}."),

			#[cfg(feature = "bin")]
			Self::PrintHelp => f.write_str(HELP),

			#[cfg(feature = "bin")]
			Self::PrintVersion => f.write_str(concat!("Rip Rip Hooray! v", env!("CARGO_PKG_VERSION"))),
		}
	}
}
