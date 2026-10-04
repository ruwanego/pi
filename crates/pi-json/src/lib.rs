//! Fast path for `parseStreamingJson` in `packages/ai/src/utils/json-parse.ts`.
//!
//! `parseStreamingJson(s)` returns `JSON.parse(s)`, else `JSON.parse(repairJson(s))`, else partial-json's
//! `parse(s)` (which trims `s` first), else partial-json on the repaired text, else `{}`. Streaming tool calls
//! call it with every growing prefix of the arguments, which makes it the main CPU cost of a long tool call.
//!
//! [`analyze`] handles the inputs seen while streaming: complete JSON, or a prefix of a JSON object or array
//! that ends inside a string, after a key, a colon, a value or a comma. For those, `repairJson(s) === s` and
//! partial-json returns every complete member plus the partial string being streamed, so the result can be
//! written as complete JSON text and parsed by V8's `JSON.parse`. Anything else (text that needs repair, an input
//! ending inside a number, literal or escape, a `"__proto__"` key, partial-json's quirks such as `[ ]`) is
//! reported as [`Analysis::Fallback`] so the caller runs the original implementation.

/// How to produce `parseStreamingJson(input)`.
#[derive(Debug, PartialEq)]
pub enum Analysis<'a> {
	/// `JSON.parse(input)` succeeds; return it.
	Complete,
	/// The result is `JSON.parse(skeleton)` with each patch's decoded string assigned at its path.
	Partial { skeleton: String, patches: Vec<Patch<'a>> },
	/// Run the original implementation.
	Fallback,
}

/// A string value left out of the skeleton (written as `null`) because it is long.
#[derive(Debug, PartialEq)]
pub struct Patch<'a> {
	pub path: Vec<Step<'a>>,
	/// The string's JSON source between the quotes, escapes intact; decode with [`decode_string`].
	pub raw: &'a str,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Step<'a> {
	/// An object key, as JSON source between the quotes.
	Key(&'a str),
	Index(u32),
}

/// Strings longer than this many source bytes are patched in instead of going through the skeleton.
pub const PATCH_THRESHOLD: usize = 1024;

/// Deeper input falls back rather than risking deep recursion.
const MAX_DEPTH: usize = 512;

pub fn analyze(input: &str) -> Analysis<'_> {
	// The caller receives text from V8 with lone surrogates replaced by U+FFFD; such text is not the original.
	if input.contains('\u{fffd}') {
		return Analysis::Fallback;
	}
	let trimmed = input.trim_matches(is_js_whitespace);
	let offset = input.len() - input.trim_start_matches(is_js_whitespace).len();
	if trimmed.is_empty() {
		return Analysis::Fallback;
	}
	let mut parser = Parser {
		src: trimmed,
		bytes: trimmed.as_bytes(),
		index: 0,
		out: String::with_capacity(64),
		path: Vec::new(),
		patches: Vec::new(),
	};
	let end = match parser.root() {
		Ok(end) => end,
		Err(Fallback) => return Analysis::Fallback,
	};
	match end {
		End::Closed => {
			// `JSON.parse` only skips JSON whitespace around the value.
			let json_whitespace = |text: &str| text.bytes().all(|byte| matches!(byte, b' ' | b'\t' | b'\n' | b'\r'));
			let after = offset + trimmed.len();
			if parser.index == trimmed.len() && json_whitespace(&input[..offset]) && json_whitespace(&input[after..]) {
				Analysis::Complete
			} else {
				Analysis::Fallback
			}
		}
		End::Eof => Analysis::Partial {
			skeleton: parser.out,
			patches: parser.patches,
		},
		// Only values inside a container are dropped.
		End::Dropped => Analysis::Fallback,
	}
}

/// `String.prototype.trim`'s WhiteSpace and LineTerminator code points.
fn is_js_whitespace(c: char) -> bool {
	matches!(
		c,
		'\u{9}'..='\u{d}'
			| ' ' | '\u{a0}'
			| '\u{1680}'
			| '\u{2000}'..='\u{200a}'
			| '\u{2028}'
			| '\u{2029}'
			| '\u{202f}'
			| '\u{205f}'
			| '\u{3000}'
			| '\u{feff}'
	)
}

