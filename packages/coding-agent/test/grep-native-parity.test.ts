import { chmodSync, mkdirSync, mkdtempSync, rmSync, symlinkSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { dirname, join } from "node:path";
import { grepFiles } from "@ruwanego/pi-native";
import { afterAll, afterEach, beforeAll, describe, expect, it } from "vitest";
import { createGrepToolDefinition, type GrepToolInput, setNativeGrep } from "../src/core/tools/grep.ts";
import { getToolPath } from "../src/utils/tools-manager.ts";

/**
 * Differential test for RUST-003: the grep tool must produce the same output whether the search runs through the
 * ripgrep binary or through `grepFiles` from pi-native (crates/pi-grep).
 */

type Outcome = { ok: true; text: string; details: unknown } | { ok: false; error: string };

async function runGrep(root: string, params: GrepToolInput, signal?: AbortSignal): Promise<Outcome> {
	const def = createGrepToolDefinition(root);
	try {
		const result = await def.execute("call", params, signal, undefined, {} as Parameters<typeof def.execute>[4]);
		const text = result.content.map((block) => (block.type === "text" ? block.text : "")).join("");
		return { ok: true, text, details: result.details };
	} catch (error) {
		return { ok: false, error: error instanceof Error ? error.message : String(error) };
	}
}

/**
 * ripgrep reports files in the order its threads finish them; the native search reports them in path order. Sort
 * output lines so the comparison is order-independent. Fixtures stay far below the output byte limit, so ordering
 * cannot change which lines are kept.
 */
function sortOutputLines(outcome: Outcome): Outcome {
	if (!outcome.ok) return outcome;
	const [body, ...notices] = outcome.text.split("\n\n[");
	return { ...outcome, text: [body.split("\n").sort().join("\n"), ...notices].join("\n\n[") };
}

async function runBoth(root: string, params: GrepToolInput): Promise<{ rg: Outcome; native: Outcome }> {
	setNativeGrep(undefined);
	const rg = await runGrep(root, params);
	setNativeGrep(grepFiles);
	try {
		return { rg: sortOutputLines(rg), native: sortOutputLines(await runGrep(root, params)) };
	} finally {
		setNativeGrep(undefined);
	}
}

function write(root: string, relative: string, contents: string | Buffer = ""): void {
	const path = join(root, relative);
	mkdirSync(dirname(path), { recursive: true });
	writeFileSync(path, contents);
}

function utf16le(text: string): Buffer {
	return Buffer.concat([Buffer.from([0xff, 0xfe]), Buffer.from(text, "utf16le")]);
}

const rgPath = getToolPath("rg");

describe.skipIf(!rgPath)("grep: native search matches ripgrep", () => {
	const roots: string[] = [];
	const makeRoot = (name: string) => {
		const root = mkdtempSync(join(tmpdir(), `pi-grep-parity-${name}-`));
		roots.push(root);
		return root;
	};

	afterAll(() => {
		for (const root of roots) {
			try {
				chmodSync(join(root, "locked"), 0o755);
			} catch {}
			rmSync(root, { recursive: true, force: true });
		}
	});

	afterEach(() => {
		setNativeGrep(undefined);
	});

	function fillTree(root: string): void {
		write(root, "src/index.ts", 'import { foo } from "./foo";\nexport const needle = foo(1);\n// TODO: needle\n');
		write(root, "src/foo.ts", "export function foo(x: number) {\n\treturn x + 1; // Needle\n}\n");
		write(root, "src/deep/a/b/c.spec.ts", "describe('needle', () => {});\n");
		write(root, "README.md", "# Title\n\nSome needle text.\nNEEDLE uppercase\n");
		write(root, "notes.txt", "no trailing newline needle");
		write(root, "crlf.txt", "first needle\r\nsecond\r\nthird needle\r\n");
		write(root, "long.txt", `${"x".repeat(600)} needle ${"y".repeat(600)}\nshort needle\n`);
		write(root, "unicode.txt", "héllo wörld needle\nnaïve ñame\n日本語 needle\n");
		write(
			root,
			"invalid-utf8.txt",
			Buffer.concat([Buffer.from("bad "), Buffer.from([0xff, 0xfe]), Buffer.from(" needle\nok needle\n")]),
		);
		write(
			root,
			"binary.dat",
			Buffer.concat([Buffer.from("needle at start\n"), Buffer.from([0, 1, 2]), Buffer.from("\nneedle\n")]),
		);
		// A NUL byte past the first read buffer: matches before it are reported, the rest of the file is skipped.
		write(
			root,
			"late-binary.dat",
			Buffer.concat([
				Buffer.from(`needle early\n${"filler\n".repeat(20000)}`),
				Buffer.from([0]),
				Buffer.from("\nneedle late\n"),
			]),
		);
		write(root, "utf16.txt", utf16le("utf16 line\nneedle in utf16\n"));
		write(root, "empty.txt", "");
		write(root, "special.txt", "a.b(c) [x] *star* $dollar ^caret\\back\nfoo.bar foo-bar\n");
		write(root, ".hidden/secret.txt", "hidden needle\n");
		write(root, ".env", "NEEDLE=1\n");
		write(root, ".gitignore", "*.log\nbuild/\n");
		write(root, "debug.log", "needle in log\n");
		write(root, "build/out.js", "needle in build\n");
		write(root, "c/.ignore", "dropped.md\n");
		write(root, "c/dropped.md", "needle dropped\n");
		write(root, "c/kept.md", "needle kept\n");
		write(root, "d/.rgignore", "skip.txt\n");
		write(root, "d/skip.txt", "needle skip\n");
		write(root, "d/keep.txt", "needle keep\n");
		write(root, "many.txt", Array.from({ length: 40 }, (_, i) => `line ${i} needle`).join("\n"));
		if (process.platform !== "win32") {
			symlinkSync(join(root, "README.md"), join(root, "link.md"));
			symlinkSync(join(root, "src"), join(root, "link-dir"));
		}
	}

	const cases: GrepToolInput[] = [
		{ pattern: "needle" },
		{ pattern: "needle", ignoreCase: true },
		{ pattern: "NEEDLE" },
		{ pattern: "needle", context: 1 },
		{ pattern: "needle", context: 3, limit: 1000 },
		{ pattern: "^export" },
		{ pattern: "needle$" },
		{ pattern: "\\bfoo\\(" },
		{ pattern: "foo.bar" },
		{ pattern: "foo.bar", literal: true },
		{ pattern: "a.b(c)", literal: true },
		{ pattern: "\\$dollar" },
		{ pattern: "ñame|日本語" },
		{ pattern: "[[:upper:]]{4,}" },
		// Matches every line; the large binary fixture would exceed the limit.
		{ pattern: "", glob: "!late-binary.dat" },
		{ pattern: "needle", glob: "*.ts" },
		{ pattern: "needle", glob: "*.{md,txt}" },
		{ pattern: "needle", glob: "!*.txt" },
		{ pattern: "needle", glob: "**/deep/**" },
		{ pattern: "needle", glob: ".hidden/*" },
		{ pattern: "needle", path: "src" },
		{ pattern: "needle", path: "src/index.ts" },
		{ pattern: "needle", path: "binary.dat" },
		{ pattern: "needle", path: "utf16.txt" },
		{ pattern: "needle", path: "invalid-utf8.txt", context: 1 },
		{ pattern: "needle", path: "missing" },
		{ pattern: "-x" },
		{ pattern: "--help" },
		{ pattern: "no match anywhere" },
		{ pattern: "(" },
		{ pattern: "a\\nb" },
		{ pattern: "(a)\\1" },
		{ pattern: "(?=x)" },
		{ pattern: "\\x00" },
		{ pattern: "needle", glob: "[" },
	];

	describe("outside a git repository", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("plain");
			fillTree(root);
		});

		it.each(cases)("%j", async (params) => {
			// A high default limit compares complete result sets; the match limit has its own tests below.
			const { rg, native } = await runBoth(root, { limit: 1000, ...params });
			expect(native).toEqual(rg);
		});
	});

	describe("inside a git repository", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("repo");
			mkdirSync(join(root, ".git", "info"), { recursive: true });
			writeFileSync(join(root, ".git", "HEAD"), "ref: refs/heads/main\n");
			writeFileSync(join(root, ".git", "info", "exclude"), "excluded.txt\n");
			fillTree(root);
			write(root, "excluded.txt", "needle excluded\n");
			// Nested repository: the parent's .gitignore must not apply inside it.
			mkdirSync(join(root, "nested", ".git"), { recursive: true });
			write(root, "nested/debug.log", "needle nested log\n");
		});

		it.each(cases)("%j", async (params) => {
			// A high default limit compares complete result sets; the match limit has its own tests below.
			const { rg, native } = await runBoth(root, { limit: 1000, ...params });
			expect(native).toEqual(rg);
		});
	});

	describe("match limits", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("limit");
			for (let i = 0; i < 30; i++) write(root, `f${String(i).padStart(2, "0")}.txt`, "needle\nneedle\nneedle\n");
		});

		it.each([1, 5, 50, 90, 100])("limit %d", async (limit) => {
			// Which matches fill the limit depends on search order in both implementations; compare everything else.
			const { rg, native } = await runBoth(root, { pattern: "needle", limit });
			const all = await runGrep(root, { pattern: "needle", limit: 1000 });
			if (!rg.ok || !native.ok || !all.ok) throw new Error("expected results");
			const [rgLines, rgNotice] = rg.text.split("\n\n[");
			const [nativeLines, nativeNotice] = native.text.split("\n\n[");
			expect(nativeNotice).toBe(rgNotice);
			expect(native.details).toEqual(rg.details);
			expect(nativeLines.split("\n")).toHaveLength(rgLines.split("\n").length);
			const allLines = new Set(all.text.split("\n"));
			for (const line of nativeLines.split("\n")) expect(allLines).toContain(line);
		});
	});

	describe.skipIf(process.platform === "win32" || process.getuid?.() === 0)("unreadable files", () => {
		let root: string;

		beforeAll(() => {
			root = makeRoot("locked");
			write(root, "ok.txt", "needle\n");
			write(root, "locked/secret.txt", "needle\n");
			write(root, "unreadable.txt", "needle\n");
			chmodSync(join(root, "unreadable.txt"), 0o000);
			chmodSync(join(root, "locked"), 0o000);
		});

		afterAll(() => {
			chmodSync(join(root, "unreadable.txt"), 0o644);
		});

		it("fails with ripgrep's error messages", async () => {
			const { rg, native } = await runBoth(root, { pattern: "needle" });
			expect(rg.ok).toBe(false);
			if (rg.ok || native.ok) throw new Error("expected errors");
			// ripgrep prints one line per failure in completion order.
			expect(native.error.split("\n").sort()).toEqual(rg.error.split("\n").sort());
		});

		it("ignores errors once the match limit is reached", async () => {
			const { rg, native } = await runBoth(root, { pattern: "needle", glob: "ok.txt", limit: 1 });
			expect(native).toEqual(rg);
		});
	});

	describe("abort", () => {
		it("rejects when aborted during the search", async () => {
			const root = makeRoot("abort");
			for (let i = 0; i < 2000; i++) write(root, `d${i % 50}/f${i}.txt`, "needle\n");
			setNativeGrep(grepFiles);
			const controller = new AbortController();
			const pending = runGrep(root, { pattern: "needle", limit: 100000 }, controller.signal);
			controller.abort();
			expect(await pending).toEqual({ ok: false, error: "Operation aborted" });
		});
	});
});
