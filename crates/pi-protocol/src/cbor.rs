//! The protocol's strict, definite-length RFC 8949 subset.
//!
//! Mirrors `packages/protocol/src/cbor`. Numbers follow JavaScript semantics: integral values other than
//! `-0` are encoded as CBOR integers and must be safe integers; everything else is a float64.

use std::collections::HashSet;
use std::fmt;

pub const DEFAULT_MAX_CBOR_BYTE_LENGTH: usize = 16 * 1024 * 1024;
pub const DEFAULT_MAX_CBOR_CONTAINER_LENGTH: usize = 1_000_000;
pub const DEFAULT_MAX_CBOR_DEPTH: usize = 64;
pub const MAX_UINT32: u64 = 0xffff_ffff;
pub const MAX_CONFIGURED_DEPTH: usize = 512;
/// `Number.MAX_SAFE_INTEGER`.
pub const MAX_SAFE_INTEGER: f64 = 9_007_199_254_740_991.0;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CborError(pub String);

impl fmt::Display for CborError {
	fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for CborError {}

fn error<T>(message: impl Into<String>) -> Result<T, CborError> {
	Err(CborError(message.into()))
}

/// Resolved limits. Validation of caller input happens in `Limits::new`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Limits {
	pub max_byte_length: usize,
	pub max_container_length: usize,
	pub max_depth: usize,
}

impl Default for Limits {
	fn default() -> Self {
		Self {
			max_byte_length: DEFAULT_MAX_CBOR_BYTE_LENGTH,
			max_container_length: DEFAULT_MAX_CBOR_CONTAINER_LENGTH,
			max_depth: DEFAULT_MAX_CBOR_DEPTH,
		}
	}
}

impl Limits {
	/// Matches `resolveOptions` range checks; the error is the `RangeError` message.
	pub fn new(max_byte_length: u64, max_container_length: u64, max_depth: u64) -> Result<Self, String> {
		fn check(name: &str, value: u64, maximum: u64) -> Result<usize, String> {
			if value > maximum {
				return Err(format!("{name} must be an integer between 0 and {maximum}"));
			}
			Ok(value as usize)
		}
		Ok(Self {
			max_byte_length: check("maxByteLength", max_byte_length, MAX_UINT32)?,
			max_container_length: check("maxContainerLength", max_container_length, MAX_UINT32)?,
			max_depth: check("maxDepth", max_depth, MAX_CONFIGURED_DEPTH as u64)?,
		})
	}
}

/// A decoded or encodable value. `Number` carries JavaScript number semantics.
#[derive(Debug, Clone, PartialEq)]
pub enum Value {
	Null,
	Bool(bool),
	Number(f64),
	Bytes(Vec<u8>),
	Text(String),
	Array(Vec<Value>),
	Map(Vec<(String, Value)>),
}

