import { loadNative, type NativeGrepOptions, type NativeGrepResult } from "./binding.ts";

/**
 * Rust implementation of the ripgrep search behind the coding-agent `grep` tool (`crates/pi-grep`), shaped to match
 * the coding-agent `NativeGrep` hook. It takes the arguments the tool would pass to ripgrep and resolves to ripgrep's
 * `match` messages, its stderr text and whether it would exit with an error. Failures that make ripgrep exit before
 * searching (a bad pattern or glob) reject with ripgrep's stderr text.
 */
export async function grepFiles(request: NativeGrepOptions, signal?: AbortSignal): Promise<NativeGrepResult> {
	const search = new (loadNative().NativeGrepSearch)(request);
	const onAbort = () => search.cancel();
	signal?.addEventListener("abort", onAbort, { once: true });
	try {
		return await search.run();
	} finally {
		signal?.removeEventListener("abort", onAbort);
	}
}