#[derive(Debug)]
struct Fallback;

/// Whether a value was closed or the input ended inside it.
#[derive(Debug, Clone, Copy, PartialEq)]
enum End {
	Closed,
	Eof,
	/// The input ended inside a value that partial-json leaves out of its container.
	Dropped,
}

struct Parser<'a> {
	src: &'a str,
	bytes: &'a [u8],
	index: usize,
	out: String,
	path: Vec<Step<'a>>,
	patches: Vec<Patch<'a>>,
}

impl<'a> Parser<'a> {
	fn peek(&self) -> Option<u8> {
		self.bytes.get(self.index).copied()
	}

	fn skip_whitespace(&mut self) {
		while let Some(b' ' | b'\t' | b'\n' | b'\r') = self.peek() {
			self.index += 1;
		}
	}

	fn root(&mut self) -> Result<End, Fallback> {
		match self.peek() {
			Some(b'{') => self.object(0),
			Some(b'[') => self.array(0),
			_ => Err(Fallback),
		}
	}

	/// Parses a value that starts at the current index (whitespace already skipped, input not exhausted).
	fn value(&mut self, depth: usize) -> Result<End, Fallback> {
		match self.peek() {
			Some(b'{') => self.object(depth + 1),
			Some(b'[') => self.array(depth + 1),
			Some(b'"') => {
				let (raw, end) = self.string()?;
				self.emit_string(raw);
				Ok(end)
			}
			Some(b't') => self.literal("true"),
			Some(b'f') => self.literal("false"),
			Some(b'n') => self.literal("null"),
			Some(b'-' | b'0'..=b'9') => self.number(),
			_ => Err(Fallback),
		}
	}

	fn object(&mut self, depth: usize) -> Result<End, Fallback> {
		if depth > MAX_DEPTH {
			return Err(Fallback);
		}
		self.index += 1;
		self.out.push('{');
		let mut first = true;
		loop {
			self.skip_whitespace();
			match self.peek() {
				None => return Ok(self.close('}')),
				Some(b'}') if first => {
					self.index += 1;
					self.out.push('}');
					return Ok(End::Closed);
				}
				Some(b'"') => {}
				_ => return Err(Fallback),
			}
			let (key, key_end) = self.string()?;
			// partial-json assigns members with `obj[key] = value`, which treats "__proto__" specially.
			if key_end == End::Eof || decode_string(key).is_none_or(|decoded| decoded.is_proto()) {
				return if key_end == End::Eof {
					Ok(self.close('}'))
				} else {
					Err(Fallback)
				};
			}
			self.skip_whitespace();
			match self.peek() {
				None => return Ok(self.close('}')),
				Some(b':') => self.index += 1,
				_ => return Err(Fallback),
			}
			self.skip_whitespace();
			if self.peek().is_none() {
				return Ok(self.close('}'));
			}
			let mark = self.out.len();
			if !first {
				self.out.push(',');
			}
			self.out.push('"');
			self.out.push_str(key);
			self.out.push_str("\":");
			self.path.push(Step::Key(key));
			let end = self.value(depth)?;
			self.path.pop();
			if end == End::Dropped {
				self.out.truncate(mark);
				return Ok(self.close('}'));
			}
			first = false;
			if end == End::Eof {
				return Ok(self.close('}'));
			}
			self.skip_whitespace();
			match self.peek() {
				None => return Ok(self.close('}')),
				Some(b'}') => {
					self.index += 1;
					self.out.push('}');
					return Ok(End::Closed);
				}
				Some(b',') => {
					self.index += 1;
					self.skip_whitespace();
					if self.peek() == Some(b'}') {
						return Err(Fallback);
					}
				}
				_ => return Err(Fallback),
			}
		}
	}