/// A decoded value that borrows its strings and byte strings from the input.
#[derive(Debug, Clone, PartialEq)]
pub enum ValueRef<'a> {
	Null,
	Bool(bool),
	Number(f64),
	Bytes(&'a [u8]),
	Text(&'a str),
	Array(Vec<ValueRef<'a>>),
	Map(Vec<(&'a str, ValueRef<'a>)>),
}

impl ValueRef<'_> {
	pub fn to_value(&self) -> Value {
		match self {
			ValueRef::Null => Value::Null,
			ValueRef::Bool(value) => Value::Bool(*value),
			ValueRef::Number(value) => Value::Number(*value),
			ValueRef::Bytes(value) => Value::Bytes(value.to_vec()),
			ValueRef::Text(value) => Value::Text((*value).to_owned()),
			ValueRef::Array(items) => Value::Array(items.iter().map(ValueRef::to_value).collect()),
			ValueRef::Map(entries) => Value::Map(
				entries
					.iter()
					.map(|(key, entry)| ((*key).to_owned(), entry.to_value()))
					.collect(),
			),
		}
	}
}

/// Maps up to this many entries check for duplicate keys by scanning instead of hashing.
const LINEAR_KEY_CHECK_LIMIT: usize = 16;

/// Bounded output buffer. Every write fails once the configured byte limit would be exceeded.
pub struct Writer {
	buffer: Vec<u8>,
	max_byte_length: usize,
}

impl Writer {
	pub fn new(max_byte_length: usize) -> Self {
		Self {
			buffer: Vec::with_capacity(max_byte_length.min(256)),
			max_byte_length,
		}
	}

	fn ensure_capacity(&self, additional: usize) -> Result<(), CborError> {
		if self.buffer.len() + additional > self.max_byte_length {
			return error(format!(
				"CBOR byte length exceeds configured limit of {}",
				self.max_byte_length
			));
		}
		Ok(())
	}

	pub fn write_byte(&mut self, value: u8) -> Result<(), CborError> {
		self.ensure_capacity(1)?;
		self.buffer.push(value);
		Ok(())
	}

	pub fn write_bytes(&mut self, bytes: &[u8]) -> Result<(), CborError> {
		self.ensure_capacity(bytes.len())?;
		self.buffer.extend_from_slice(bytes);
		Ok(())
	}

	pub fn write_argument(&mut self, major_type: u8, value: u64) -> Result<(), CborError> {
		let prefix = major_type << 5;
		if value < 24 {
			self.write_byte(prefix | value as u8)
		} else if value <= 0xff {
			self.write_byte(prefix | 24)?;
			self.write_byte(value as u8)
		} else if value <= 0xffff {
			self.write_byte(prefix | 25)?;
			self.write_bytes(&(value as u16).to_be_bytes())
		} else if value <= MAX_UINT32 {
			self.write_byte(prefix | 26)?;
			self.write_bytes(&(value as u32).to_be_bytes())
		} else {
			self.write_byte(prefix | 27)?;
			self.write_bytes(&((value >> 32) as u32).to_be_bytes())?;
			self.write_bytes(&(value as u32).to_be_bytes())
		}
	}

	pub fn write_float64(&mut self, value: f64) -> Result<(), CborError> {
		self.ensure_capacity(9)?;
		self.buffer.push(0xfb);
		self.buffer.extend_from_slice(&value.to_be_bytes());
		Ok(())
	}

	pub fn len(&self) -> usize {
		self.buffer.len()
	}

	pub fn is_empty(&self) -> bool {
		self.buffer.is_empty()
	}

	pub fn finish(self) -> Vec<u8> {
		self.buffer
	}
}

pub fn check_depth(depth: usize, limits: &Limits) -> Result<(), CborError> {
	if depth > limits.max_depth {
		return error(format!(
			"CBOR nesting depth exceeds configured limit of {}",
			limits.max_depth
		));
	}
	Ok(())
}

/// Encodes a JavaScript number.
pub fn encode_number(writer: &mut Writer, value: f64) -> Result<(), CborError> {
	if !value.is_finite() {
		return error("CBOR numbers must be finite");
	}
	let negative_zero = value == 0.0 && value.is_sign_negative();
	if value.fract() == 0.0 && !negative_zero {
		if value.abs() > MAX_SAFE_INTEGER {
			return error("CBOR integers must be safe JavaScript integers");
		}
		if value >= 0.0 {
			writer.write_argument(0, value as u64)
		} else {
			writer.write_argument(1, (-1.0 - value) as u64)
		}
	} else {
		writer.write_float64(value)
	}
}

fn text_length_error<T>(limits: &Limits) -> Result<T, CborError> {
	error(format!(
		"CBOR text string length exceeds configured limit of {}",
		limits.max_byte_length
	))
}

pub fn encode_text(writer: &mut Writer, value: &str, limits: &Limits) -> Result<(), CborError> {
	if value.len() > limits.max_byte_length {
		return text_length_error(limits);
	}
	writer.write_argument(3, value.len() as u64)?;
	writer.write_bytes(value.as_bytes())
}

/// Encodes a JavaScript (UTF-16) string. The length limit is checked against the `TextEncoder` output, in
/// which each lone surrogate becomes a three-byte U+FFFD, before lone surrogates are rejected.
pub fn encode_text_utf16(writer: &mut Writer, units: &[u16], limits: &Limits) -> Result<(), CborError> {
	let mut utf8_length = 0usize;
	let mut valid = true;
	for decoded in char::decode_utf16(units.iter().copied()) {
		match decoded {
			Ok(character) => utf8_length += character.len_utf8(),
			Err(_) => {
				utf8_length += 3;
				valid = false;
			}
		}
	}
	if utf8_length > limits.max_byte_length {
		return text_length_error(limits);
	}
	if !valid {
		return error("CBOR text strings must contain valid Unicode scalar values");
	}
	let text = String::from_utf16(units).expect("validated UTF-16");
	writer.write_argument(3, text.len() as u64)?;
	writer.write_bytes(text.as_bytes())
}

pub fn encode_byte_string(writer: &mut Writer, bytes: &[u8], limits: &Limits) -> Result<(), CborError> {
	if bytes.len() > limits.max_byte_length {
		return error(format!(
			"CBOR byte string length exceeds configured limit of {}",
			limits.max_byte_length
		));
	}
	writer.write_argument(2, bytes.len() as u64)?;
	writer.write_bytes(bytes)
}

pub fn check_array_length(length: usize, limits: &Limits) -> Result<(), CborError> {
	if length > limits.max_container_length {
		return error(format!(
			"CBOR array length exceeds configured limit of {}",
			limits.max_container_length
		));
	}
	Ok(())
}

pub fn check_map_length(length: usize, limits: &Limits) -> Result<(), CborError> {
	if length > limits.max_container_length {
		return error(format!(
			"CBOR map length exceeds configured limit of {}",
			limits.max_container_length
		));
	}
	Ok(())
}

fn encode_value(writer: &mut Writer, value: &Value, limits: &Limits, depth: usize) -> Result<(), CborError> {
	check_depth(depth, limits)?;
	match value {
		Value::Null => writer.write_byte(0xf6),
		Value::Bool(value) => writer.write_byte(if *value { 0xf5 } else { 0xf4 }),
		Value::Number(value) => encode_number(writer, *value),
		Value::Text(value) => encode_text(writer, value, limits),
		Value::Bytes(value) => encode_byte_string(writer, value, limits),
		Value::Array(items) => {
			check_array_length(items.len(), limits)?;
			writer.write_argument(4, items.len() as u64)?;
			for item in items {
				encode_value(writer, item, limits, depth + 1)?;
			}
			Ok(())
		}
		Value::Map(entries) => {
			let mut keys = HashSet::with_capacity(entries.len());
			if !entries.iter().all(|(key, _)| keys.insert(key.as_str())) {
				return error("CBOR map contains a duplicate key");
			}
			check_map_length(entries.len(), limits)?;
			writer.write_argument(5, entries.len() as u64)?;
			for (key, entry) in entries {
				encode_text(writer, key, limits)?;
				encode_value(writer, entry, limits, depth + 1)?;
			}
			Ok(())
		}
	}
}

/// Encodes one value.
pub fn encode(value: &Value, limits: &Limits) -> Result<Vec<u8>, CborError> {
	let mut writer = Writer::new(limits.max_byte_length);
	encode_value(&mut writer, value, limits, 0)?;
	Ok(writer.finish())
}

struct Reader<'a> {
	bytes: &'a [u8],
	offset: usize,
	limits: Limits,
}

