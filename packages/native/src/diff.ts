import { loadNative } from "./binding.ts";

export function generateUnifiedPatch(path: string, oldContent: string, newContent: string, contextLines = 4): string {
	return loadNative().generateUnifiedPatch(path, oldContent, newContent, contextLines);
}

export function generateDiffString(
	oldContent: string,
	newContent: string,
	contextLines = 4,
): { diff: string; firstChangedLine: number | undefined } {
	const result = loadNative().generateDiffString(oldContent, newContent, contextLines);
	return { diff: result.diff, firstChangedLine: result.firstChangedLine ?? undefined };
}
