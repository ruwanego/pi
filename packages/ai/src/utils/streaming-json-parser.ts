import { parseStreamingJson } from "./json-parse.ts";

type StreamingJsonDelta =
	| [0, string, (string | number)[][]] // Replace skeleton: skeleton, activePaths
	| [1, (string | number)[], string] // Append string: path, suffix
	| [2] // Fallback
	| [3, (string | number)[], string]; // Set string: path, text

export interface NativeStreamingJsonParser {
	append(chunk: string): string;
}

let nativeFactory: (() => NativeStreamingJsonParser) | undefined;

export function setNativeStreamingJsonParser(factory: (() => NativeStreamingJsonParser) | undefined) {
	nativeFactory = factory;
}

export class StreamingJsonParser<T = any> {
	private parsedAst: T = {} as T;
	private nativeParser?: NativeStreamingJsonParser;
	private buffer = "";
	private hasFallback = false;

	constructor() {
		this.nativeParser = nativeFactory?.();
	}

	public append(delta: string): void {
		this.buffer += delta;

		if (this.hasFallback || !this.nativeParser) {
			this.parsedAst = parseStreamingJson(this.buffer);
			return;
		}

		let opsStr: string;
		try {
			opsStr = this.nativeParser.append(delta);
		} catch (_error) {
			this.hasFallback = true;
			this.parsedAst = parseStreamingJson(this.buffer);
			return;
		}

		const ops = JSON.parse(opsStr) as StreamingJsonDelta[];
		for (const op of ops) {
			if (op[0] === 2) {
				this.hasFallback = true;
				this.parsedAst = parseStreamingJson(this.buffer);
				return;
			} else if (op[0] === 0) {
				const newAst = JSON.parse(op[1]);
				const activePaths = op[2];
				if (activePaths) {
					for (const path of activePaths) {
						let oldCurr: any = this.parsedAst;
						let newCurr: any = newAst;
						for (let i = 0; i < path.length - 1; i++) {
							if (oldCurr && typeof oldCurr === "object" && newCurr && typeof newCurr === "object") {
								oldCurr = oldCurr[path[i]];
								newCurr = newCurr[path[i]];
							} else {
								oldCurr = undefined;
								break;
							}
						}
						if (oldCurr && typeof oldCurr === "object" && newCurr && typeof newCurr === "object") {
							const last = path[path.length - 1];
							if (oldCurr[last] !== undefined) {
								newCurr[last] = oldCurr[last];
							}
						}
					}
				}
				this.parsedAst = newAst;
			} else if (op[0] === 1 || op[0] === 3) {
				const path = op[1];
				let current: any = this.parsedAst;
				for (let i = 0; i < path.length - 1; i++) {
					if (current && typeof current === "object") {
						current = current[path[i]];
					}
				}
				const last = path[path.length - 1];
				if (current && typeof current === "object") {
					if (op[0] === 1) {
						current[last] = (current[last] || "") + op[2];
					} else {
						current[last] = op[2];
					}
				}
			}
		}
	}

	public get parsed(): T {
		return this.parsedAst;
	}
}
