import { setNativeCodec } from "@earendil-works/pi-protocol";
import { findFiles, grepFiles, loadNative, protocolCodec } from "@ruwanego/pi-native";
import { setNativeFind } from "./tools/find.ts";
import { setNativeGrep } from "./tools/grep.ts";

/**
 * Experimental: switches every Rust substitution from `@ruwanego/pi-native` on at once.
 *
 * - `find` tool search (RUST-002), instead of the fd binary
 * - `grep` tool search (RUST-003), instead of the ripgrep binary
 * - pi-protocol CBOR and framing (RUST-001), instead of the TypeScript codec
 *
 * Loads the addon first, so it throws without changing anything when the addon has not been built for this
 * platform. `PI_RUST=1` calls this at startup.
 */
export function enableRustFeatures(): void {
	loadNative();
	setNativeFind(findFiles);
	setNativeGrep(grepFiles);
	setNativeCodec(protocolCodec);
}

/** Switches every Rust substitution back to the default implementation. */
export function disableRustFeatures(): void {
	setNativeFind(undefined);
	setNativeGrep(undefined);
	setNativeCodec(undefined);
}
