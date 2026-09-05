/*!
# Rip Rip Hooray: Logger.

This module implements a really simple STDOUT logger for `log`.
*/

use std::{
	fmt::{
		self,
		Arguments,
	},
	sync::OnceLock,
};
use utc2k::FmtUtc2k;



/// # Logging Level.
static LOG_LEVEL: OnceLock<LogLevel> = OnceLock::new();



#[derive(Debug, Clone, Copy)]
/// # Logger.
///
/// This struct serves as a super-simple STDOUT logging interface for Rip Rip.
pub struct LogLog;

impl LogLog {
	#[inline(never)]
	/// # (Maybe) Log Something.
	///
	/// Log a message if the level is applicable, do nothing if not.
	pub fn log(
		level: LogLevel,
		args: Arguments<'_>,
		location: Option<(&'static str, u32)>,
	) {
		if let Some(v) = LOG_LEVEL.get().copied() && level <= v {
			println!(
				"{date} {level} {args}{location}",
				date=FmtUtc2k::now().to_rfc3339(),
				level=level.as_str(),
				location=LogLocation(location),
			);
		}
	}

	/// # Set Global Logging Level.
	///
	/// Enable logging at the defined verbosity level, returning the
	/// corresponding logging level, if any.
	pub fn init(verbosity: u8) -> Option<LogLevel> {
		if
			let Some(level) = LogLevel::from_verbosity(verbosity) &&
			LOG_LEVEL.set(level).is_ok()
		{
			Some(level)
		}
		else { None }
	}
}



/// # Helper: Log Level.
macro_rules! level {
	( $( $k:ident $v:literal $str:literal, )+ ) => (
		#[repr(u8)]
		#[derive(Debug, Clone, Copy, Eq, Ord, PartialEq, PartialOrd)]
		/// # Log Level.
		pub enum LogLevel {
			$(
				#[doc = concat!("# ", stringify!($k), ".")]
				$k = $v,
			)+
		}

		impl LogLevel {
			#[must_use]
			/// # As String.
			const fn as_str(self) -> &'static str {
				match self {
					$( Self::$k => $str, )+
				}
			}

			#[must_use]
			/// # From Verbosity Level.
			const fn from_verbosity(verbosity: u8) -> Option<Self> {
				match verbosity {
					0 => None,
					1 => Some(Self::Info),
					2 => Some(Self::Debug),
					_ => Some(Self::Trace),
				}
			}
		}
	);
}
level! {
	Error 1 "[error]",
	Warn  2 "[warn] ",
	Info  3 "[info] ",
	Debug 4 "[debug]",
	Trace 5 "[trace]",
}



/// # Maybe Print Line Details.
struct LogLocation(Option<(&'static str, u32)>);

impl fmt::Display for LogLocation {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		self.0.map_or(
			Ok(()),
			|(file, line)| write!(f, "\n  {file}:{line}")
		)
	}
}
