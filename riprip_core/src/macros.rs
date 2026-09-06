/*!
# Rip Rip Hooray: Macros.
*/



#[macro_export]
/// # Helper: Log Crate Wrapper.
///
/// This macro helps ensure logs are handled consistently across the apps.
macro_rules! log {
	// Common logging (internal).
	($level:ident $($log:tt)+) => (
		$crate::LogLog::log(
			$crate::LogLevel::$level,
			::std::format_args!($($log)+),
			None,
		);
	);

	// Level-specific helpers.
	(@error $($log:tt)+) => ( $crate::macros::log!(Error $($log)+) );
	(@warn  $($log:tt)+) => ( $crate::macros::log!(Warn  $($log)+) );
	(@info  $($log:tt)+) => ( $crate::macros::log!(Info  $($log)+) );
	(@debug $($log:tt)+) => ( $crate::macros::log!(Debug $($log)+) );

	// Trace includes location details.
	(@trace $($log:tt)+) => (
		$crate::LogLog::log(
			$crate::LogLevel::Trace,
			::std::format_args!($($log)+),
			Some((::std::file!(), ::std::line!())),
		);
	);
}
pub use log;
