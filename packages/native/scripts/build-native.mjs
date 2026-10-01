#!/usr/bin/env node
// Builds crates/pi-native and copies the cdylib to prebuilds/<platform>-<arch>/pi-native.node.

import { spawnSync } from "node:child_process";
import { copyFileSync, mkdirSync } from "node:fs";
import { dirname, join, resolve } from "node:path";
import { fileURLToPath } from "node:url";

const packageDir = resolve(dirname(fileURLToPath(import.meta.url)), "..");
const repoRoot = resolve(packageDir, "../..");
const profile = process.argv.includes("--debug") ? "debug" : "release";

const cargoArgs = ["build", "-p", "pi-native", "--locked"];
if (profile === "release") cargoArgs.push("--release");

const result = spawnSync("cargo", cargoArgs, { cwd: repoRoot, stdio: "inherit" });
if (result.error) {
	console.error(`Failed to run cargo: ${result.error.message}. Install Rust from https://rustup.rs`);
	process.exit(1);
}
if (result.status !== 0) process.exit(result.status ?? 1);

const libraryNames = { darwin: "libpi_native.dylib", linux: "libpi_native.so", win32: "pi_native.dll" };
const libraryName = libraryNames[process.platform];
if (!libraryName) {
	console.error(`Unsupported platform: ${process.platform}`);
	process.exit(1);
}

const targetDir = process.env.CARGO_TARGET_DIR ? resolve(process.env.CARGO_TARGET_DIR) : join(repoRoot, "target");
const outDir = join(packageDir, "prebuilds", `${process.platform}-${process.arch}`);
mkdirSync(outDir, { recursive: true });
copyFileSync(join(targetDir, profile, libraryName), join(outDir, "pi-native.node"));
console.log(`pi-native: wrote ${join(outDir, "pi-native.node")}`);