impl<'a> Reader<'a> {
	fn read_item(&mut self, depth: usize) -> Result<ValueRef<'a>, CborError> {
		check_depth(depth, &self.limits)?;
		let initial = self.read_byte()?;
		let major_type = initial >> 5;
		let additional_information = initial & 0x1f;

		match major_type {
			0 => Ok(ValueRef::Number(self.read_argument(additional_information)? as f64)),
			1 => {
				let value = -1.0 - self.read_argument(additional_information)? as f64;
				if value < -MAX_SAFE_INTEGER {
					return error("Decoded CBOR integer is outside the safe range");
				}
				Ok(ValueRef::Number(value))
			}
			2 => {
				let length = self.read_length(additional_information, "byte string", self.limits.max_byte_length)?;
				Ok(ValueRef::Bytes(self.read_bytes(length)?))
			}
			3 => {
				let length = self.read_length(additional_information, "text string", self.limits.max_byte_length)?;
				let bytes = self.read_bytes(length)?;
				match std::str::from_utf8(bytes) {
					Ok(text) => Ok(ValueRef::Text(text)),
					Err(_) => error("CBOR text string contains invalid UTF-8"),
				}
			}
			4 => {
				let length = self.read_length(additional_information, "array", self.limits.max_container_length)?;
				let mut result = Vec::new();
				for _ in 0..length {
					result.push(self.read_item(depth + 1)?);
				}
				Ok(ValueRef::Array(result))
			}
			5 => {
				let length = self.read_length(additional_information, "map", self.limits.max_container_length)?;
				let mut result: Vec<(&'a str, ValueRef<'a>)> = Vec::new();
				let mut keys = HashSet::new();
				for _ in 0..length {
					let ValueRef::Text(key) = self.read_item(depth + 1)? else {
						return error("CBOR map keys must be strings");
					};
					let duplicate = if result.len() < LINEAR_KEY_CHECK_LIMIT {
						result.iter().any(|(existing, _)| *existing == key)
					} else {
						if keys.is_empty() {
							keys.extend(result.iter().map(|(existing, _)| *existing));
						}
						!keys.insert(key)
					};
					if duplicate {
						return error("CBOR map contains a duplicate key");
					}
					let value = self.read_item(depth + 1)?;
					result.push((key, value));
				}
				Ok(ValueRef::Map(result))
			}
			6 => error("CBOR tags are not supported"),
			_ => self.read_simple(additional_information),
		}
	}

	fn read_simple(&mut self, additional_information: u8) -> Result<ValueRef<'a>, CborError> {
		match additional_information {
			20 => Ok(ValueRef::Bool(false)),
			21 => Ok(ValueRef::Bool(true)),
			22 => Ok(ValueRef::Null),
			27 => {
				let bytes: [u8; 8] = self.read_bytes(8)?.try_into().expect("eight bytes");
				let value = f64::from_be_bytes(bytes);
				if !value.is_finite() {
					return error("Decoded CBOR number must be finite");
				}
				if value.fract() == 0.0 && value.abs() > MAX_SAFE_INTEGER {
					return error("Decoded CBOR integer is outside the safe range");
				}
				Ok(ValueRef::Number(value))
			}
			31 => error("CBOR break marker is not supported"),
			_ => error("Unsupported CBOR simple value or floating-point width"),
		}
	}

	fn read_length(&mut self, additional_information: u8, kind: &str, limit: usize) -> Result<usize, CborError> {
		if additional_information == 31 {
			return error(format!("Indefinite-length CBOR {kind}s are not supported"));
		}
		let length = self.read_argument(additional_information)?;
		if length > limit as u64 {
			return error(format!("CBOR {kind} length exceeds configured limit of {limit}"));
		}
		Ok(length as usize)
	}

	fn read_argument(&mut self, additional_information: u8) -> Result<u64, CborError> {
		match additional_information {
			0..24 => Ok(additional_information as u64),
			24 => Ok(self.read_byte()? as u64),
			25 => Ok(u16::from_be_bytes(self.read_bytes(2)?.try_into().expect("two bytes")) as u64),
			26 => Ok(u32::from_be_bytes(self.read_bytes(4)?.try_into().expect("four bytes")) as u64),
			27 => {
				let high = self.read_argument(26)?;
				let low = self.read_argument(26)?;
				if high > 0x1f_ffff {
					return error("Decoded CBOR integer or length is outside the safe range");
				}
				Ok((high << 32) | low)
			}
			31 => error("Indefinite-length CBOR items are not supported"),
			_ => error("Malformed CBOR additional information"),
		}
	}

	fn read_byte(&mut self) -> Result<u8, CborError> {
		let Some(&value) = self.bytes.get(self.offset) else {
			return error("Truncated CBOR payload");
		};
		self.offset += 1;
		Ok(value)
	}

	fn read_bytes(&mut self, length: usize) -> Result<&'a [u8], CborError> {
		if length > self.bytes.len() - self.offset {
			return error("Truncated CBOR payload");
		}
		let value = &self.bytes[self.offset..self.offset + length];
		self.offset += length;
		Ok(value)
	}
}

