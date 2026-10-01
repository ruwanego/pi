import { createRequire } from "node:module";
import { dirname, join } from "node:path";
import { fileURLToPath } from "node:url";

/** Functions exported by crates/pi-native. Keep in sync with the #[napi] exports. */
export interface NativeBinding {
	rustVersion(): string;
}

const cjsRequire = createRequire(import.meta.url);
let binding: NativeBinding | undefined;

/** Path of the prebuilt addon for the current platform, relative to this package. */
export function getNativeModulePath(): string {
	const packageDir = join(dirname(fileURLToPath(import.meta.url)), "..");
	return join(packageDir, "prebuilds", `${process.platform}-${process.arch}`, "pi-native.node");
}

/** Loads the addon on first use. Throws if it has not been built for this platform. */
export function loadNative(): NativeBinding {
	if (binding) return binding;
	const modulePath = getNativeModulePath();
	try {
		binding = cjsRequire(modulePath) as NativeBinding;
	} catch (error) {
		throw new Error(`Failed to load pi-native from ${modulePath}. Run "npm run build" in packages/native.`, {
			cause: error,
		});
	}
	return binding;
}

/** Version of the Rust crate behind the bridge. */
export function rustVersion(): string {
	return loadNative().rustVersion();
}

const native = { rustVersion };
export default native;
