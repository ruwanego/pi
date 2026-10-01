import { loadNative } from "./binding.ts";

export { getNativeModulePath, loadNative, type NativeBinding, type NativeFrameDecoderHandle } from "./binding.ts";
export { type NativeCborLimits, protocolCodec } from "./protocol.ts";

/** Version of the Rust crate behind the bridge. */
export function rustVersion(): string {
	return loadNative().rustVersion();
}

const native = { rustVersion };
export default native;