/// Decodes exactly one item.
pub fn decode(bytes: &[u8], limits: &Limits) -> Result<Value, CborError> {
	decode_ref(bytes, limits).map(|value| value.to_value())
}

/// Decodes exactly one item without copying strings or byte strings out of `bytes`.
pub fn decode_ref<'a>(bytes: &'a [u8], limits: &Limits) -> Result<ValueRef<'a>, CborError> {
	if bytes.len() > limits.max_byte_length {
		return error(format!(
			"CBOR byte length exceeds configured limit of {}",
			limits.max_byte_length
		));
	}
	let mut reader = Reader {
		bytes,
		offset: 0,
		limits: *limits,
	};
	let value = reader.read_item(0)?;
	if reader.offset != bytes.len() {
		return error("CBOR payload contains trailing data");
	}
	Ok(value)
}

#[cfg(test)]
mod tests {
	use super::*;

	fn hex(bytes: &[u8]) -> String {
		bytes.iter().map(|byte| format!("{byte:02x}")).collect()
	}

	fn from_hex(hex: &str) -> Vec<u8> {
		(0..hex.len())
			.step_by(2)
			.map(|index| u8::from_str_radix(&hex[index..index + 2], 16).unwrap())
			.collect()
	}

	fn number(value: f64) -> Value {
		Value::Number(value)
	}

