import { mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { findFiles } from "@ruwanego/pi-native";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { createFindToolDefinition, type FindToolInput, setNativeFind } from "../src/core/tools/find.ts";
import { getToolPath } from "../src/utils/tools-manager.ts";

/**
 * Differential test for RUST-002: the find tool must produce the same output whether the search runs through the
 * fd binary or through `findFiles` from pi-native (crates/pi-find).
 */

type Outcome = { ok: true; text: string; details: unknown } | { ok: false; error: string };

async function runFind(root: string, params: FindToolInput, signal?: AbortSignal): Promise<Outcome> {
	const def = createFindToolDefinition(root);
	try {
		const result = await def.execute("call", params, signal, undefined, {} as Parameters<typeof def.execute>[4]);
		const text = result.content.map((block) => (block.type === "text" ? block.text : "")).join("");
		return { ok: true, text, details: result.details };
	} catch (error) {
		return { ok: false, error: error instanceof Error ? error.message : String(error) };
	}
}

/**
 * fd and the native search both sort only when the search finishes within 100 ms with at most 1000 results, and
 * otherwise print results in the order they are found, which varies between runs. Sort result lines so the
 * comparison is order-independent; crates/pi-find tests the ordering rule. Fixtures stay far below the output byte
 * limit, so ordering cannot change which lines are kept.
 */
function sortResultLines(outcome: Outcome): Outcome {
	if (!outcome.ok) return outcome;
	const [body, ...notices] = outcome.text.split("\n\n");
	return { ...outcome, text: [body.split("\n").sort().join("\n"), ...notices].join("\n\n") };
}

async function runBoth(root: string, params: FindToolInput): Promise<{ fd: Outcome; native: Outcome }> {
	setNativeFind(undefined);
	const fd = await runFind(root, params);
	setNativeFind(findFiles);
	let native: Outcome;
	try {
		native = await runFind(root, params);
	} finally {
		setNativeFind(undefined);
	}
	return { fd: sortResultLines(fd), native: sortResultLines(native) };
}

function write(root: string, relative: string, contents = ""): void {
	const path = join(root, relative);
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, contents);
}

const fdPath = getToolPath("fd");

