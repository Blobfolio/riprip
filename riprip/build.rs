/*!
# Rip Rip: Build
*/

use argyle::KeyWordsBuilder;
use std::path::PathBuf;



/// # Set Up CLI Arguments.
fn main() {
	println!("cargo:rerun-if-env-changed=CARGO_PKG_VERSION");

	let mut builder = KeyWordsBuilder::default();
	builder.push_keys([
		"--backward", "--backwards",
		"--flip-flop",
		"-h", "--help",
		"--no-resume",
		"--no-rip",
		"--no-summary",
		"--reset",
		"--status",
		"--strict",
		"--sync",
		"-v", "--verbose",
		"-V", "--version",
	]);
	builder.push_keys_with_values([
		"-c", "--cache",
		"-d", "--dev",
		"--confidence",
		"-o", "--offset",
		"-p", "--pass", "--passes",
		"-r", "--reread", "--rereads",
		"-t", "--track", "--tracks",
	]);
	builder.save(out_path("argyle.rs"));
}

/// # Output Path.
///
/// Append the sub-path to OUT_DIR and return it.
fn out_path(stub: &str) -> PathBuf {
	std::fs::canonicalize(std::env::var("OUT_DIR").expect("Missing OUT_DIR."))
		.expect("Missing OUT_DIR.")
		.join(stub)
}
