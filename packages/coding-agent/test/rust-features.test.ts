import { mkdtempSync, rmSync, writeFileSync } from "node:fs";
import { tmpdir } from "node:os";
import { join } from "node:path";
import { getNativeCodec } from "@earendil-works/pi-protocol";
import { protocolCodec } from "@ruwanego/pi-native";
import { afterEach, describe, expect, it, vi } from "vitest";
import { disableRustFeatures, enableRustFeatures } from "../src/core/rust-features.ts";
import { createFindToolDefinition } from "../src/core/tools/find.ts";
import { createGrepToolDefinition } from "../src/core/tools/grep.ts";

// Without fd or ripgrep binaries, only the Rust searches can answer find and grep calls.
vi.mock("../src/utils/tools-manager.ts", () => ({ ensureTool: async () => undefined }));

async function grep(root: string) {
	const def = createGrepToolDefinition(root);
	return def.execute("call", { pattern: "needle" }, undefined, undefined, {} as Parameters<typeof def.execute>[4]);
}

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
			writeFileSync(join(root, "a.txt"), "needle\n");
			await expect(find(root)).rejects.toThrow("fd is not available");
			await expect(grep(root)).rejects.toThrow("ripgrep (rg) is not available");
			expect(getNativeCodec()).toBeUndefined();

			enableRustFeatures();
			expect(getNativeCodec()).toBe(protocolCodec);
			const result = await find(root);
			expect(result.content).toEqual([{ type: "text", text: "a.txt" }]);
			expect((await grep(root)).content).toEqual([{ type: "text", text: "a.txt:1: needle" }]);

			disableRustFeatures();
			expect(getNativeCodec()).toBeUndefined();
			await expect(find(root)).rejects.toThrow("fd is not available");
			await expect(grep(root)).rejects.toThrow("ripgrep (rg) is not available");
		} finally {
			rmSync(root, { recursive: true, force: true });
		}
	});
});
