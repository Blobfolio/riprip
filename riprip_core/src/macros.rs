/*!
# Rip Rip Hooray: Macros.
*/



#[macro_export]
/// # Helper: Log Crate Wrapper.
///
/// This macro helps ensure logs are handled consistently across the apps.
macro_rules! log {
	// Argument muncher.
	(@munch) => ( "" );
	(@munch $next:expr, $( $rest:expr, )* ) => (
		::std::concat!("\n  {}: {:?}", $crate::macros::log!(@munch $($rest,)*) )
	);

	// Common logging (internal).
	($level:ident $($log:tt)+) => (
		$crate::LogLog::log(
			$crate::LogLevel::$level,
			::std::format_args!($($log)+),
			None,
			None,
		);
	);

	// Level-specific helpers.
	(@error $($log:tt)+) => ( $crate::macros::log!(Error $($log)+) );
	(@warn  $($log:tt)+) => ( $crate::macros::log!(Warn  $($log)+) );
	(@info  $($log:tt)+) => ( $crate::macros::log!(Info  $($log)+) );
	(@debug $($log:tt)+) => ( $crate::macros::log!(Debug $($log)+) );

	// Trace with location and arguments.
	(@trace [ $( $args:expr ),+ $(,)? ] $($log:tt)+) => (
		$crate::LogLog::log(
			$crate::LogLevel::Trace,
			::std::format_args!($($log)+),
			Some(::std::format_args!(
				$crate::macros::log!(@munch $($args,)+),
				$( ::std::stringify!($args), $args, )+
			)),
			Some((::std::file!(), ::std::line!())),
		);
	);

	// Trace with location.
	(@trace $($log:tt)+) => (
		$crate::LogLog::log(
			$crate::LogLevel::Trace,
			::std::format_args!($($log)+),
			None,
			Some((::std::file!(), ::std::line!())),
		);
	);
}
pub use log;
