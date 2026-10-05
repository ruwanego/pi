import { describe, expect, it } from "vitest";
import { applyEditsToNormalizedContent, fuzzyFindText } from "../src/core/tools/edit-diff.ts";

describe("batch edit matching", () => {
	it("matches all edits against the original text and preserves untouched bytes", () => {
		const content = "keep ‘quotes’ and ＡＢＣ  \t\nfirst\nsecond\nlast  \t\n";
		expect(
			applyEditsToNormalizedContent(
				content,
				[
					{ oldText: "second", newText: "third" },
					{ oldText: "first", newText: "second" },
				],
				"file.txt",
			),
		).toEqual({ baseContent: content, newContent: "keep ‘quotes’ and ＡＢＣ  \t\nsecond\nthird\nlast  \t\n" });
	});

	it("rematches exact edits after fuzzy normalization changes offsets", () => {
		const content = "keep ‘quotes’  \t\nＡＢＣ  \t\nfinish\nuntouched　\n";
		expect(
			applyEditsToNormalizedContent(
				content,
				[
					{ oldText: "finish", newText: "done" },
					{ oldText: "ABC", newText: "changed" },
				],
				"file.txt",
			),
		).toEqual({ baseContent: content, newContent: "keep ‘quotes’  \t\nchanged\ndone\nuntouched　\n" });
	});

	it("normalizes CRLF edit arguments and supports deletions", () => {
		expect(
			applyEditsToNormalizedContent(
				"first\nsecond\nthird\n",
				[
					{ oldText: "first\r\nsecond", newText: "one\r\ntwo" },
					{ oldText: "third\n", newText: "" },
				],
				"file.txt",
			).newContent,
		).toBe("one\ntwo\n");
	});

	it("rejects normalized duplicates even when one occurrence matches exactly", () => {
		expect(() =>
			applyEditsToNormalizedContent("ＡＢＣ\nABC\nother\n", [{ oldText: "ABC", newText: "changed" }], "file.txt"),
		).toThrow(
			"Found 2 occurrences of the text in file.txt. The text must be unique. Please provide more context to make it unique.",
		);
	});

	it("reports the original edit index and full normalized duplicate count", () => {
		expect(() =>
			applyEditsToNormalizedContent(
				"unique\nＡＢＣ\nABC\nＡBC\n",
				[
					{ oldText: "unique", newText: "changed" },
					{ oldText: "ABC", newText: "replacement" },
				],
				"file.txt",
			),
		).toThrow(
			"Found 3 occurrences of edits[1] in file.txt. Each oldText must be unique. Please provide more context to make it unique.",
		);
	});

	it("checks empty oldText before matching any edits", () => {
		expect(() =>
			applyEditsToNormalizedContent(
				"content",
				[
					{ oldText: "missing", newText: "replacement" },
					{ oldText: "", newText: "replacement" },
				],
				"file.txt",
			),
		).toThrow("edits[1].oldText must not be empty in file.txt.");
	});

	it("reports earlier duplicate errors before later missing edits", () => {
		expect(() =>
			applyEditsToNormalizedContent(
				"ABC\nＡＢＣ\n",
				[
					{ oldText: "ABC", newText: "replacement" },
					{ oldText: "missing", newText: "replacement" },
				],
				"file.txt",
			),
		).toThrow("Found 2 occurrences of edits[0] in file.txt.");
	});

	it("detects overlaps after fuzzy matching", () => {
		expect(() =>
			applyEditsToNormalizedContent(
				"ＡＢＣＤＥ\n",
				[
					{ oldText: "CDE", newText: "x" },
					{ oldText: "ABC", newText: "y" },
				],
				"file.txt",
			),
		).toThrow("edits[1] and edits[0] overlap in file.txt. Merge them into one edit or target disjoint regions.");
	});

	it("does not reuse a normalized view between operations", () => {
		const edits = [{ oldText: "ABC", newText: "replacement" }];
		expect(applyEditsToNormalizedContent("ＡＢＣ\n", edits, "file.txt").newContent).toBe("replacement\n");
		expect(() => applyEditsToNormalizedContent("different\n", edits, "file.txt")).toThrow(
			"Could not find the exact text in file.txt. The old text must match exactly including all whitespace and newlines.",
		);
	});

	it("preserves no-change errors", () => {
		expect(() => applyEditsToNormalizedContent("same\n", [{ oldText: "same", newText: "same" }], "file.txt")).toThrow(
			"No changes made to file.txt. The replacement produced identical content. This might indicate an issue with special characters or the text not existing as expected.",
		);
	});

	it("keeps standalone matching exact-first", () => {
		expect(fuzzyFindText("ＡＢＣ\nABC\n", "ABC")).toEqual({
			found: true,
			index: 4,
			matchLength: 3,
			usedFuzzyMatch: false,
			contentForReplacement: "ＡＢＣ\nABC\n",
		});
	});
});
