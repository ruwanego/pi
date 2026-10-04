import assert from "node:assert";
import { mkdirSync, mkdtempSync, readFileSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join, sep } from "node:path";
import { describe, it } from "node:test";
import native, {
	findFiles,
	grepFiles,
	loadNative,
	parseStreamingJsonFast,
	protocolCodec,
	rustVersion,
} from "../src/index.ts";

const packageJson = JSON.parse(readFileSync(new URL("../package.json", import.meta.url), "utf8")) as {
	version: string;
};

describe("pi-native", () => {
	it("loads the addon", () => {
		assert.strictEqual(typeof loadNative().rustVersion, "function");
	});

	it("returns the crate version from Rust", () => {
		assert.strictEqual(rustVersion(), packageJson.version);
	});

	it("exposes the same function on the default export", () => {
		assert.strictEqual(native.rustVersion(), rustVersion());
	});
});

describe("protocolCodec", () => {
	const limits = { maxByteLength: 1024, maxContainerLength: 16, maxDepth: 8 };

	it("round-trips CBOR through Rust", () => {
		const wire = protocolCodec.encodeCbor({ a: 1, b: [2, 3] }, limits);
		assert.strictEqual(Buffer.from(wire).toString("hex"), "a26161016162820203");
		assert.deepStrictEqual(protocolCodec.decodeCbor(wire, limits), { a: 1, b: [2, 3] });
	});

	it("tags protocol failures with an error code", () => {
		assert.throws(() => protocolCodec.decodeCbor(new Uint8Array([0xff]), limits), {
			code: "PI_CBOR_ERROR",
			message: "CBOR break marker is not supported",
		});
		const decoder = protocolCodec.createFrameDecoder(1);
		assert.throws(() => decoder.push(new Uint8Array([0, 0, 0, 2])), { code: "PI_FRAME_ERROR" });
	});
});

describe("findFiles", () => {
	const request = (searchPath: string, pattern: string, maxResults = 0) => ({
		pattern,
		searchPath,
		fullPath: false,
		requireGit: false,
		maxResults,
	});

	it("prints absolute paths in fd's format", async () => {
		const root = mkdtempSync(join(tmpdir(), "pi-native-find-"));
		try {
			mkdirSync(join(root, "src.ts"));
			writeFileSync(join(root, "a.ts"), "");
			writeFileSync(join(root, "b.js"), "");
			assert.deepStrictEqual(await findFiles(request(root, "*.ts")), [
				join(root, "a.ts"),
				`${join(root, "src.ts")}${sep}`,
			]);
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("rejects with fd's error text", async () => {
		await assert.rejects(findFiles(request(tmpdir(), "[")), {
			message: "[fd error]: error parsing glob '[': unclosed character class; missing ']'",
		});
		await assert.rejects(findFiles(request(tmpdir(), "*", 2.5)), /invalid value '2.5' for '--max-results <count>'/);
	});
});

describe("grepFiles", () => {
	const request = (searchPath: string, pattern: string) => ({
		pattern,
		searchPath,
		ignoreCase: false,
		fixedStrings: false,
		maxMatches: 100,
	});

	it("returns ripgrep's match messages", async () => {
		const root = mkdtempSync(join(tmpdir(), "pi-native-grep-"));
		try {
			writeFileSync(join(root, "a.txt"), "one\nneedle\n");
			const result = await grepFiles(request(root, "needle"));
			assert.deepStrictEqual(result, {
				matches: [{ path: join(root, "a.txt"), lineNumber: 2, line: "needle\n" }],
				limitReached: false,
				stderr: "",
				errored: false,
			});
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});

	it("rejects with ripgrep's error text", async () => {
		await assert.rejects(grepFiles(request(tmpdir(), "(")), {
			message: "rg: regex parse error:\n    (?:()\n    ^\nerror: unclosed group",
		});
	});
});

describe("parseStreamingJsonFast", () => {
	it("parses complete JSON and closes streamed prefixes", () => {
		assert.deepStrictEqual(parseStreamingJsonFast('{"a":[1,"x"]}'), { a: [1, "x"] });
		assert.deepStrictEqual(parseStreamingJsonFast('{"path":"a.ts","content":"line\\n'), {
			path: "a.ts",
			content: "line\n",
		});
		const content = "y".repeat(5000);
		assert.deepStrictEqual(parseStreamingJsonFast(`{"content":"${content}`), { content });
	});

	it("defers inputs it does not handle", () => {
		assert.strictEqual(parseStreamingJsonFast('{"a":"bad \\q"}'), undefined);
		assert.strictEqual(parseStreamingJsonFast("12"), undefined);
	});
});

describe("captured built-ins", () => {
	it("keeps using the original JSON.parse after the global is replaced", () => {
		const limits = { maxByteLength: 1024, maxContainerLength: 16, maxDepth: 8 };
		const wire = protocolCodec.encodeCbor({ a: [1, "x"] }, limits);
		const original = JSON.parse;
		JSON.parse = () => {
			throw new Error("replaced");
		};
		try {
			assert.deepStrictEqual(protocolCodec.decodeCbor(wire, limits), { a: [1, "x"] });
			assert.deepStrictEqual(parseStreamingJsonFast('{"a":"b'), { a: "b" });
		} finally {
			JSON.parse = original;
		}
	});
});
