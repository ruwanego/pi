import { loadNative } from "./binding.ts";

/**
 * Rust fast path for pi-ai's `parseStreamingJson` (`crates/pi-json`), shaped to match pi-ai's
 * `NativeStreamingJsonParser` hook. Handles complete JSON and well-formed prefixes of an object or array, which is
 * what streamed tool-call arguments look like, and returns `undefined` for anything else so the TypeScript
 * implementation runs.
 */
export function parseStreamingJsonFast(partialJson: string): unknown {
	return loadNative().parseStreamingJsonFast(partialJson);
}
