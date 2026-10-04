import { readFile as fsReadFile, stat as fsStat } from "node:fs/promises";
import { createInterface } from "node:readline";
import type { AgentTool } from "@earendil-works/pi-agent-core";
import { spawn } from "child_process";
import path from "path";
import { type Static, Type } from "typebox";
import { ensureTool } from "../../utils/tools-manager.ts";
import type { ExtensionContext, ToolDefinition } from "../extensions/types.ts";
import { resolveToCwd } from "./path-utils.ts";
import { grepRenderers } from "./renderers/grep.ts";
import { wrapToolDefinition } from "./tool-definition-wrapper.ts";
import {
	DEFAULT_MAX_BYTES,
	formatSize,
	GREP_MAX_LINE_LENGTH,
	type TruncationResult,
	truncateHead,
	truncateLine,
} from "./truncate.ts";

const grepSchema = Type.Object({
	pattern: Type.String({ description: "Search pattern (regex or literal string)" }),
	path: Type.Optional(Type.String({ description: "Directory or file to search (default: current directory)" })),
	glob: Type.Optional(Type.String({ description: "Filter files by glob pattern, e.g. '*.ts' or '**/*.spec.ts'" })),
	ignoreCase: Type.Optional(Type.Boolean({ description: "Case-insensitive search (default: false)" })),
	literal: Type.Optional(
		Type.Boolean({ description: "Treat pattern as literal string instead of regex (default: false)" }),
	),
	context: Type.Optional(
		Type.Number({ description: "Number of lines to show before and after each match (default: 0)" }),
	),
	limit: Type.Optional(Type.Number({ description: "Maximum number of matches to return (default: 100)" })),
});

export const grepToolSystemPromptContribution = {
	snippet: "Search file contents for patterns (respects .gitignore)",
	guidelines: [],
} as const;

export type GrepToolInput = Static<typeof grepSchema>;
const DEFAULT_LIMIT = 100;

export interface GrepToolDetails {
	truncation?: TruncationResult;
	matchLimitReached?: number;
	linesTruncated?: boolean;
}

/**
 * Pluggable operations for the grep tool.
 * Override these to delegate search to remote systems (for example SSH).
 */
export interface GrepOperations {
	/** Check if path is a directory. Throws if path does not exist. */
	isDirectory: (absolutePath: string) => Promise<boolean> | boolean;
	/** Read file contents for context lines */
	readFile: (absolutePath: string) => Promise<string> | string;
}

const defaultGrepOperations: GrepOperations = {
	isDirectory: async (p) => (await fsStat(p)).isDirectory(),
	readFile: (p) => fsReadFile(p, "utf-8"),
};

/** Arguments of one ripgrep invocation, as the grep tool would pass them on the command line. */
export interface NativeGrepRequest {
	pattern: string;
	/** Absolute file or directory to search. */
	searchPath: string;
	/** `--glob`. */
	glob?: string;
	/** `--ignore-case`. */
	ignoreCase: boolean;
	/** `--fixed-strings`. */
	fixedStrings: boolean;
	/** The tool stops ripgrep after this many matches. */
	maxMatches: number;
	/** Lines of context the tool shows around each match. */
	context: number;
}

/** ripgrep's output for one search: its `match` messages, stderr text, and whether it would exit with status 2. */
export interface NativeGrepResult {
	/**
	 * `path.text`, `line_number` and `lines.text`; `path` and `line` are absent when ripgrep reports bytes. With
	 * `context > 0`, `contextLines` are the lines `contextStart..` that the tool shows for the match, read the way
	 * the tool reads files; absent if the file could not be read.
	 */
	matches: Array<{
		path?: string;
		lineNumber: number;
		line?: string;
		contextStart?: number;
		contextLines?: string[];
	}>;
	/** Whether the search stopped at `maxMatches`. */
	limitReached: boolean;
	stderr: string;
	errored: boolean;
}

