import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

export interface NativeFindOptions {
	pattern: string;
	searchPath: string;
	fullPath: boolean;
	requireGit: boolean;
	maxResults: number;
}

export interface NativeFindSearchHandle {
	run(): Promise<string[]>;
	cancel(): void;
}

export interface NativeGrepOptions {
	pattern: string;
	searchPath: string;
	glob?: string;
	ignoreCase: boolean;
	fixedStrings: boolean;
	maxMatches: number;
	context?: number;
}

export interface NativeGrepResult {
	matches: Array<{
		path?: string;
		lineNumber: number;
		line?: string;
		contextStart?: number;
		contextLines?: string[];
	}>;
	limitReached: boolean;
	stderr: string;
	errored: boolean;
}

export interface NativeGrepSearchHandle {
	run(): Promise<NativeGrepResult>;
	cancel(): void;
}

export interface NativeFrameDecoderHandle {
	push(chunk: Uint8Array): Uint8Array[];
	end(): void;
}

/** Functions exported by crates/pi-native. Keep in sync with the #[napi] exports. */
export interface NativeBinding {
	rustVersion(): string;
	cborEncode(value: unknown, maxByteLength: number, maxContainerLength: number, maxDepth: number): Uint8Array;
	cborDecode(bytes: Uint8Array, maxByteLength: number, maxContainerLength: number, maxDepth: number): unknown;
	frameEncode(payload: Uint8Array): Uint8Array;
	NativeFrameDecoder: new (maxFrameLength: number) => NativeFrameDecoderHandle;
	NativeFindSearch: new (options: NativeFindOptions) => NativeFindSearchHandle;
	NativeGrepSearch: new (options: NativeGrepOptions) => NativeGrepSearchHandle;
}

const cjsRequire = createRequire(import.meta.url);
let binding: NativeBinding | undefined;

/** Path of the prebuilt addon for the current platform, relative to this package. */
export function getNativeModulePath(): string {
	const packageDir = join(dirname(fileURLToPath(import.meta.url)), "..");
	return join(packageDir, "prebuilds", `${process.platform}-${process.arch}`, "pi-native.node");
}

/** Loads the addon on first use. Throws if it has not been built for this platform. */
export function loadNative(): NativeBinding {
	if (binding) return binding;
	const modulePath = getNativeModulePath();
	try {
		binding = cjsRequire(modulePath) as NativeBinding;
	} catch (error) {
		throw new Error(`Failed to load pi-native from ${modulePath}. Run "npm run build" in packages/native.`, {
			cause: error,
		});
	}
	return binding;
}