	fn array(&mut self, depth: usize) -> Result<End, Fallback> {
		if depth > MAX_DEPTH {
			return Err(Fallback);
		}
		self.index += 1;
		self.out.push('[');
		let opened_at = self.index;
		self.skip_whitespace();
		match self.peek() {
			None => return Ok(self.close(']')),
			Some(b']') => {
				// partial-json does not skip whitespace before the first element, so `[ ]` takes another path.
				if self.index != opened_at {
					return Err(Fallback);
				}
				self.index += 1;
				self.out.push(']');
				return Ok(End::Closed);
			}
			_ => {}
		}
		let mut count = 0u32;
		loop {
			let mark = self.out.len();
			if count > 0 {
				self.out.push(',');
			}
			self.path.push(Step::Index(count));
			let end = self.value(depth)?;
			self.path.pop();
			if end == End::Dropped {
				self.out.truncate(mark);
				return Ok(self.close(']'));
			}
			count += 1;
			if end == End::Eof {
				return Ok(self.close(']'));
			}
			self.skip_whitespace();
			match self.peek() {
				None => return Ok(self.close(']')),
				Some(b']') => {
					self.index += 1;
					self.out.push(']');
					return Ok(End::Closed);
				}
				Some(b',') => {
					self.index += 1;
					self.skip_whitespace();
					match self.peek() {
						None => return Ok(self.close(']')),
						Some(b']') => return Err(Fallback),
						_ => {}
					}
				}
				_ => return Err(Fallback),
			}
		}
	}

	fn close(&mut self, bracket: char) -> End {
		self.out.push(bracket);
		End::Eof
	}

	/// Scans a string starting at its opening quote. Returns the source between the quotes and whether the input
	/// ended inside it. Falls back on anything `JSON.parse` rejects or `repairJson` would change, and on an input
	/// ending inside an escape.
	fn string(&mut self) -> Result<(&'a str, End), Fallback> {
		let start = self.index + 1;
		let mut index = start;
		loop {
			let Some(&byte) = self.bytes.get(index) else {
				self.index = index;
				return Ok((&self.src[start..index], End::Eof));
			};
			match byte {
				b'"' => {
					self.index = index + 1;
					return Ok((&self.src[start..index], End::Closed));
				}
				b'\\' => match self.bytes.get(index + 1) {
					Some(b'"' | b'\\' | b'/' | b'b' | b'f' | b'n' | b'r' | b't') => index += 2,
					Some(b'u') => {
						let digits = &self.bytes[(index + 2).min(self.bytes.len())..(index + 6).min(self.bytes.len())];
						if !digits.iter().all(u8::is_ascii_hexdigit) {
							return Err(Fallback);
						}
						if digits.len() < 4 {
							// Input ends inside `\uXXXX`: partial-json cuts the string at its last backslash.
							self.index = self.bytes.len();
							return Ok((&self.src[start..index], End::Eof));
						}
						index += 6;
					}
					// Input ends with a lone backslash: partial-json drops it.
					None => {
						self.index = self.bytes.len();
						return Ok((&self.src[start..index], End::Eof));
					}
					_ => return Err(Fallback),
				},
				0x00..=0x1f => return Err(Fallback),
				_ => index += 1,
			}
		}
	}

	fn emit_string(&mut self, raw: &'a str) {
		if raw.len() > PATCH_THRESHOLD {
			self.out.push_str("null");
			self.patches.push(Patch {
				path: self.path.clone(),
				raw,
			});
		} else {
			self.out.push('"');
			self.out.push_str(raw);
			self.out.push('"');
		}
	}

	fn literal(&mut self, word: &str) -> Result<End, Fallback> {
		let rest = &self.src[self.index..];
		if rest.len() < word.len() && word.starts_with(rest) {
			// partial-json completes a literal cut off by the end of the input.
			self.index = self.bytes.len();
			self.out.push_str(word);
			return Ok(End::Closed);
		}
		if !rest.starts_with(word) {
			return Err(Fallback);
		}
		self.index += word.len();
		self.out.push_str(word);
		// A complete literal at the end of the input is kept, as partial-json does.
		if self.peek().is_none() {
			return Ok(End::Closed);
		}
		self.delimited()
	}

