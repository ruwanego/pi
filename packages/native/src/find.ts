import { loadNative } from "./binding.ts";

const MAX_USIZE = 2n ** 64n - 1n;

/**
 * Rust implementation of the `fd` search behind the coding-agent `find` tool (`crates/pi-find`), shaped to match
 * the coding-agent `NativeFind` hook. It takes the arguments the tool would pass to fd and resolves to fd's stdout
 * lines. Failures reject with the text fd would print to stderr, including fd's argument errors for `maxResults`.
 */
export async function findFiles(
	request: { pattern: string; searchPath: string; fullPath: boolean; requireGit: boolean; maxResults: number },
	signal?: AbortSignal,
): Promise<string[]> {
	// The tool passes String(limit) to fd, so reject what fd's argument parser rejects, with its message.
	const maxResults = String(request.maxResults);
	if (maxResults.startsWith("-")) {
		// fd's argument parser reads a value that starts with one of fd's short flags (-0, -1, -I; the only ones a
		// number's string form can start with) as that flag, leaving --max-results without a value.
		if (/^-[01I]/.test(maxResults)) {
			throw new Error(
				"error: a value is required for '--max-results <count>' but none was supplied\n\nFor more information, try '--help'.",
			);
		}
		throw new Error(
			`error: unexpected argument '${maxResults}' found\n\n  tip: to pass '${maxResults}' as a value, use '-- ${maxResults}'\n\nUsage: fd [OPTIONS] [pattern] [path]...\n\nFor more information, try '--help'.`,
		);
	}
	const reason = !/^\d+$/.test(maxResults)
		? "invalid digit found in string"
		: BigInt(maxResults) > MAX_USIZE
			? "number too large to fit in target type"
			: undefined;
	if (reason) {
		throw new Error(
			`error: invalid value '${maxResults}' for '--max-results <count>': ${reason}\n\nFor more information, try '--help'.`,
		);
	}

	const search = new (loadNative().NativeFindSearch)({ ...request, maxResults: Number(maxResults) });
	const onAbort = () => search.cancel();
	signal?.addEventListener("abort", onAbort, { once: true });
	try {
		return await search.run();
	} finally {
		signal?.removeEventListener("abort", onAbort);
	}
}