	fn text(value: &str) -> Value {
		Value::Text(value.to_owned())
	}

	/// Same vectors as packages/protocol/test/cbor/cbor.test.ts.
	fn known_vectors() -> Vec<(Value, &'static str)> {
		vec![
			(Value::Null, "f6"),
			(Value::Bool(false), "f4"),
			(Value::Bool(true), "f5"),
			(number(0.0), "00"),
			(number(1.0), "01"),
			(number(10.0), "0a"),
			(number(23.0), "17"),
			(number(24.0), "1818"),
			(number(25.0), "1819"),
			(number(100.0), "1864"),
			(number(1000.0), "1903e8"),
			(number(1_000_000.0), "1a000f4240"),
			(number(1_000_000_000_000.0), "1b000000e8d4a51000"),
			(number(MAX_SAFE_INTEGER), "1b001fffffffffffff"),
			(number(-1.0), "20"),
			(number(-10.0), "29"),
			(number(-24.0), "37"),
			(number(-25.0), "3818"),
			(number(-100.0), "3863"),
			(number(-1000.0), "3903e7"),
			(number(-1_000_000.0), "3a000f423f"),
			(number(-MAX_SAFE_INTEGER), "3b001ffffffffffffe"),
			(number(1.1), "fb3ff199999999999a"),
			(number(-0.0), "fb8000000000000000"),
			(Value::Bytes(vec![1, 2, 3, 4]), "4401020304"),
			(text(""), "60"),
			(text("IETF"), "6449455446"),
			(text("ü"), "62c3bc"),
			(text("水"), "63e6b0b4"),
			(text("𐅑"), "64f0908591"),
			(Value::Array(vec![]), "80"),
			(Value::Array(vec![number(1.0), number(2.0), number(3.0)]), "83010203"),
			(
				Value::Array(vec![
					number(1.0),
					Value::Array(vec![number(2.0), number(3.0)]),
					Value::Array(vec![number(4.0), number(5.0)]),
				]),
				"8301820203820405",
			),
			(
				Value::Map(vec![
					("a".into(), number(1.0)),
					("b".into(), Value::Array(vec![number(2.0), number(3.0)])),
				]),
				"a26161016162820203",
			),
		]
	}

	#[test]
	fn encodes_and_decodes_rfc_8949_vectors() {
		let limits = Limits::default();
		for (value, wire) in known_vectors() {
			assert_eq!(hex(&encode(&value, &limits).unwrap()), wire, "encode {value:?}");
			let decoded = decode(&from_hex(wire), &limits).unwrap();
			assert_eq!(decoded, value, "decode {wire}");
			if let (Value::Number(expected), Value::Number(actual)) = (&value, &decoded) {
				assert_eq!(expected.is_sign_negative(), actual.is_sign_negative(), "sign of {wire}");
			}
		}
	}

