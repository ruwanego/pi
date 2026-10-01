import { protocolCodec } from "@earendil-works/pi-native";
import { afterAll, describe, expect, test } from "vitest";
import {
	CborError,
	type CborOptions,
	DEFAULT_MAX_CBOR_DEPTH,
	decodeCbor,
	encodeCbor,
	encodeFrame,
	FrameDecoder,
	type FrameDecoderOptions,
	getNativeCodec,
	type NativeCodec,
	setNativeCodec,
} from "../src/index.ts";

// Differential tests: every input runs through the TypeScript codec and the Rust codec, and the observable results
// (bytes, decoded structure including prototypes and -0, error class and message) must be identical.

const previousCodec = getNativeCodec();
afterAll(() => setNativeCodec(previousCodec));

type Outcome = { ok: string } | { error: string };

function describeValue(value: unknown, seen = new Set<object>()): string {
	if (value === null) return "null";
	if (typeof value === "number") return Object.is(value, -0) ? "number:-0" : `number:${value}`;
	if (typeof value !== "object") return `${typeof value}:${String(value)}`;
	if (seen.has(value)) return "<cycle>";
	seen.add(value);
	const prototype = Object.getPrototypeOf(value);
	const prototypeName = prototype === null ? "null" : (prototype.constructor?.name ?? "?");
	if (value instanceof Uint8Array) return `${prototypeName}(${Buffer.from(value).toString("hex")})`;
	const entries = Reflect.ownKeys(value).map((key) => {
		const descriptor = Object.getOwnPropertyDescriptor(value, key)!;
		const flags = `${descriptor.enumerable ? "e" : ""}${descriptor.writable ? "w" : ""}${descriptor.configurable ? "c" : ""}`;
		return `${String(key)}[${flags}]=${"value" in descriptor ? describeValue(descriptor.value, seen) : "<accessor>"}`;
	});
	return `${prototypeName}{${entries.join(",")}}`;
}

function run(codec: NativeCodec | undefined, operation: () => unknown): Outcome {
	setNativeCodec(codec);
	try {
		return { ok: describeValue(operation()) };
	} catch (error) {
		const name = error instanceof Error ? error.constructor.name : typeof error;
		return { error: `${name}: ${error instanceof Error ? error.message : String(error)}` };
	} finally {
		setNativeCodec(previousCodec);
	}
}

function expectParity(operation: () => unknown): Outcome {
	const typescript = run(undefined, operation);
	const native = run(protocolCodec, operation);
	expect(native).toEqual(typescript);
	return typescript;
}

/** Deterministic PRNG so failures are reproducible. */
function random(seed: number): () => number {
	let state = seed >>> 0;
	return () => {
		state = (state + 0x6d2b79f5) >>> 0;
		let t = state;
		t = Math.imul(t ^ (t >>> 15), t | 1);
		t ^= t + Math.imul(t ^ (t >>> 7), t | 61);
		return ((t ^ (t >>> 14)) >>> 0) / 4294967296;
	};
}

const STRINGS = [
	"",
	"a",
	"IETF",
	"ü",
	"水",
	"𐅑",
	"﻿",
	"\ud800",
	"a\udc00b",
	"__proto__",
	"constructor",
	"x".repeat(300),
];
const NUMBERS = [
	0,
	-0,
	1,
	-1,
	23,
	24,
	255,
	256,
	65_535,
	65_536,
	2 ** 32 - 1,
	2 ** 32,
	Number.MAX_SAFE_INTEGER,
	Number.MIN_SAFE_INTEGER,
	2 ** 53,
	-(2 ** 53),
	0.1,
	-1.5,
	1e300,
	Number.MIN_VALUE,
	Number.EPSILON,
	Number.NaN,
	Number.POSITIVE_INFINITY,
];

