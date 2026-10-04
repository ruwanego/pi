import type { ResolvedCborOptions } from "./cbor/options.ts";

/** Incremental frame splitter supplied by a native codec. */
export interface NativeFrameDecoder {
	push(chunk: Uint8Array): Uint8Array[];
	end(): void;
}

/**
 * Alternative implementation of CBOR and framing, e.g. `protocolCodec` from `@ruwanego/pi-native`.
 * Arguments and limits are validated before a codec is called. Protocol failures must throw an `Error` whose
 * `code` is `PI_CBOR_ERROR` or `PI_FRAME_ERROR`; they are rethrown as `CborError` / `FrameError`.
 */
export interface NativeCodec {
	encodeCbor(value: unknown, limits: ResolvedCborOptions): Uint8Array;
	decodeCbor(bytes: Uint8Array, limits: ResolvedCborOptions): unknown;
	encodeFrame(payload: Uint8Array): Uint8Array;
	createFrameDecoder(maxFrameLength: number): NativeFrameDecoder;
}

let nativeCodec: NativeCodec | undefined;

/**
 * Experimental: routes CBOR and framing through `codec`, or back to the built-in TypeScript implementation when
 * `undefined`. Frame decoders keep the implementation that was active when they were constructed.
 */
export function setNativeCodec(codec: NativeCodec | undefined): void {
	nativeCodec = codec;
}

export function getNativeCodec(): NativeCodec | undefined {
	return nativeCodec;
}

export function nativeErrorMessage(error: unknown, code: "PI_CBOR_ERROR" | "PI_FRAME_ERROR"): string | undefined {
	return error instanceof Error && (error as Error & { code?: unknown }).code === code ? error.message : undefined;
}