/**
 * Alternative to spawning ripgrep, e.g. `grepFiles` from `@ruwanego/pi-native`. Failures that make ripgrep exit
 * before searching must reject with the text ripgrep would print to stderr.
 */
export type NativeGrep = (request: NativeGrepRequest, signal?: AbortSignal) => Promise<NativeGrepResult>;

let nativeGrep: NativeGrep | undefined;

/**
 * Experimental: runs the default grep search through `grep` instead of the ripgrep binary, or back through ripgrep
 * when `undefined`. Custom `GrepOperations` still provide file access. ripgrep is still used while
 * `RIPGREP_CONFIG_PATH` is set, because the native search does not read ripgrep configuration files.
 */
export function setNativeGrep(grep: NativeGrep | undefined): void {
	nativeGrep = grep;
}

export interface GrepToolOptions {
	/** Custom operations for grep. Default: local filesystem plus ripgrep */
	operations?: GrepOperations;
}

export function createGrepToolDefinition(
	cwd: string,
	options?: GrepToolOptions,
): ToolDefinition<typeof grepSchema, GrepToolDetails | undefined> {
	const customOps = options?.operations;
	return {
		name: "grep",
		label: "grep",
		description: `Search file contents for a pattern. Returns matching lines with file paths and line numbers. Respects .gitignore. Output is truncated to ${DEFAULT_LIMIT} matches or ${DEFAULT_MAX_BYTES / 1024}KB (whichever is hit first). Long lines are truncated to ${GREP_MAX_LINE_LENGTH} chars.`,
		promptSnippet: grepToolSystemPromptContribution.snippet,
		parameters: grepSchema,
		async execute(
			_toolCallId,
			{
				pattern,
				path: searchDir,
				glob,
				ignoreCase,
				literal,
				context,
				limit,
			}: {
				pattern: string;
				path?: string;
				glob?: string;
				ignoreCase?: boolean;
				literal?: boolean;
				context?: number;
				limit?: number;
			},
			signal?: AbortSignal,
			_onUpdate?,
			ctx?: ExtensionContext,
		) {
			return new Promise((resolve, reject) => {
				if (signal?.aborted) {
					reject(new Error("Operation aborted"));
					return;
				}
				let settled = false;
				const settle = (fn: () => void) => {
					if (!settled) {
						settled = true;
						fn();
					}
				};

				(async () => {
					try {
						// Default implementation uses ripgrep, or the native replacement when one is installed. ripgrep
						// reads a configuration file when RIPGREP_CONFIG_PATH is non-empty; the native search does not, so
						// keep ripgrep then.
						const activeNativeGrep = process.env.RIPGREP_CONFIG_PATH ? undefined : nativeGrep;
						const rgPath = activeNativeGrep ? undefined : await ensureTool("rg");
						if (!activeNativeGrep && !rgPath) {
							settle(() => reject(new Error("ripgrep (rg) is not available and could not be downloaded")));
							return;
						}

						const searchPath = resolveToCwd(searchDir || ".", ctx?.cwd || cwd);
						const ops = customOps ?? defaultGrepOperations;
						let isDirectory: boolean;
						try {
							isDirectory = await ops.isDirectory(searchPath);
						} catch {
							settle(() => reject(new Error(`Path not found: ${searchPath}`)));
							return;
						}

						const contextValue = context && context > 0 ? context : 0;
						const effectiveLimit = Math.max(1, limit ?? DEFAULT_LIMIT);
						const formatPath = (filePath: string): string => {
							if (isDirectory) {
								const relative = path.relative(searchPath, filePath);
								if (relative && !relative.startsWith("..")) {
									return relative.replace(/\\/g, "/");
								}
							}
							return path.basename(filePath);
						};

						const fileCache = new Map<string, string[]>();
						const getFileLines = async (filePath: string): Promise<string[]> => {
							let lines = fileCache.get(filePath);
							if (!lines) {
								try {
									const content = await ops.readFile(filePath);
									lines = content.replace(/\r\n/g, "\n").replace(/\r/g, "\n").split("\n");
								} catch {
									lines = [];
								}
								fileCache.set(filePath, lines);
							}
							return lines;
						};

						let matchCount = 0;
						let matchLimitReached = false;
						let linesTruncated = false;
						const outputLines: string[] = [];
						const matches: Array<{
							filePath: string;
							lineNumber: number;
							lineText?: string;
							context?: { start: number; lines: string[] };
						}> = [];
						// Records one ripgrep match message. Returns true once the match limit is reached.
						const recordMatch = (
							filePath: unknown,
							lineNumber: unknown,
							lineText: string | undefined,
							context?: { start: number; lines: string[] },
						) => {
							matchCount++;
							if (typeof filePath === "string" && filePath && typeof lineNumber === "number")
								matches.push({ filePath, lineNumber, lineText, context });
							if (matchCount >= effectiveLimit) matchLimitReached = true;
							return matchLimitReached;
						};

						const formatBlock = async (
							filePath: string,
							lineNumber: number,
							context?: { start: number; lines: string[] },
						): Promise<string[]> => {
							const relativePath = formatPath(filePath);
							// The native search provides the lines it read; otherwise read the file here.
							const lines = context ? [] : await getFileLines(filePath);
							if (!context && !lines.length) return [`${relativePath}:${lineNumber}: (unable to read file)`];
							const block: string[] = [];
							const start = context
								? context.start
								: contextValue > 0
									? Math.max(1, lineNumber - contextValue)
									: lineNumber;
							const end = context
								? context.start + context.lines.length - 1
								: contextValue > 0
									? Math.min(lines.length, lineNumber + contextValue)
									: lineNumber;
							for (let current = start; current <= end; current++) {
								const lineText = (context ? context.lines[current - start] : lines[current - 1]) ?? "";
								const sanitized = lineText.replace(/\r/g, "");
								const isMatchLine = current === lineNumber;
								// Truncate long lines so grep output stays compact.
								const { text: truncatedText, wasTruncated } = truncateLine(sanitized);
								if (wasTruncated) linesTruncated = true;
								if (isMatchLine) block.push(`${relativePath}:${current}: ${truncatedText}`);
								else block.push(`${relativePath}-${current}- ${truncatedText}`);
							}
							return block;
						};

						// Builds the tool result once ripgrep has exited (or was stopped at the match limit).
						const finish = async (code: number | null, stderr: string, killedDueToLimit: boolean) => {
							if (!killedDueToLimit && code !== 0 && code !== 1) {
								const errorMsg = stderr.trim() || `ripgrep exited with code ${code}`;
								settle(() => reject(new Error(errorMsg)));
								return;
							}
							if (matchCount === 0) {
								settle(() =>
									resolve({ content: [{ type: "text", text: "No matches found" }], details: undefined }),
								);
								return;
							}

							// Format matches after the search finishes so custom readFile() backends can be async.
							for (const match of matches) {
								if (contextValue === 0 && match.lineText !== undefined) {
									const relativePath = formatPath(match.filePath);
									const sanitized = match.lineText
										.replace(/\r\n/g, "\n")
										.replace(/\r/g, "")
										.replace(/\n$/, "");
									const { text: truncatedText, wasTruncated } = truncateLine(sanitized);
									if (wasTruncated) linesTruncated = true;
									outputLines.push(`${relativePath}:${match.lineNumber}: ${truncatedText}`);
								} else {
									const block = await formatBlock(match.filePath, match.lineNumber, match.context);
									outputLines.push(...block);
								}
							}

							const rawOutput = outputLines.join("\n");
							// Apply byte truncation. There is no line limit here because the match limit already capped rows.
							const truncation = truncateHead(rawOutput, { maxLines: Number.MAX_SAFE_INTEGER });
							let output = truncation.content;
							const details: GrepToolDetails = {};
							// Build actionable notices for truncation and match limits.
							const notices: string[] = [];
							if (matchLimitReached) {
								notices.push(
									`${effectiveLimit} matches limit reached. Use limit=${effectiveLimit * 2} for more, or refine pattern`,
								);
								details.matchLimitReached = effectiveLimit;
							}
							if (truncation.truncated) {
								notices.push(`${formatSize(DEFAULT_MAX_BYTES)} limit reached`);
								details.truncation = truncation;
							}
							if (linesTruncated) {
								notices.push(
									`Some lines truncated to ${GREP_MAX_LINE_LENGTH} chars. Use read tool to see full lines`,
								);
								details.linesTruncated = true;
							}
							if (notices.length > 0) output += `\n\n[${notices.join(". ")}]`;
							settle(() =>
								resolve({
									content: [{ type: "text", text: output }],
									details: Object.keys(details).length > 0 ? details : undefined,
								}),
							);
						};

						if (activeNativeGrep) {
							const result = await activeNativeGrep(
								{
									pattern,
									searchPath,
									glob: glob || undefined,
									ignoreCase: Boolean(ignoreCase),
									fixedStrings: Boolean(literal),
									maxMatches: effectiveLimit,
									// Context lines read by the native search stand in for local reads only.
									context: customOps ? 0 : contextValue,
								},
								signal,
							);
							if (signal?.aborted) {
								settle(() => reject(new Error("Operation aborted")));
								return;
							}
							for (const match of result.matches) {
								const context =
									match.contextStart !== undefined && match.contextLines
										? { start: match.contextStart, lines: match.contextLines }
										: undefined;
								if (recordMatch(match.path, match.lineNumber, match.line, context)) break;
							}
							const code = result.errored ? 2 : matchCount > 0 ? 0 : 1;
							await finish(code, result.stderr, result.limitReached);
							return;
						}

						if (!rgPath) return;
						const args: string[] = ["--json", "--line-number", "--color=never", "--hidden"];
						if (ignoreCase) args.push("--ignore-case");
						if (literal) args.push("--fixed-strings");
						if (glob) args.push("--glob", glob);
						args.push("--", pattern, searchPath);

						const child = spawn(rgPath, args, { stdio: ["ignore", "pipe", "pipe"] });
						const rl = createInterface({ input: child.stdout });
						let stderr = "";
						let aborted = false;
						let killedDueToLimit = false;

						const cleanup = () => {
							rl.close();
							signal?.removeEventListener("abort", onAbort);
						};
						const stopChild = (dueToLimit = false) => {
							if (!child.killed) {
								killedDueToLimit = dueToLimit;
								child.kill();
							}
						};
						const onAbort = () => {
							aborted = true;
							stopChild();
						};
						signal?.addEventListener("abort", onAbort, { once: true });
						child.stderr?.on("data", (chunk) => {
							stderr += chunk.toString();
						});

						// Collect matches during streaming, then format them after rg exits.
						rl.on("line", (line) => {
							if (!line.trim() || matchCount >= effectiveLimit) return;
							let event: any;
							try {
								event = JSON.parse(line);
							} catch {
								return;
							}
							if (event.type === "match") {
								if (recordMatch(event.data?.path?.text, event.data?.line_number, event.data?.lines?.text))
									stopChild(true);
							}
						});

						child.on("error", (error) => {
							cleanup();
							settle(() => reject(new Error(`Failed to run ripgrep: ${error.message}`)));
						});
						child.on("close", async (code) => {
							cleanup();
							if (aborted) {
								settle(() => reject(new Error("Operation aborted")));
								return;
							}
							await finish(code, stderr, killedDueToLimit);
						});
					} catch (err) {
						settle(() => reject(err as Error));
					}
				})();
			});
		},
		...grepRenderers,
	};
}

export function createGrepTool(cwd: string, options?: GrepToolOptions): AgentTool<typeof grepSchema> {
	return wrapToolDefinition(createGrepToolDefinition(cwd, options));
}
