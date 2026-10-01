import assert from "node:assert";
import { readFileSync } from "node:fs";
import { describe, it } from "node:test";
import native, { loadNative, rustVersion } from "../src/index.ts";

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
