type StackItem = { type: "object"; obj: any; key: string | null } | { type: "array"; arr: any[] };

type State = number;
const State = {
	EXPECT_VALUE: 0,
	EXPECT_OBJ_KEY: 1,
	EXPECT_COLON: 2,
	EXPECT_COMMA_OR_OBJ_END: 3,
	EXPECT_COMMA_OR_ARR_END: 4,
	IN_STRING: 5,
	IN_NUMBER: 6,
	IN_LITERAL: 7,
	DONE: 8,
} as const;

export class StreamingJsonParser<T = any> {
	private buffer = "";
	private index = 0;

	private root: any = undefined;
	private hasRoot = false;

	private stack: StackItem[] = [];
	private state: State = State.EXPECT_VALUE;

	private strBuf = "";
	private strTarget: "key" | "value" = "value";
	private isEscape = false;
	private unicodeBuf = "";

	private numBuf = "";
	private litBuf = "";

	public get parsed(): T {
		return (this.hasRoot ? this.root : {}) as T;
	}

	public append(delta: string): void {
		this.buffer += delta;
		this.parseIncremental();
	}

	private pushValue(val: any) {
		if (this.stack.length === 0) {
			this.root = val;
			this.hasRoot = true;
		} else {
			const top = this.stack[this.stack.length - 1];
			if (top.type === "object") {
				top.obj[top.key!] = val;
			} else {
				top.arr.push(val);
			}
		}
	}

	private updateValue(val: any) {
		if (this.stack.length === 0) {
			this.root = val;
		} else {
			const top = this.stack[this.stack.length - 1];
			if (top.type === "object") {
				top.obj[top.key!] = val;
			} else {
				top.arr[top.arr.length - 1] = val;
			}
		}
	}

	private afterValue() {
		if (this.stack.length === 0) {
			this.state = State.DONE;
		} else {
			const top = this.stack[this.stack.length - 1];
			if (top.type === "object") {
				this.state = State.EXPECT_COMMA_OR_OBJ_END;
			} else {
				this.state = State.EXPECT_COMMA_OR_ARR_END;
			}
		}
	}

