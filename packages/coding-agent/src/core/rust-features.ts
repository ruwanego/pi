import { setNativeCodec } from "@earendil-works/pi-protocol";
import { findFiles, loadNative, protocolCodec } from "@ruwanego/pi-native";
import { setNativeFind } from "./tools/find.ts";

/**
 * Experimental: switches every Rust substitution from `@ruwanego/pi-native` on at once.
 *
 * - `find` tool search (RUST-002), instead of the fd binary
 * - pi-protocol CBOR and framing (RUST-001), instead of the TypeScript codec
 *
 * Loads the addon first, so it throws without changing anything when the addon has not been built for this
 * platform. `PI_RUST=1` calls this at startup.
 */
export function enableRustFeatures(): void {
	loadNative();
	setNativeFind(findFiles);
	setNativeCodec(protocolCodec);
}

/** Switches every Rust substitution back to the default implementation. */
export function disableRustFeatures(): void {
	setNativeFind(undefined);
	setNativeCodec(undefined);
}