	#[test]
	fn rejects_unsupported_encoder_values() {
		let limits = Limits::default();
		for value in [f64::NAN, f64::INFINITY, f64::NEG_INFINITY] {
			assert_eq!(
				encode(&number(value), &limits).unwrap_err().0,
				"CBOR numbers must be finite"
			);
		}
		for value in [MAX_SAFE_INTEGER + 1.0, -MAX_SAFE_INTEGER - 1.0] {
			assert_eq!(
				encode(&number(value), &limits).unwrap_err().0,
				"CBOR integers must be safe JavaScript integers"
			);
		}
	}

	#[test]
	fn rejects_lone_surrogates_and_excessive_depth() {
		let limits = Limits::default();
		let mut writer = Writer::new(limits.max_byte_length);
		assert_eq!(
			encode_text_utf16(&mut writer, &[0xd800], &limits).unwrap_err().0,
			"CBOR text strings must contain valid Unicode scalar values"
		);
		let mut too_deep = Value::Null;
		for _ in 0..=DEFAULT_MAX_CBOR_DEPTH {
			too_deep = Value::Array(vec![too_deep]);
		}
		assert!(encode(&too_deep, &limits).unwrap_err().0.contains("depth"));
	}

	#[test]
	fn rejects_invalid_decoder_input() {
		let cases = [
			("", "Truncated CBOR payload"),
			("18", "Truncated CBOR payload"),
			("1c", "Malformed CBOR additional information"),
			("5f", "Indefinite-length CBOR byte strings are not supported"),
			("7f", "Indefinite-length CBOR text strings are not supported"),
			("9f", "Indefinite-length CBOR arrays are not supported"),
			("bf", "Indefinite-length CBOR maps are not supported"),
			("c000", "CBOR tags are not supported"),
			("f7", "Unsupported CBOR simple value or floating-point width"),
			("e0", "Unsupported CBOR simple value or floating-point width"),
			("ff", "CBOR break marker is not supported"),
			("f93c00", "Unsupported CBOR simple value or floating-point width"),
			("fa3f800000", "Unsupported CBOR simple value or floating-point width"),
			("fb7ff0000000000000", "Decoded CBOR number must be finite"),
			("fb7ff8000000000000", "Decoded CBOR number must be finite"),
			("fb3ff00000", "Truncated CBOR payload"),
			("44010203", "Truncated CBOR payload"),
			("636162", "Truncated CBOR payload"),
			("8201", "Truncated CBOR payload"),
			("a16161", "Truncated CBOR payload"),
			("0000", "CBOR payload contains trailing data"),
			("a10102", "CBOR map keys must be strings"),
			("a2616101616102", "CBOR map contains a duplicate key"),
			("61ff", "CBOR text string contains invalid UTF-8"),
			("62c080", "CBOR text string contains invalid UTF-8"),
			("63eda080", "CBOR text string contains invalid UTF-8"),
			(
				"1b0020000000000000",
				"Decoded CBOR integer or length is outside the safe range",
			),
			("3b001fffffffffffff", "Decoded CBOR integer is outside the safe range"),
			("fb4340000000000000", "Decoded CBOR integer is outside the safe range"),
		];
		for (wire, message) in cases {
			assert_eq!(
				decode(&from_hex(wire), &Limits::default()).unwrap_err().0,
				message,
				"{wire}"
			);
		}
	}

	#[test]
	fn preserves_a_leading_bom() {
		assert_eq!(
			decode(&from_hex("63efbbbf"), &Limits::default()).unwrap(),
			text("\u{feff}")
		);
	}

	#[test]
	fn enforces_limits() {
		let strict = Limits {
			max_byte_length: 2,
			max_container_length: 2,
			max_depth: DEFAULT_MAX_CBOR_DEPTH,
		};
		assert!(decode(&from_hex("83010203"), &strict).unwrap_err().0.contains("limit"));
		assert!(decode(&from_hex("626162"), &strict).unwrap_err().0.contains("limit"));
		assert!(encode(&text("ab"), &strict).unwrap_err().0.contains("limit"));
		let mut too_deep = vec![0x81; DEFAULT_MAX_CBOR_DEPTH + 1];
		too_deep.push(0xf6);
		assert!(decode(&too_deep, &Limits::default()).unwrap_err().0.contains("depth"));
		assert_eq!(
			Limits::new(0, 0, 513).unwrap_err(),
			"maxDepth must be an integer between 0 and 512"
		);
	}
}
