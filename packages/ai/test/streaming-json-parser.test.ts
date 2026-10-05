import { describe, expect, it } from "vitest";
import { StreamingJsonParser } from "../src/utils/streaming-json-parser.ts";

describe("StreamingJsonParser", () => {
	it("parses a simple object incrementally", () => {
		const parser = new StreamingJsonParser();
		parser.append('{"a"');
		expect(parser.parsed).toEqual({ a: null });
		parser.append(': 1, "b": ');
		expect(parser.parsed).toEqual({ a: 1, b: null });
		parser.append("[1, 2, ");
		expect(parser.parsed).toEqual({ a: 1, b: [1, 2] });
		parser.append("3]}");
		expect(parser.parsed).toEqual({ a: 1, b: [1, 2, 3] });
	});

	it("handles escaped strings", () => {
		const parser = new StreamingJsonParser();
		parser.append('{"str": "he');
		expect(parser.parsed).toEqual({ str: "he" });
		parser.append('ll\\no"');
		expect(parser.parsed).toEqual({ str: "hell\no" });
		parser.append("}");
		expect(parser.parsed).toEqual({ str: "hell\no" });
	});

	it("handles partial literals", () => {
		const parser = new StreamingJsonParser();
		parser.append('{"a": tr');
		expect(parser.parsed).toEqual({ a: true }); // or wait for complete literal
		parser.append('ue, "b": nu');
		expect(parser.parsed).toEqual({ a: true, b: null });
		parser.append("ll}");
		expect(parser.parsed).toEqual({ a: true, b: null });
	});

	it("handles nested structures", () => {
		const parser = new StreamingJsonParser();
		parser.append('{"a": {"b": [{"c": "');
		expect(parser.parsed).toEqual({ a: { b: [{ c: "" }] } });
		parser.append("val");
		expect(parser.parsed).toEqual({ a: { b: [{ c: "val" }] } });
		parser.append('"}]} }');
		expect(parser.parsed).toEqual({ a: { b: [{ c: "val" }] } });
	});
});