function randomValue(next: () => number, depth: number): unknown {
	const pick = <T>(items: readonly T[]): T => items[Math.floor(next() * items.length)]!;
	const roll = next();
	if (depth > 4 || roll < 0.45) {
		return pick<unknown>([
			null,
			true,
			false,
			pick(NUMBERS),
			Math.floor(next() * 2 ** 40) - 2 ** 39,
			next() * 1e6,
			pick(STRINGS),
			Uint8Array.from({ length: Math.floor(next() * 40) }, () => Math.floor(next() * 256)),
		]);
	}
	if (roll < 0.7) return Array.from({ length: Math.floor(next() * 6) }, () => randomValue(next, depth + 1));
	const result: Record<string, unknown> = {};
	for (let index = Math.floor(next() * 6); index > 0; index--) {
		Object.defineProperty(result, pick([...STRINGS, `k${index}`, "0", "1"]), {
			configurable: true,
			enumerable: true,
			value: next() < 0.1 ? undefined : randomValue(next, depth + 1),
			writable: true,
		});
	}
	return result;
}

describe("native codec parity", () => {
	test("encodes edge-case JavaScript values identically", () => {
		const cyclicArray: unknown[] = [];
		cyclicArray.push(cyclicArray);
		const cyclicObject: Record<string, unknown> = {};
		cyclicObject.self = cyclicObject;
		const shared = { shared: true };
		const enumerableSymbol = { visible: 1 };
		Object.defineProperty(enumerableSymbol, Symbol("hidden"), { enumerable: true, value: 1 });
		const hiddenSymbol = { visible: 1 };
		Object.defineProperty(hiddenSymbol, Symbol("hidden"), { enumerable: false, value: 1 });
		const protoKey: Record<string, unknown> = {};
		Object.defineProperty(protoKey, "__proto__", { enumerable: true, value: "safe" });
		const getters = {
			get computed() {
				return [1, 2];
			},
			get missing() {
				return undefined;
			},
		};
		const throwing = {
			get boom() {
				throw new SyntaxError("getter failed");
			},
		};
		const arrayWithGetter: unknown[] = [1, 2];
		Object.defineProperty(arrayWithGetter, 1, { get: () => "from getter", enumerable: true });
		const bytes = Uint8Array.from([0, 1, 2, 3, 4, 5, 6, 7]);
		let deep: unknown = null;
		for (let depth = 0; depth < DEFAULT_MAX_CBOR_DEPTH; depth++) deep = [deep];

		const values: unknown[] = [
			...NUMBERS,
			...STRINGS,
			undefined,
			1n,
			Symbol("value"),
			() => undefined,
			new Date(0),
			new Map(),
			new Set(),
			/regex/,
			new (class Custom {})(),
			Object.create(null),
			Object.assign(Object.create(null), { a: 1 }),
			Object.create({ inherited: 1 }),
			new Array(2),
			[1, undefined],
			{ a: undefined, b: 0, c: "", d: false, e: null },
			{ 2: "b", 1: "a", z: 0, y: 1 },
			cyclicArray,
			cyclicObject,
			[shared, shared, { shared }],
			enumerableSymbol,
			hiddenSymbol,
			protoKey,
			getters,
			throwing,
			arrayWithGetter,
			bytes,
			bytes.subarray(3, 6),
			Buffer.from([9, 8, 7]),
			new Uint8ClampedArray([1]),
			new Uint16Array([1]),
			new DataView(new ArrayBuffer(1)),
			new ArrayBuffer(2),
			Object.create(Uint8Array.prototype),
			new Proxy([1, 2, 3], {}),
			new Proxy({ a: 1 }, {}),
			new Proxy(
				{ a: 1 },
				{
					ownKeys: () => ["a", "b"],
					getOwnPropertyDescriptor: () => ({ enumerable: true, configurable: true, value: 7 }),
				},
			),
			deep,
			[deep],
		];
		for (const value of values) expectParity(() => encodeCbor(value));
	});

	test("enforces limits at the same byte, depth, and container boundaries", () => {
		const value = { text: "héllo wörld", list: [1, 2.5, [true, null]], bytes: Uint8Array.from([1, 2, 3]), n: -300 };
		const encodedLength = encodeCbor(value).byteLength;
		for (let maxByteLength = 0; maxByteLength <= encodedLength + 1; maxByteLength++) {
			const options: CborOptions = { maxByteLength };
			const encoded = expectParity(() => encodeCbor(value, options));
			const wire = encodeCbor(value);
			expectParity(() => decodeCbor(wire, options));
			expect("ok" in encoded).toBe(maxByteLength >= encodedLength);
		}
		for (let maxContainerLength = 0; maxContainerLength <= 5; maxContainerLength++) {
			expectParity(() => encodeCbor(value, { maxContainerLength }));
			expectParity(() => decodeCbor(encodeCbor(value), { maxContainerLength }));
		}
		for (let maxDepth = 0; maxDepth <= 4; maxDepth++) {
			expectParity(() => encodeCbor(value, { maxDepth }));
			expectParity(() => decodeCbor(encodeCbor(value), { maxDepth }));
		}
		for (const options of [{ maxDepth: 513 }, { maxByteLength: -1 }, { maxContainerLength: 1.5 }]) {
			expectParity(() => encodeCbor(value, options));
		}
		expectParity(() => decodeCbor("not bytes" as unknown as Uint8Array));
	});

	test("matches on randomly generated values and their decoded form", () => {
		const next = random(0x5eed);
		for (let iteration = 0; iteration < 3000; iteration++) {
			const value = randomValue(next, 0);
			expectParity(() => encodeCbor(value));
			let wire: Uint8Array;
			try {
				wire = encodeCbor(value);
			} catch (error) {
				expect(error).toBeInstanceOf(CborError);
				continue;
			}
			expectParity(() => decodeCbor(wire));
		}
	});

	test("matches on mutated, truncated, and random byte strings", () => {
		const next = random(0xc0ffee);
		for (let iteration = 0; iteration < 4000; iteration++) {
			let wire: Uint8Array;
			try {
				wire = encodeCbor(randomValue(next, 0));
			} catch {
				continue;
			}
			const mutated = wire.slice(0, Math.max(0, wire.byteLength - Math.floor(next() * 3)));
			for (let flips = Math.floor(next() * 3); flips > 0 && mutated.byteLength > 0; flips--) {
				mutated[Math.floor(next() * mutated.byteLength)] = Math.floor(next() * 256);
			}
			expectParity(() => decodeCbor(mutated));
			const noise = Uint8Array.from({ length: Math.floor(next() * 12) }, () => Math.floor(next() * 256));
			expectParity(() => decodeCbor(noise));
		}
	});

	test("splits fragmented frame streams identically", () => {
		const next = random(0xf4a3e);
		for (let iteration = 0; iteration < 500; iteration++) {
			const payloads = Array.from({ length: Math.floor(next() * 5) }, () =>
				Uint8Array.from({ length: Math.floor(next() * 20) }, () => Math.floor(next() * 256)),
			);
			const parts = payloads.map((payload) => encodeFrame(payload));
			if (next() < 0.2)
				parts.push(Uint8Array.from({ length: Math.floor(next() * 6) }, () => Math.floor(next() * 256)));
			const wire = new Uint8Array(parts.reduce((length, part) => length + part.byteLength, 0));
			let offset = 0;
			for (const part of parts) {
				wire.set(part, offset);
				offset += part.byteLength;
			}
			const options: FrameDecoderOptions = { maxFrameLength: Math.floor(next() * 24) };
			const cuts = Array.from({ length: Math.floor(next() * 5) }, () =>
				Math.floor(next() * (wire.byteLength + 1)),
			).sort((a, b) => a - b);

			expectParity(() => {
				const decoder = new FrameDecoder(options);
				const events: unknown[] = [];
				let start = 0;
				for (const cut of [...cuts, wire.byteLength]) {
					try {
						events.push(decoder.push(wire.subarray(start, cut)));
					} catch (error) {
						events.push(`${(error as Error).constructor.name}: ${(error as Error).message}`);
					}
					start = cut;
				}
				try {
					decoder.end();
					events.push("ended");
				} catch (error) {
					events.push(`${(error as Error).constructor.name}: ${(error as Error).message}`);
				}
				return events;
			});
		}
		for (const payload of [new Uint8Array(), Uint8Array.from([1, 2, 3]), Buffer.from("frame")]) {
			expectParity(() => encodeFrame(payload));
		}
		expectParity(() => encodeFrame("nope" as unknown as Uint8Array));
		expectParity(() => new FrameDecoder().push("nope" as unknown as Uint8Array));
	});
});
