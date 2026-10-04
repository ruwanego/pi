import { loadNative } from "./binding.ts";

export {
	getNativeModulePath,
	loadNative,
	type NativeBinding,
	type NativeFindOptions,
	type NativeFindSearchHandle,
	type NativeFrameDecoderHandle,
	type NativeGrepOptions,
	type NativeGrepResult,
	type NativeGrepSearchHandle,
} from "./binding.ts";
export { findFiles } from "./find.ts";
export { grepFiles } from "./grep.ts";
export { parseStreamingJsonFast } from "./json.ts";
export { type NativeCborLimits, protocolCodec } from "./protocol.ts";

/** Version of the Rust crate behind the bridge. */
export function rustVersion(): string {
	return loadNative().rustVersion();
}

const native = { rustVersion };
export default native;
