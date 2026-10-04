//! Native bridge for pi. Exports are loaded by `@earendil-works/pi-native`.

mod find;
mod protocol;

use napi_derive::napi;

/// Version of the native crate, used to verify the bridge is loaded.
#[napi]
pub fn rust_version() -> String {
	env!("CARGO_PKG_VERSION").to_string()
}

#[cfg(test)]
mod tests {
	use super::*;

	#[test]
	fn rust_version_matches_crate_version() {
		assert_eq!(rust_version(), env!("CARGO_PKG_VERSION"));
		assert!(!rust_version().is_empty());
	}
}