describe.skipIf(!fdPath)("find: native search matches fd", () => {
	const roots: string[] = [];
	const makeRoot = (name: string) => {
		const root = mkdtempSync(join(tmpdir(), `pi-find-parity-${name}-`));
		roots.push(root);
		return root;
	};

	afterAll(() => {
		for (const root of roots) rmSync(root, { recursive: true, force: true });
	});

	afterEach(() => {
		setNativeFind(undefined);
	});

	describe("outside a git repository", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("plain");
			for (const file of [
				"README.md",
				"readme.txt",
				"Makefile",
				"notes.TXT",
				"a.ts",
				"b.ts",
				"x.d.ts",
				"src/index.ts",
				"src/util.spec.ts",
				"src/foo/bar/example.spec.ts",
				"src/foo/bar/example.ts",
				"some/parent/child/file.ext",
				"some/parent/child/test.spec.ts",
				"lib/c.spec.ts",
				".env",
				".secret/hidden.txt",
				".config/app/settings.json",
				"with space/file name.txt",
				"unicodé/ñame.md",
				"[brackets]/x.ts",
				"{braces}/y.ts",
				"deep/1/2/3/4/5/6/7/8/leaf.txt",
				// Ignore files apply outside git repos because the tool passes --no-require-git.
				".gitignore",
				"build/out.js",
				"debug.log",
				"logs/keep.log",
				"a/.gitignore",
				"a/ignored.txt",
				"a/kept.txt",
				"a/deep/.gitignore",
				"a/deep/ignored.txt",
				"a/deep/secret.txt",
				"a/deep/kept.txt",
				"b/ignored.txt",
				"b/kept.txt",
				"c/.ignore",
				"c/dropped.md",
				"c/kept.md",
				"d/.fdignore",
				"d/dropped.json",
				"d/kept.json",
			]) {
				write(root, file);
			}
			writeFileSync(join(root, ".gitignore"), "build/\n*.log\n!logs/keep.log\n");
			writeFileSync(join(root, "a", ".gitignore"), "ignored.txt\n");
			writeFileSync(join(root, "a", "deep", ".gitignore"), "secret.txt\n");
			writeFileSync(join(root, "c", ".ignore"), "dropped.md\n");
			writeFileSync(join(root, "d", ".fdignore"), "dropped.json\n");
			mkdirSync(join(root, "empty-dir"));
			if (process.platform !== "win32") {
				symlinkSync(join(root, "a.ts"), join(root, "link-to-file.ts"));
				symlinkSync(join(root, "src"), join(root, "link-to-dir"));
				symlinkSync(join(root, "missing-target"), join(root, "broken-link.ts"));
			}
		});

		const patterns = [
			"*",
			"**",
			"",
			"*.ts",
			"**/*.ts",
			"*.spec.ts",
			"src/**/*.spec.ts",
			"src/*",
			"src/**",
			"some/parent/child/**",
			"**/child/*",
			"*/",
			"readme*",
			"README*",
			"*.txt",
			"*.TXT",
			"Makefile",
			".secret",
			"**/.secret/*",
			".*",
			"*.{ts,md}",
			"[!a]*.ts",
			"[ab].ts",
			"?.ts",
			"\\[brackets\\]",
			"file name.txt",
			"ñame.md",
			"*.log",
			"leaf.txt",
			"deep/**/leaf.txt",
			"*link*",
			"empty-dir",
			"--help",
			"-x",
			"no-match-anywhere",
			"[",
			"{a,b",
			"src/[",
		];

		it.each(patterns)("pattern %j", async (pattern) => {
			const { fd, native } = await runBoth(root, { pattern });
			expect(native).toEqual(fd);
		});

		it.each(["src", "a", "a/deep", ".", "./src/foo", "with space"])("search path %j", async (path) => {
			const { fd, native } = await runBoth(root, { pattern: "*", path });
			expect(native).toEqual(fd);
		});

		it.each([
			{ pattern: "*.ts", path: "missing" },
			{ pattern: "*", path: "README.md" },
		])("invalid search path $path", async (params) => {
			const { fd, native } = await runBoth(root, params);
			expect(fd.ok).toBe(false);
			expect(native).toEqual(fd);
		});

		it.each([100, 0, 2.5, -1, -2, -10, -0.5, -0, Number.NaN, Number.NEGATIVE_INFINITY, 1e21, 2 ** 64, 2 ** 53])(
			"limit %d",
			async (limit) => {
				const { fd, native } = await runBoth(root, { pattern: "*.ts", limit });
				expect(native).toEqual(fd);
			},
		);

		it.each([1, 3, 10])("limit %d stops after that many results", async (limit) => {
			// Which matches fill the limit depends on walk order in both implementations; compare everything else.
			const { fd, native } = await runBoth(root, { pattern: "*", limit });
			const all = await runFind(root, { pattern: "*", limit: 1000 });
			if (!fd.ok || !native.ok || !all.ok) throw new Error("expected results");
			const [fdLines, fdNotice] = fd.text.split("\n\n");
			const [nativeLines, nativeNotice] = native.text.split("\n\n");
			expect(nativeNotice).toBe(fdNotice);
			expect(native.details).toEqual(fd.details);
			expect(nativeLines.split("\n")).toHaveLength(fdLines.split("\n").length);
			const allLines = new Set(all.text.split("\n"));
			for (const line of nativeLines.split("\n")) expect(allLines).toContain(line);
		});
	});

	describe("inside a git repository", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("repo");
			mkdirSync(join(root, ".git", "info"), { recursive: true });
			writeFileSync(join(root, ".git", "HEAD"), "ref: refs/heads/main\n");
			writeFileSync(join(root, ".git", "info", "exclude"), "excluded.txt\n");
			writeFileSync(join(root, ".gitignore"), "*.tmp\nnested-ignored.txt\n");
			for (const file of ["kept.txt", "excluded.txt", "scratch.tmp", "src/main.ts", "src/cache.tmp"]) {
				write(root, file);
			}
			// Nested repository: the parent's .gitignore must not apply inside it (#5960).
			mkdirSync(join(root, "nested", ".git"), { recursive: true });
			writeFileSync(join(root, "nested", ".git", "HEAD"), "ref: refs/heads/main\n");
			write(root, "nested/nested-ignored.txt");
			write(root, "nested/scratch.tmp");
			write(root, "nested/code.ts");
		});

		it.each(["*", "*.txt", "*.tmp", "**/*.ts", "nested/*", "HEAD", ".git", "**/.git/**"])(
			"pattern %j",
			async (pattern) => {
				const { fd, native } = await runBoth(root, { pattern });
				expect(native).toEqual(fd);
			},
		);

		it.each(["src", "nested"])("search path %j", async (path) => {
			const { fd, native } = await runBoth(root, { pattern: "*", path });
			expect(native).toEqual(fd);
		});
	});

	describe("larger tree", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("large");
			writeFileSync(join(root, ".gitignore"), "*.skip\n");
			for (let a = 0; a < 12; a++) {
				for (let b = 0; b < 8; b++) {
					for (let c = 0; c < 8; c++) {
						const ext = ["ts", "js", "md", "skip"][(a + b + c) % 4];
						write(root, `pkg${a}/mod${b}/file${c}.${ext}`);
					}
				}
			}
		});

		it.each(["*.ts", "pkg1*/**/*.md", "mod3", "*.skip"])("pattern %j", async (pattern) => {
			const { fd, native } = await runBoth(root, { pattern });
			expect(native).toEqual(fd);
		});

		it("unsorted fd output has the same results", async () => {
			// More than 1000 results always makes fd stream in walk order instead of sorting.
			const { fd, native } = await runBoth(root, { pattern: "*", limit: 5000 });
			expect(native).toEqual(fd);
		});
	});

	describe("abort", () => {
		it("rejects when the signal is already aborted", async () => {
			setNativeFind(findFiles);
			const controller = new AbortController();
			controller.abort();
			const outcome = await runFind(tmpdir(), { pattern: "*" }, controller.signal);
			expect(outcome).toEqual({ ok: false, error: "Operation aborted" });
		});

		it("rejects when aborted during the search", async () => {
			const root = makeRoot("abort");
			for (let i = 0; i < 2000; i++) write(root, `d${i % 50}/f${i}.txt`);
			setNativeFind(findFiles);
			const controller = new AbortController();
			const pending = runFind(root, { pattern: "*" }, controller.signal);
			controller.abort();
			expect(await pending).toEqual({ ok: false, error: "Operation aborted" });
		});
	});
});