	private parseIncremental() {
		while (this.index < this.buffer.length) {
			const c = this.buffer[this.index];

			switch (this.state) {
				case State.EXPECT_VALUE: {
					if (c === " " || c === "\n" || c === "\r" || c === "\t") {
						this.index++;
						break;
					}
					if (c === "{") {
						const obj = {};
						this.pushValue(obj);
						this.stack.push({ type: "object", obj, key: null });
						this.state = State.EXPECT_OBJ_KEY;
						this.index++;
					} else if (c === "[") {
						const arr: any[] = [];
						this.pushValue(arr);
						this.stack.push({ type: "array", arr });
						this.state = State.EXPECT_VALUE;
						this.index++;
					} else if (c === '"') {
						this.strBuf = "";
						this.strTarget = "value";
						this.isEscape = false;
						this.unicodeBuf = "";
						this.pushValue("");
						this.state = State.IN_STRING;
						this.index++;
					} else if (c === "-" || (c >= "0" && c <= "9")) {
						this.numBuf = c;
						this.pushValue(Number(this.numBuf));
						this.state = State.IN_NUMBER;
						this.index++;
					} else if (c === "t" || c === "f" || c === "n") {
						this.litBuf = c;
						// optimistic
						if (c === "t") this.pushValue(true);
						else if (c === "f") this.pushValue(false);
						else if (c === "n") this.pushValue(null);

						this.state = State.IN_LITERAL;
						this.index++;
					} else if (c === "]") {
						// Empty array case
						if (this.stack.length > 0 && this.stack[this.stack.length - 1].type === "array") {
							this.stack.pop();
							this.afterValue();
							this.index++;
						} else {
							// Syntax error, ignore or throw
							this.index++;
						}
					} else {
						// Ignore invalid char
						this.index++;
					}
					break;
				}
				case State.EXPECT_OBJ_KEY: {
					if (c === " " || c === "\n" || c === "\r" || c === "\t") {
						this.index++;
						break;
					}
					if (c === '"') {
						this.strBuf = "";
						this.strTarget = "key";
						this.isEscape = false;
						this.unicodeBuf = "";
						this.state = State.IN_STRING;
						this.index++;
					} else if (c === "}") {
						this.stack.pop();
						this.afterValue();
						this.index++;
					} else {
						this.index++;
					}
					break;
				}
				case State.EXPECT_COLON: {
					if (c === " " || c === "\n" || c === "\r" || c === "\t") {
						this.index++;
						break;
					}
					if (c === ":") {
						this.state = State.EXPECT_VALUE;
						this.index++;
					} else {
						this.index++;
					}
					break;
				}
				case State.EXPECT_COMMA_OR_OBJ_END: {
					if (c === " " || c === "\n" || c === "\r" || c === "\t") {
						this.index++;
						break;
					}
					if (c === ",") {
						this.state = State.EXPECT_OBJ_KEY;
						this.index++;
					} else if (c === "}") {
						this.stack.pop();
						this.afterValue();
						this.index++;
					} else {
						this.index++;
					}
					break;
				}
				case State.EXPECT_COMMA_OR_ARR_END: {
					if (c === " " || c === "\n" || c === "\r" || c === "\t") {
						this.index++;
						break;
					}
					if (c === ",") {
						this.state = State.EXPECT_VALUE;
						this.index++;
					} else if (c === "]") {
						this.stack.pop();
						this.afterValue();
						this.index++;
					} else {
						this.index++;
					}
					break;
				}
				case State.IN_STRING: {
					if (this.isEscape) {
						if (this.unicodeBuf.length !== 0 || c === "u") {
							if (c === "u" && this.unicodeBuf.length === 0) {
								this.unicodeBuf = "u";
							} else {
								this.unicodeBuf += c;
								if (this.unicodeBuf.length === 5) {
									this.strBuf += String.fromCharCode(parseInt(this.unicodeBuf.slice(1), 16));
									this.isEscape = false;
									this.unicodeBuf = "";
									if (this.strTarget === "value") this.updateValue(this.strBuf);
								}
							}
						} else {
							if (c === '"') this.strBuf += '"';
							else if (c === "\\") this.strBuf += "\\";
							else if (c === "/") this.strBuf += "/";
							else if (c === "b") this.strBuf += "\b";
							else if (c === "f") this.strBuf += "\f";
							else if (c === "n") this.strBuf += "\n";
							else if (c === "r") this.strBuf += "\r";
							else if (c === "t") this.strBuf += "\t";
							else this.strBuf += `\\${c}`;
							this.isEscape = false;
							if (this.strTarget === "value") this.updateValue(this.strBuf);
						}
					} else {
						if (c === "\\") {
							this.isEscape = true;
						} else if (c === '"') {
							if (this.strTarget === "key") {
								const top = this.stack[this.stack.length - 1];
								if (top.type === "object") {
									top.key = this.strBuf;
									// initialize value to null
									top.obj[top.key] = null;
								}
								this.state = State.EXPECT_COLON;
							} else {
								this.updateValue(this.strBuf);
								this.afterValue();
							}
						} else {
							this.strBuf += c;
							if (this.strTarget === "value") this.updateValue(this.strBuf);
						}
					}
					this.index++;
					break;
				}
				case State.IN_NUMBER: {
					if ((c >= "0" && c <= "9") || c === "." || c === "e" || c === "E" || c === "+" || c === "-") {
						this.numBuf += c;
						const num = Number(this.numBuf);
						if (!Number.isNaN(num)) {
							this.updateValue(num);
						}
						this.index++;
					} else {
						this.afterValue();
						// DO NOT increment index, let the new state handle `c`
					}
					break;
				}
				case State.IN_LITERAL: {
					if (c >= "a" && c <= "z") {
						this.litBuf += c;
						// check if complete
						if (this.litBuf === "true") {
							this.updateValue(true);
						} else if (this.litBuf === "false") {
							this.updateValue(false);
						} else if (this.litBuf === "null") {
							this.updateValue(null);
						}
						this.index++;
					} else {
						this.afterValue();
					}
					break;
				}
				case State.DONE: {
					this.index++;
					break;
				}
			}
		}
	}
}
