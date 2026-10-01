import assert from "node:assert";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import native, { loadNative, protocolCodec, rustVersion } from "../src/index.ts";

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