	/// A JSON number. At the end of the input, partial-json keeps a complete number. For an incomplete one (`-`,
	/// `1.`, `1e+`) its `parseNum` retries `JSON.parse` on the text before the input's last `e`, which keeps the
	/// mantissa of a number with a lowercase exponent, and otherwise drops the value: the other retry parses text
	/// before the number, which is never valid JSON inside an object or array.
	fn number(&mut self) -> Result<End, Fallback> {
		let start = self.index;
		let end = number_end(self.bytes, start);
		if end < self.bytes.len() {
			if !is_json_number(&self.bytes[start..end]) {
				return Err(Fallback);
			}
			self.out.push_str(&self.src[start..end]);
			self.index = end;
			return self.delimited();
		}
		// The number runs to the end of the input; partial-json reads everything up to `,`, `]` or `}`.
		let text = &self.src[start..];
		if !text
			.bytes()
			.all(|byte| matches!(byte, b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E'))
		{
			return Err(Fallback);
		}
		self.index = self.bytes.len();
		if is_json_number(text.as_bytes()) {
			self.out.push_str(text);
			return Ok(End::Closed);
		}
		match text.rfind('e') {
			Some(e) if text != "-" && is_json_number(&text.as_bytes()[..e]) => {
				self.out.push_str(&text[..e]);
				Ok(End::Closed)
			}
			_ => Ok(End::Dropped),
		}
	}

	/// After a number or literal: the next byte must end it.
	fn delimited(&mut self) -> Result<End, Fallback> {
		match self.peek() {
			Some(b' ' | b'\t' | b'\n' | b'\r' | b',' | b']' | b'}') => Ok(End::Closed),
			_ => Err(Fallback),
		}
	}
}

/// End of the run of number characters starting at `start`.
fn number_end(bytes: &[u8], start: usize) -> usize {
	let mut end = start;
	while let Some(b'0'..=b'9' | b'-' | b'+' | b'.' | b'e' | b'E') = bytes.get(end) {
		end += 1;
	}
	end
}

/// Whether `text` is exactly a JSON number.
fn is_json_number(text: &[u8]) -> bool {
	let mut index = 0;
	let digits = |index: &mut usize| {
		let from = *index;
		while let Some(b'0'..=b'9') = text.get(*index) {
			*index += 1;
		}
		*index - from
	};
	if text.get(index) == Some(&b'-') {
		index += 1;
	}
	let integer_start = index;
	let integer = digits(&mut index);
	if integer == 0 || (integer > 1 && text[integer_start] == b'0') {
		return false;
	}
	if text.get(index) == Some(&b'.') {
		index += 1;
		if digits(&mut index) == 0 {
			return false;
		}
	}
	if let Some(b'e' | b'E') = text.get(index) {
		index += 1;
		if let Some(b'+' | b'-') = text.get(index) {
			index += 1;
		}
		if digits(&mut index) == 0 {
			return false;
		}
	}
	index == text.len()
}

/// A decoded JSON string.
#[derive(Debug, PartialEq)]
pub enum Decoded {
	Utf8(String),
	/// Contains a lone surrogate from a `\u` escape, which UTF-8 cannot hold.
	Utf16(Vec<u16>),
}

impl Decoded {
	fn is_proto(&self) -> bool {
		matches!(self, Decoded::Utf8(text) if text == "__proto__")
	}
}

/// Decodes the source of a JSON string (between the quotes) as `JSON.parse` would. Returns `None` for invalid
/// source, which [`analyze`] never produces.
pub fn decode_string(raw: &str) -> Option<Decoded> {
	let bytes = raw.as_bytes();
	let mut out = String::with_capacity(raw.len());
	let mut run_start = 0;
	let mut index = 0;
	while index < bytes.len() {
		if bytes[index] != b'\\' {
			index += 1;
			continue;
		}
		out.push_str(&raw[run_start..index]);
		let escape = *bytes.get(index + 1)?;
		index += 2;
		match escape {
			b'"' => out.push('"'),
			b'\\' => out.push('\\'),
			b'/' => out.push('/'),
			b'b' => out.push('\u{8}'),
			b'f' => out.push('\u{c}'),
			b'n' => out.push('\n'),
			b'r' => out.push('\r'),
			b't' => out.push('\t'),
			b'u' => {
				let unit = hex4(bytes.get(index..index + 4)?)?;
				index += 4;
				let character = if (0xd800..0xdc00).contains(&unit) {
					// A high surrogate must be followed by an escaped low surrogate to form a character.
					let low = match bytes.get(index..index + 6) {
						Some([b'\\', b'u', digits @ ..]) => hex4(digits).filter(|low| (0xdc00..0xe000).contains(low)),
						_ => None,
					};
					let Some(low) = low else {
						return decode_utf16(raw);
					};
					index += 6;
					char::from_u32(0x10000 + ((unit as u32 - 0xd800) << 10) + (low as u32 - 0xdc00))?
				} else {
					match char::from_u32(unit as u32) {
						Some(character) => character,
						None => return decode_utf16(raw),
					}
				};
				out.push(character);
			}
			_ => return None,
		}
		run_start = index;
	}
	out.push_str(&raw[run_start..]);
	Some(Decoded::Utf8(out))
}

fn hex4(digits: &[u8]) -> Option<u16> {
	let digits = std::str::from_utf8(digits).ok()?;
	if digits.len() != 4 || !digits.bytes().all(|byte| byte.is_ascii_hexdigit()) {
		return None;
	}
	u16::from_str_radix(digits, 16).ok()
}

/// Slow path for strings with lone surrogates: decode to UTF-16 code units.
fn decode_utf16(raw: &str) -> Option<Decoded> {
	let mut units: Vec<u16> = Vec::with_capacity(raw.len());
	let mut chars = raw.chars();
	while let Some(c) = chars.next() {
		if c != '\\' {
			let mut buffer = [0u16; 2];
			units.extend_from_slice(c.encode_utf16(&mut buffer));
			continue;
		}
		let unit = match chars.next()? {
			'"' => b'"' as u16,
			'\\' => b'\\' as u16,
			'/' => b'/' as u16,
			'b' => 0x08,
			'f' => 0x0c,
			'n' => b'\n' as u16,
			'r' => b'\r' as u16,
			't' => b'\t' as u16,
			'u' => {
				let hex: String = chars.by_ref().take(4).collect();
				hex4(hex.as_bytes())?
			}
			_ => return None,
		};
		units.push(unit);
	}
	Some(match String::from_utf16(&units) {
		Ok(text) => Decoded::Utf8(text),
		Err(_) => Decoded::Utf16(units),
	})
}

#[cfg(test)]
mod tests {
	use super::*;

	fn partial(input: &str) -> String {
		match analyze(input) {
			Analysis::Partial { skeleton, patches } => {
				assert!(patches.is_empty());
				skeleton
			}
			other => panic!("expected a partial result for {input:?}, got {other:?}"),
		}
	}

	#[test]
	fn complete_json() {
		assert_eq!(analyze(r#"{"a":1,"b":[true,null,"x"]}"#), Analysis::Complete);
		assert_eq!(analyze(" \n{\"a\": -1.5e3}\t"), Analysis::Complete);
		assert_eq!(analyze("[]"), Analysis::Complete);
	}

	#[test]
	fn closes_prefixes_like_partial_json() {
		assert_eq!(partial("{"), "{}");
		assert_eq!(partial(r#"{"pa"#), "{}");
		assert_eq!(partial(r#"{"path""#), "{}");
		assert_eq!(partial(r#"{"path":"#), "{}");
		assert_eq!(partial(r#"{"path": ""#), r#"{"path":""}"#);
		assert_eq!(
			partial(r#"{"path":"src/a.ts","content":"line\n"#),
			r#"{"path":"src/a.ts","content":"line\n"}"#
		);
		assert_eq!(partial(r#"{"a":1,"#), r#"{"a":1}"#);
		assert_eq!(partial(r#"{"a":{"b":["x","y"#), r#"{"a":{"b":["x","y"]}}"#);
		assert_eq!(partial(r#"["a", {"b": true"#), r#"["a",{"b":true}]"#);
		assert_eq!(partial(r#"{"a":tr"#), r#"{"a":true}"#);
		assert_eq!(partial(r#"{"a":n"#), r#"{"a":null}"#);
		assert_eq!(partial(r#"{"a":-1.5e3"#), r#"{"a":-1.5e3}"#);
		assert_eq!(partial(r#"{"a":1,"b":-"#), r#"{"a":1}"#);
		assert_eq!(partial(r#"{"a":1."#), "{}");
		assert_eq!(partial(r#"{"a":1.5e"#), r#"{"a":1.5}"#);
		assert_eq!(partial(r#"{"a":1.5e+"#), r#"{"a":1.5}"#);
		assert_eq!(partial(r#"{"a":1.5E"#), "{}");
		assert_eq!(partial(r#"[1,2."#), "[1]");
		assert_eq!(partial(r#"{"a":"x\"#), r#"{"a":"x"}"#);
		assert_eq!(partial(r#"{"a":"x\u12"#), r#"{"a":"x"}"#);
		assert_eq!(partial(r#"{"a":"x\n\u"#), r#"{"a":"x\n"}"#);
		// partial-json trims the input, including whitespace inside an unterminated string.
		assert_eq!(partial("{\"a\":\"text  \u{a0}"), r#"{"a":"text"}"#);
	}

	#[test]
	fn falls_back_on_inputs_the_original_handles_differently() {
		for input in [
			"",
			"   ",
			"12",
			r#""str"#,
			r#"{"a":Na"#,
			r#"{"a":-I"#,
			r#"{"a":1x"#,
			"{\"a\":\"raw\ncontrol\"}",
			r#"{"a":"bad \q escape"}"#,
			r#"{"__proto__":{"x":1}}"#,
			r#"{"__proto__":1}"#,
			r#"{"a":[ ]}"#,
			r#"{"a":1,}"#,
			r#"{"a":01}"#,
			r#"{"a":1}}"#,
			"\u{a0}{}",
			"{\"a\":\"\u{fffd}\"}",
		] {
			assert_eq!(analyze(input), Analysis::Fallback, "{input:?}");
		}
	}

	#[test]
	fn long_strings_become_patches() {
		let content = "x\\n".repeat(PATCH_THRESHOLD);
		let input = format!(r#"{{"path":"a","content":"{content}"#);
		let Analysis::Partial { skeleton, patches } = analyze(&input) else {
			panic!("expected partial");
		};
		assert_eq!(skeleton, r#"{"path":"a","content":null}"#);
		assert_eq!(
			patches,
			[Patch {
				path: vec![Step::Key("content")],
				raw: &content
			}]
		);
	}

	#[test]
	fn decodes_strings_like_json_parse() {
		assert_eq!(decode_string("plain"), Some(Decoded::Utf8("plain".into())));
		assert_eq!(
			decode_string(r#"a\n\t\"\\\/\u00e9\ud83d\ude00"#),
			Some(Decoded::Utf8("a\n\t\"\\/\u{e9}\u{1f600}".into()))
		);
		assert_eq!(
			decode_string(r"\ud800x"),
			Some(Decoded::Utf16(vec![0xd800, b'x' as u16]))
		);
		assert_eq!(
			decode_string(r"x\udc00"),
			Some(Decoded::Utf16(vec![b'x' as u16, 0xdc00]))
		);
		assert_eq!(decode_string(r"\ud83d\u0041"), Some(Decoded::Utf16(vec![0xd83d, 0x41])));
		assert_eq!(
			decode_string(r"\u0000\u001F\uFFFF\b\f\r"),
			Some(Decoded::Utf8("\0\u{1f}\u{ffff}\u{8}\u{c}\r".into()))
		);
	}
}
