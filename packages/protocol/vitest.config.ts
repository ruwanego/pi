import { defineConfig } from "vitest/config";

export default defineConfig({
	test: {
		globals: true,
		environment: "node",
		reporters: process.env.GITHUB_ACTIONS ? ["dot", "github-actions"] : ["dot"],
		// The whole suite runs against the TypeScript codec and again with the Rust codec from pi-native installed.
		projects: [
			{ extends: true, test: { name: "typescript" } },
			{ extends: true, test: { name: "native", setupFiles: ["./test/setup-native.ts"] } },
		],
	},
	resolve: { conditions: ["source"] },
	ssr: { resolve: { conditions: ["source"] } },
});
