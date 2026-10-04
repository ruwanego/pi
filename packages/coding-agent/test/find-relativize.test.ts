import { posix } from "node:path";
import { describe, expect, it } from "vitest";
import { relativizeFindResultPath } from "../src/core/tools/find.ts";

/** The original implementation, which always used path.relative for absolute results. */
function reference(resultPath: string, searchPath: string): string {
	const hadTrailingSeparator = resultPath.endsWith("/");
	const relativePath = posix.isAbsolute(resultPath) ? posix.relative(searchPath, resultPath) : resultPath;
	const posixPath = relativePath.split("/").join("/");
	return hadTrailingSeparator && !posixPath.endsWith("/") ? `${posixPath}/` : posixPath;
}

describe("relativizeFindResultPath prefix fast path", () => {
	const searchPaths = ["/", "/repo", "/repo/src", "/re po/ünï", "/repo/", "/repo/./src", "/repo/../other"];
	const segments = ["a", "b.ts", ".hidden", "..x", "x..", "...", ".", "..", "", " sp ace", "ünï", "repo", "src"];

	it("matches path.relative on generated paths", () => {
		let seed = 1;
		const random = (n: number) => {
			seed = (seed * 1103515245 + 12345) % 2 ** 31;
			return seed % n;
		};
		for (let i = 0; i < 5000; i++) {
			const searchPath = searchPaths[random(searchPaths.length)]!;
			const parts = Array.from({ length: 1 + random(4) }, () => segments[random(segments.length)]!);
			const base = random(4) === 0 ? "/elsewhere" : searchPath;
			let resultPath = `${base.endsWith("/") ? base : `${base}/`}${parts.join("/")}`;
			if (random(3) === 0) resultPath += "/";
			expect(relativizeFindResultPath(resultPath, searchPath, posix), `${resultPath} from ${searchPath}`).toBe(
				reference(resultPath, searchPath),
			);
		}
	});
});
