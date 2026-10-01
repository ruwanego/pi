import { loadNative, type NativeFrameDecoderHandle } from "./binding.ts";

export interface NativeCborLimits {
	maxByteLength: number;
	maxContainerLength: number;
	maxDepth: number;
}

/**
 * Rust implementation of the pi-protocol CBOR subset and framing, shaped to match pi-protocol's `NativeCodec`.
 * Callers validate arguments and limits first; protocol failures throw an `Error` whose `code` is
 * `PI_CBOR_ERROR` or `PI_FRAME_ERROR`.
 */
export const protocolCodec = {
	encodeCbor(value: unknown, limits: NativeCborLimits): Uint8Array {
		return loadNative().cborEncode(value, limits.maxByteLength, limits.maxContainerLength, limits.maxDepth);
	},
	decodeCbor(bytes: Uint8Array, limits: NativeCborLimits): unknown {
		return loadNative().cborDecode(bytes, limits.maxByteLength, limits.maxContainerLength, limits.maxDepth);
	},
	encodeFrame(payload: Uint8Array): Uint8Array {
		return loadNative().frameEncode(payload);
	},
	createFrameDecoder(maxFrameLength: number): NativeFrameDecoderHandle {
		return new (loadNative().NativeFrameDecoder)(maxFrameLength);
	},
};
