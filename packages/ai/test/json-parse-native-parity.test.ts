import { parseStreamingJsonFast } from "@ruwanego/pi-native";
import { afterEach, describe, expect, it } from "vitest";
import { parseStreamingJson, setNativeStreamingJsonParser } from "../src/utils/json-parse.ts";

/**
 * Differential test for RUST-004: parseStreamingJson must return the same value with the Rust fast path installed as
 * without it, for every prefix of streamed tool-call arguments and for malformed input.
 */

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
		// partial-json's `obj["__proto__"] = value` gives each result its own prototype object.
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

function both(input: string): { typescript: unknown; native: unknown } {
	setNativeStreamingJsonParser(undefined);
	const typescript = parseStreamingJson(input);
	setNativeStreamingJsonParser(parseStreamingJsonFast);
	try {
		return { typescript, native: parseStreamingJson(input) };
	} finally {
		setNativeStreamingJsonParser(undefined);
	}
}

function expectSame(input: string): void {
	const { typescript, native } = both(input);
	const difference = sameValue(native, typescript);
	if (difference) throw new Error(`${difference}\ninput: ${JSON.stringify(input)}`);
}

function prefixes(text: string, maxCount = 4000): string[] {
	const step = Math.max(1, Math.floor(text.length / maxCount));
	const result: string[] = [];
	for (let length = 0; length <= text.length; length += step) result.push(text.slice(0, length));
	result.push(text);
	return result;
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

describe("parseStreamingJson: native fast path matches TypeScript", () => {
	afterEach(() => {
		setNativeStreamingJsonParser(undefined);
	});

	it.each(documents.map((doc, i) => [i, doc] as const))("every prefix of compact document %d", (_, doc) => {
		for (const prefix of prefixes(JSON.stringify(doc))) expectSame(prefix);
	});

	it.each(documents.map((doc, i) => [i, doc] as const))("every prefix of pretty-printed document %d", (_, doc) => {
		for (const prefix of prefixes(JSON.stringify(doc, null, 2), 1500)) expectSame(prefix);
	});

	it("handles most streaming prefixes natively", () => {
		const text = JSON.stringify(documents[0]);
		let native = 0;
		const all = prefixes(text);
		for (const prefix of all) if (prefix.trim() && parseStreamingJsonFast(prefix) !== undefined) native++;
		expect(native / all.length).toBeGreaterThan(0.9);
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
		expectSame(input);
	});

	it("matches on random mutations of streaming prefixes", () => {
		let seed = 7;
		const random = (n: number) => {
			seed = (seed * 1103515245 + 12345) % 2 ** 31;
			return seed % n;
		};
		const pieces = [
			'"',
			"\\",
			"\\u",
			"\\ud800",
			"{",
			"}",
			"[",
			"]",
			",",
			":",
			" ",
			"\t",
			"\n",
			" ",
			"1",
			"-",
			".",
			"e",
			"true",
			"null",
			"__proto__",
			"\u0001",
		];
		const sources = documents.map((doc) => JSON.stringify(doc));
		for (let i = 0; i < 20000; i++) {
			const source = sources[random(sources.length)]!;
			let text = source.slice(0, random(source.length + 1));
			const edits = random(3);
			for (let e = 0; e < edits; e++) {
				const at = random(text.length + 1);
				text = text.slice(0, at) + pieces[random(pieces.length)] + text.slice(at + random(2));
			}
			expectSame(text);
		}
	});
});
