import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { protocolCodec } from "@earendil-works/pi-native";
import { getNativeCodec } from "@earendil-works/pi-protocol";
import { afterEach, describe, expect, it, vi } from "vitest";
import { disableRustFeatures, enableRustFeatures } from "../src/core/rust-features.ts";
import { createFindToolDefinition } from "../src/core/tools/find.ts";

// Without an fd binary, only the Rust search can answer a find call.
vi.mock("../src/utils/tools-manager.ts", () => ({ ensureTool: async () => undefined }));

async function find(root: string) {
	const def = createFindToolDefinition(root);
	return def.execute("call", { pattern: "*.txt" }, undefined, undefined, {} as Parameters<typeof def.execute>[4]);
}

describe("enableRustFeatures", () => {
	afterEach(() => {
		disableRustFeatures();
	});

	it("switches every Rust implementation on and off", async () => {
		const root = mkdtempSync(join(tmpdir(), "pi-rust-features-"));
		try {
			writeFileSync(join(root, "a.txt"), "");
			await expect(find(root)).rejects.toThrow("fd is not available");
			expect(getNativeCodec()).toBeUndefined();

			enableRustFeatures();
			expect(getNativeCodec()).toBe(protocolCodec);
			const result = await find(root);
			expect(result.content).toEqual([{ type: "text", text: "a.txt" }]);

			disableRustFeatures();
			expect(getNativeCodec()).toBeUndefined();
			await expect(find(root)).rejects.toThrow("fd is not available");
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
});
