import { loadNative } from "@ruwanego/pi-native";
import { describe, it } from "vitest";
import { parseStreamingJson } from "../src/utils/json-parse.ts";
import { StreamingJsonParser, setNativeStreamingJsonParser } from "../src/utils/streaming-json-parser.ts";

/** Strict structural equality: same types, prototypes, key order, and Object.is for primitives. */
function sameValue(a: unknown, b: unknown, path = "$"): string | undefined {
	if (typeof a !== typeof b) return `${path}: ${typeof a} vs ${typeof b}`;
	if (a === null || b === null || typeof a !== "object") {
		return Object.is(a, b) ? undefined : `${path}: ${String(a)} vs ${String(b)}`;
	}
	const protoA = Object.getPrototypeOf(a);
	const protoB = Object.getPrototypeOf(b);
	const standard = [Object.prototype, Array.prototype, null];
	if (standard.includes(protoA) || standard.includes(protoB)) {
		if (protoA !== protoB) return `${path}: different prototypes`;
	} else {
		const difference = sameValue(protoA, protoB, `${path}.[[Prototype]]`);
		if (difference) return difference;
	}
	if (Array.isArray(a) !== Array.isArray(b)) return `${path}: array vs object`;
	const keysA = Reflect.ownKeys(a as object);
	const keysB = Reflect.ownKeys(b as object);
	if (keysA.length !== keysB.length || keysA.some((key, i) => key !== keysB[i])) {
		return `${path}: keys ${JSON.stringify(keysA.map(String))} vs ${JSON.stringify(keysB.map(String))}`;
	}
	for (const key of keysA) {
		const difference = sameValue(
			(a as Record<PropertyKey, unknown>)[key],
			(b as Record<PropertyKey, unknown>)[key],
			`${path}.${String(key)}`,
		);
		if (difference) return difference;
	}
	return undefined;
}

function expectSameChunks(chunks: string[]): void {
	const input = chunks.join("");
	const typescript = parseStreamingJson(input);

	const parser = new StreamingJsonParser();
	for (const chunk of chunks) {
		parser.append(chunk);
	}
	const native = parser.parsed;

	const difference = sameValue(native, typescript);
	if (difference) throw new Error(`${difference}\ninput: ${JSON.stringify(input)}`);
}

function chunkify(text: string, maxCount = 4000): string[][] {
	const step = Math.max(1, Math.floor(text.length / maxCount));
	const result: string[][] = [];
	const current: string[] = [];
	for (let length = 0; length <= text.length; length += step) {
		const slice = text.slice(length - step, length);
		if (slice) current.push(slice);
		result.push([...current, text.slice(length)]); // this is not quite right
	}
	// Let's just create sequences of chunks that build up the string
	const sequences: string[][] = [];
	for (let length = 0; length <= text.length; length += step) {
		const prefix = text.slice(0, length);
		// just split prefix into 1-3 chunks
		const p1 = Math.floor(prefix.length / 3);
		const p2 = Math.floor((prefix.length * 2) / 3);
		sequences.push([prefix.slice(0, p1), prefix.slice(p1, p2), prefix.slice(p2)]);
	}
	sequences.push([text]);
	return sequences;
}

const longContent = Array.from(
	{ length: 120 },
	(_, i) => `line ${i}: const s = "quote \\"x\\"" + '\\t' + "\\u00e9 é 日本 😀"; // ${"x".repeat(i % 7)}`,
).join("\n");

const documents: unknown[] = [
	{ path: "src/index.ts", content: longContent },
	{ path: "a.txt", content: "short" },
	{ command: "ls -la", timeout: 120, background: false, cwd: null },
	{
		edits: [
			{ oldText: "a\r\nb", newText: "c\td" },
			{ oldText: "\u0000\u001f", newText: "" },
		],
	},
	{ nested: { deep: { deeper: [1, -2.5, 3e21, 1e-7, 0, -0, [], {}, [[]], [{}]] } } },
	{ unicode: "  ﻿  trailing   ", emoji: "😀👍🏽", escaped: "😀" },
	{ 'key with "quotes"': 1, "": 2, "123": 3, "1": 4, b: 5, a: 6 },
	{ big: Number.MAX_SAFE_INTEGER, small: Number.MIN_SAFE_INTEGER, float: 0.1 + 0.2 },
	[1, "two", { three: 3 }, [4, [5]], true, false, null],
	{ content: "x".repeat(5000), more: "y".repeat(3000) },
	{ values: Array.from({ length: 60 }, (_, i) => (i - 30) * 1.25 * 10 ** ((i % 9) - 4)) },
];

describe("StreamingJsonParser: native stateful fast path matches TypeScript", () => {
	it("initializes native parser", () => {
		setNativeStreamingJsonParser(() => new (loadNative().StreamingJsonParser)());
	});

	it.each(documents.map((doc, i) => [i, doc] as const))("every prefix of compact document %d", (_, doc) => {
		for (const chunks of chunkify(JSON.stringify(doc))) expectSameChunks(chunks);
	});

	it.each(documents.map((doc, i) => [i, doc] as const))("every prefix of pretty-printed document %d", (_, doc) => {
		for (const chunks of chunkify(JSON.stringify(doc, null, 2), 1500)) expectSameChunks(chunks);
	});

	it.each([
		// Repair cases: raw control characters, invalid escapes, trailing backslashes.
		'{"a":"raw\ttab"}',
		'{"a":"raw\nnewline',
		'{"a":"bad \\q escape"}',
		'{"a":"bad \\q escape',
		'{"a":"ends with backslash\\',
		'{"a":"partial \\u12',
		'{"a":"partial \\u',
		'{"a":"x"} trailing',
		'{"a":"x"}}',
		" {}",
		"{}\u000b",
		// partial-json quirks.
		'{"a":[ ]}',
		'{"a":[ ],"b":1',
		'{"a": tr',
		'{"a": nu',
		'{"a": -',
		'{"a": 1.',
		'{"a": 1e',
		'{"name": 1.',
		'{"a": 12',
		'{"a": 1E',
		'{"a": 1.5E+',
		'{"a": 1.5e+',
		'{"a": -e',
		'{"a": 1e5e',
		'{"value": 1.',
		"[1, 2.",
		"[1, -",
		"[1, 1.5e",
		'{"a": Infinity',
		'{"a": NaN}',
		'{"__proto__":{"polluted":true}}',
		'{"__proto__":{"polluted":true',
		'{"\\u005f_proto__":[]}',
		'{"a":1,"a":2}',
		'{"a":1,"a":',
		// Lone surrogates, raw and escaped.
		'{"a":"\ud800"}',
		'{"a":"x\udc00y',
		'{"a":"\\ud800"}',
		'{"a":"\\ud800',
		'{"a":"\\ud83d\\ude00',
		// Roots that are not objects or arrays.
		'"string',
		"12",
		"null",
		"tru",
		// Whitespace and BOM.
		"﻿{}",
		'  {"a":1}  ',
		'{"a":"x   ',
		'{"a":"x  ',
		"",
		"   ",
	])("malformed or unusual input %j", (input) => {
		expectSameChunks([input]);
	});
});
