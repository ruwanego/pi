//! N-API bindings for `pi-protocol`.
//!
//! The encoder walks JavaScript values with the same operations, in the same order, as
//! `packages/protocol/src/cbor/encoder.ts` (`instanceof Uint8Array`, `Array.isArray`, `Object.getPrototypeOf`,
//! `Object.keys`, property reads that run getters, ...), so it accepts, rejects and throws exactly what the
//! TypeScript encoder does. Protocol failures are thrown as `Error`s whose `code` is `PI_CBOR_ERROR` or
//! `PI_FRAME_ERROR`; the TypeScript side rethrows them as `CborError` / `FrameError` with the same message.
//! Exceptions raised by user code (getters, proxies) propagate unchanged.

#![cfg_attr(test, allow(dead_code))]

use std::ffi::{CString, c_char};
use std::ptr;

use napi::bindgen_prelude::Uint8Array;
use napi::{Env, Error, Status, sys};
use napi::{JsValue, Unknown};
use napi_derive::napi;
use pi_protocol::cbor::{self, CborError, Limits, ValueRef, Writer};
use pi_protocol::framing::{self, FrameError};

const CBOR_ERROR_CODE: &str = "PI_CBOR_ERROR";
const FRAME_ERROR_CODE: &str = "PI_FRAME_ERROR";

enum Failure {
	Cbor(String),
	Frame(String),
	/// A JavaScript exception is pending and must propagate as-is.
	Pending,
	Napi(sys::napi_status),
}

impl From<CborError> for Failure {
	fn from(error: CborError) -> Self {
		Failure::Cbor(error.0)
	}
}

impl From<FrameError> for Failure {
	fn from(error: FrameError) -> Self {
		Failure::Frame(error.0)
	}
}

type Outcome<T> = std::result::Result<T, Failure>;

fn throw(env: sys::napi_env, failure: Failure) -> Error {
	let (code, message) = match failure {
		Failure::Pending => return Error::new(Status::PendingException, String::new()),
		Failure::Napi(status) => {
			return Error::new(
				Status::GenericFailure,
				format!("Node-API call failed with status {status}"),
			);
		}
		Failure::Cbor(message) => (CBOR_ERROR_CODE, message),
		Failure::Frame(message) => (FRAME_ERROR_CODE, message),
	};
	let code = CString::new(code).expect("static code");
	let message = CString::new(message.replace('\0', "\u{fffd}")).expect("NUL-free message");
	// SAFETY: env is the live callback env; both strings outlive the call.
	unsafe { sys::napi_throw_error(env, code.as_ptr(), message.as_ptr()) };
	Error::new(Status::PendingException, String::new())
}

fn check(env: sys::napi_env, status: sys::napi_status) -> Outcome<()> {
	if status == sys::Status::napi_ok {
		return Ok(());
	}
	let mut pending = false;
	// SAFETY: env is the live callback env.
	unsafe { sys::napi_is_exception_pending(env, &mut pending) };
	Err(if pending {
		Failure::Pending
	} else {
		Failure::Napi(status)
	})
}

/// Thin safe wrappers over the Node-API calls the codec needs.
#[derive(Clone, Copy)]
struct Js {
	env: sys::napi_env,
}

impl Js {
	fn type_of(self, value: sys::napi_value) -> Outcome<sys::napi_valuetype> {
		let mut result = 0;
		check(self.env, unsafe { sys::napi_typeof(self.env, value, &mut result) })?;
		Ok(result)
	}

	fn named(self, object: sys::napi_value, name: &str) -> Outcome<sys::napi_value> {
		let name = CString::new(name).expect("static name");
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_get_named_property(self.env, object, name.as_ptr(), &mut result)
		})?;
		Ok(result)
	}

	fn get(self, object: sys::napi_value, key: sys::napi_value) -> Outcome<sys::napi_value> {
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_get_property(self.env, object, key, &mut result)
		})?;
		Ok(result)
	}

	fn element(self, object: sys::napi_value, index: u32) -> Outcome<sys::napi_value> {
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_get_element(self.env, object, index, &mut result)
		})?;
		Ok(result)
	}

	fn call(
		self,
		function: sys::napi_value,
		this: sys::napi_value,
		args: &[sys::napi_value],
	) -> Outcome<sys::napi_value> {
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_call_function(self.env, this, function, args.len(), args.as_ptr(), &mut result)
		})?;
		Ok(result)
	}

	fn strict_equals(self, left: sys::napi_value, right: sys::napi_value) -> Outcome<bool> {
		let mut result = false;
		check(self.env, unsafe {
			sys::napi_strict_equals(self.env, left, right, &mut result)
		})?;
		Ok(result)
	}

	fn bool(self, value: sys::napi_value) -> Outcome<bool> {
		let mut result = false;
		check(self.env, unsafe {
			sys::napi_get_value_bool(self.env, value, &mut result)
		})?;
		Ok(result)
	}

	fn number(self, value: sys::napi_value) -> Outcome<f64> {
		let mut result = 0.0;
		check(self.env, unsafe {
			sys::napi_get_value_double(self.env, value, &mut result)
		})?;
		Ok(result)
	}

	/// `ToNumber`, as used by the `<` comparisons in the TypeScript encoder.
	fn to_number(self, value: sys::napi_value) -> Outcome<f64> {
		let mut coerced = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_coerce_to_number(self.env, value, &mut coerced)
		})?;
		self.number(coerced)
	}

	fn utf16(self, value: sys::napi_value) -> Outcome<Vec<u16>> {
		let mut length = 0;
		check(self.env, unsafe {
			sys::napi_get_value_string_utf16(self.env, value, ptr::null_mut(), 0, &mut length)
		})?;
		let mut units = vec![0u16; length + 1];
		let mut written = 0;
		check(self.env, unsafe {
			sys::napi_get_value_string_utf16(self.env, value, units.as_mut_ptr(), units.len(), &mut written)
		})?;
		units.truncate(written);
		Ok(units)
	}

	/// Length in UTF-16 code units, without copying the string.
	fn utf16_length(self, value: sys::napi_value) -> Outcome<usize> {
		let mut length = 0;
		check(self.env, unsafe {
			sys::napi_get_value_string_utf16(self.env, value, ptr::null_mut(), 0, &mut length)
		})?;
		Ok(length)
	}

	/// Writes the string as UTF-8 into `out`, replacing lone surrogates with U+FFFD. `capacity` must be at least
	/// the UTF-8 length (three bytes per UTF-16 code unit always suffices).
	fn utf8_into(self, value: sys::napi_value, capacity: usize, out: &mut Vec<u8>) -> Outcome<()> {
		out.clear();
		out.reserve(capacity + 1);
		let mut written = 0;
		check(self.env, unsafe {
			sys::napi_get_value_string_utf8(
				self.env,
				value,
				out.as_mut_ptr() as *mut c_char,
				capacity + 1,
				&mut written,
			)
		})?;
		// SAFETY: Node-API initialized `written` bytes (plus a NUL terminator) within the reserved capacity.
		unsafe { out.set_len(written) };
		Ok(())
	}

	/// Creates a string from UTF-8. ASCII uses the Latin-1 constructor (a plain copy); long non-ASCII text is
	/// transcoded to UTF-16 here, which is faster than V8's UTF-8 constructor.
	fn text(self, value: &str) -> Outcome<sys::napi_value> {
		if !value.is_ascii() {
			if value.len() < LONG_TEXT_LENGTH {
				return self.string(value);
			}
			let units: Vec<u16> = value.encode_utf16().collect();
			let mut result = ptr::null_mut();
			check(self.env, unsafe {
				sys::napi_create_string_utf16(self.env, units.as_ptr(), units.len() as isize, &mut result)
			})?;
			return Ok(result);
		}
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_create_string_latin1(
				self.env,
				value.as_ptr() as *const c_char,
				value.len() as isize,
				&mut result,
			)
		})?;
		Ok(result)
	}

	fn string(self, value: &str) -> Outcome<sys::napi_value> {
		let mut result = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_create_string_utf8(
				self.env,
				value.as_ptr() as *const c_char,
				value.len() as isize,
				&mut result,
			)
		})?;
		Ok(result)
	}

	fn instance_of(self, value: sys::napi_value, constructor: sys::napi_value) -> Outcome<bool> {
		let mut result = false;
		check(self.env, unsafe {
			sys::napi_instanceof(self.env, value, constructor, &mut result)
		})?;
		Ok(result)
	}

	fn has_own(self, object: sys::napi_value, key: sys::napi_value) -> Outcome<bool> {
		let mut result = false;
		check(self.env, unsafe {
			sys::napi_has_own_property(self.env, object, key, &mut result)
		})?;
		Ok(result)
	}

	fn array_length(self, array: sys::napi_value) -> Outcome<u32> {
		let mut result = 0;
		check(self.env, unsafe {
			sys::napi_get_array_length(self.env, array, &mut result)
		})?;
		Ok(result)
	}

	fn uint8_bytes<'a>(self, value: sys::napi_value) -> Outcome<Option<&'a [u8]>> {
		let mut is_typed_array = false;
		check(self.env, unsafe {
			sys::napi_is_typedarray(self.env, value, &mut is_typed_array)
		})?;
		if !is_typed_array {
			return Ok(None);
		}
		let mut kind = 0;
		let mut length = 0;
		let mut data = ptr::null_mut();
		check(self.env, unsafe {
			sys::napi_get_typedarray_info(
				self.env,
				value,
				&mut kind,
				&mut length,
				&mut data,
				ptr::null_mut(),
				ptr::null_mut(),
			)
		})?;
		if length == 0 {
			return Ok(Some(&[]));
		}
		// SAFETY: a Uint8Array (or subclass) exposes `length` bytes at `data`, which stay alive and unmodified
		// while this synchronous call holds a handle to it.
		Ok(Some(unsafe { std::slice::from_raw_parts(data as *const u8, length) }))
	}

	fn open_scope(self) -> Outcome<Scope> {
		let mut scope = ptr::null_mut();
		check(self.env, unsafe { sys::napi_open_handle_scope(self.env, &mut scope) })?;
		Ok(Scope { env: self.env, scope })
	}
}

/// Releases handles created inside one container element.
struct Scope {
	env: sys::napi_env,
	scope: sys::napi_handle_scope,
}

impl Drop for Scope {
	fn drop(&mut self) {
		// SAFETY: scopes are dropped in reverse order of creation on the callback thread.
		unsafe { sys::napi_close_handle_scope(self.env, self.scope) };
	}
}

/// Globals captured once per call, mirroring the identifiers the TypeScript encoder references.
struct Realm {
	uint8_array: sys::napi_value,
	array: sys::napi_value,
	array_is_array: sys::napi_value,
	object_prototype: sys::napi_value,
	object_keys: sys::napi_value,
	object_get_prototype_of: sys::napi_value,
	object_get_own_property_symbols: sys::napi_value,
	property_is_enumerable: sys::napi_value,
	object: sys::napi_value,
}

impl Realm {
	fn new(js: Js) -> Outcome<Self> {
		let mut global = ptr::null_mut();
		check(js.env, unsafe { sys::napi_get_global(js.env, &mut global) })?;
		let object = js.named(global, "Object")?;
		let object_prototype = js.named(object, "prototype")?;
		let array = js.named(global, "Array")?;
		Ok(Self {
			uint8_array: js.named(global, "Uint8Array")?,
			array_is_array: js.named(array, "isArray")?,
			array,
			object_keys: js.named(object, "keys")?,
			object_get_prototype_of: js.named(object, "getPrototypeOf")?,
			object_get_own_property_symbols: js.named(object, "getOwnPropertySymbols")?,
			property_is_enumerable: js.named(object_prototype, "propertyIsEnumerable")?,
			object_prototype,
			object,
		})
	}
}

struct Encoder {
	js: Js,
	realm: Realm,
	limits: Limits,
	writer: Writer,
	ancestors: Vec<sys::napi_value>,
	/// Reused UTF-8 output buffer for strings.
	scratch: Vec<u8>,
}

fn type_name(kind: sys::napi_valuetype) -> &'static str {
	match kind {
		sys::ValueType::napi_undefined => "undefined",
		sys::ValueType::napi_boolean => "boolean",
		sys::ValueType::napi_number => "number",
		sys::ValueType::napi_string => "string",
		sys::ValueType::napi_symbol => "symbol",
		sys::ValueType::napi_function => "function",
		sys::ValueType::napi_bigint => "bigint",
		_ => "object",
	}
}

impl Encoder {
	fn is_ancestor(&self, value: sys::napi_value) -> Outcome<bool> {
		for &ancestor in &self.ancestors {
			if self.js.strict_equals(ancestor, value)? {
				return Ok(true);
			}
		}
		Ok(false)
	}

	fn is_plain_object(&self, value: sys::napi_value) -> Outcome<bool> {
		let prototype = self
			.js
			.call(self.realm.object_get_prototype_of, self.realm.object, &[value])?;
		Ok(self.js.strict_equals(prototype, self.realm.object_prototype)?
			|| self.js.type_of(prototype)? == sys::ValueType::napi_null)
	}

	fn length(&self, array: sys::napi_value) -> Outcome<f64> {
		self.js.to_number(self.js.named(array, "length")?)
	}

	/// V8 writes UTF-8 directly, replacing lone surrogates with U+FFFD. Output without a 0xEF byte cannot
	/// contain U+FFFD, so it is exact; otherwise the UTF-16 path decides between a real U+FFFD and a lone
	/// surrogate, with the TypeScript encoder's error order.
	fn encode_string(&mut self, value: sys::napi_value) -> Outcome<()> {
		let units = self.js.utf16_length(value)?;
		self.js.utf8_into(value, units * 3, &mut self.scratch)?;
		if self.scratch.contains(&0xef) {
			let units = self.js.utf16(value)?;
			cbor::encode_text_utf16(&mut self.writer, &units, &self.limits)?;
			return Ok(());
		}
		// SAFETY: V8 writes well-formed UTF-8; without U+FFFD there were no lone surrogates.
		let text = unsafe { std::str::from_utf8_unchecked(&self.scratch) };
		cbor::encode_text(&mut self.writer, text, &self.limits)?;
		Ok(())
	}

	fn encode_value(&mut self, value: sys::napi_value, depth: usize) -> Outcome<()> {
		cbor::check_depth(depth, &self.limits)?;
		let kind = self.js.type_of(value)?;
		match kind {
			sys::ValueType::napi_null => return Ok(self.writer.write_byte(0xf6)?),
			sys::ValueType::napi_boolean => {
				let value = self.js.bool(value)?;
				return Ok(self.writer.write_byte(if value { 0xf5 } else { 0xf4 })?);
			}
			sys::ValueType::napi_number => return Ok(cbor::encode_number(&mut self.writer, self.js.number(value)?)?),
			sys::ValueType::napi_string => return self.encode_string(value),
			sys::ValueType::napi_object | sys::ValueType::napi_function => {}
			_ => {
				return Err(Failure::Cbor(format!(
					"Unsupported CBOR value type: {}",
					type_name(kind)
				)));
			}
		}

		if self.js.instance_of(value, self.realm.uint8_array)? {
			let Some(bytes) = self.js.uint8_bytes(value)? else {
				// Not a real typed array (e.g. `Object.create(Uint8Array.prototype)`): reading `byteLength`
				// throws the same TypeError the TypeScript encoder hits.
				self.js.named(value, "byteLength")?;
				return Err(Failure::Cbor("CBOR byte string must be a Uint8Array".into()));
			};
			return Ok(cbor::encode_byte_string(&mut self.writer, bytes, &self.limits)?);
		}
		// `napi_is_array` does not see through proxies; `Array.isArray` does.
		let is_array = self.js.call(self.realm.array_is_array, self.realm.array, &[value])?;
		if self.js.bool(is_array)? {
			return self.encode_array(value, depth);
		}
		if kind == sys::ValueType::napi_object && self.is_plain_object(value)? {
			return self.encode_object(value, depth);
		}
		Err(Failure::Cbor(format!(
			"Unsupported CBOR value type: {}",
			type_name(kind)
		)))
	}

	fn encode_array(&mut self, value: sys::napi_value, depth: usize) -> Outcome<()> {
		if self.is_ancestor(value)? {
			return Err(Failure::Cbor("CBOR values must not contain cycles".into()));
		}
		if self.length(value)? > self.limits.max_container_length as f64 {
			return Err(Failure::Cbor(format!(
				"CBOR array length exceeds configured limit of {}",
				self.limits.max_container_length
			)));
		}
		self.ancestors.push(value);
		let result = (|| {
			self.writer.write_argument(4, self.length(value)? as u64)?;
			let mut index = 0u32;
			while (index as f64) < self.length(value)? {
				let _scope = self.js.open_scope()?;
				let key = self.js.string(&index.to_string())?;
				let missing = !self.js.has_own(value, key)? || {
					let element = self.js.element(value, index)?;
					self.js.type_of(element)? == sys::ValueType::napi_undefined
				};
				if missing {
					return Err(Failure::Cbor(
						"CBOR arrays must not contain holes or undefined values".into(),
					));
				}
				let element = self.js.element(value, index)?;
				self.encode_value(element, depth + 1)?;
				index += 1;
			}
			Ok(())
		})();
		self.ancestors.pop();
		result
	}

	fn encode_object(&mut self, value: sys::napi_value, depth: usize) -> Outcome<()> {
		if self.is_ancestor(value)? {
			return Err(Failure::Cbor("CBOR values must not contain cycles".into()));
		}
		let symbols = self
			.js
			.call(self.realm.object_get_own_property_symbols, self.realm.object, &[value])?;
		for index in 0..self.js.array_length(symbols)? {
			let symbol = self.js.element(symbols, index)?;
			let enumerable = self.js.call(self.realm.property_is_enumerable, value, &[symbol])?;
			if self.js.bool(enumerable)? {
				return Err(Failure::Cbor("CBOR map keys must be strings".into()));
			}
		}
		let keys = self.js.call(self.realm.object_keys, self.realm.object, &[value])?;
		let mut entries = Vec::new();
		for index in 0..self.js.array_length(keys)? {
			let key = self.js.element(keys, index)?;
			let entry = self.js.get(value, key)?;
			if self.js.type_of(entry)? != sys::ValueType::napi_undefined {
				entries.push((key, entry));
			}
		}
		cbor::check_map_length(entries.len(), &self.limits)?;
		self.ancestors.push(value);
		let result = (|| {
			self.writer.write_argument(5, entries.len() as u64)?;
			for &(key, entry) in &entries {
				self.encode_string(key)?;
				self.encode_value(entry, depth + 1)?;
			}
			Ok(())
		})();
		self.ancestors.pop();
		result
	}
}

/// Byte strings of at least this length are copied with `napi_create_buffer_copy`, which skips the zero-fill
/// of `napi_create_arraybuffer`.
const LARGE_BYTES_LENGTH: usize = 64 * 1024;

/// Creates a plain `Uint8Array` with its own `ArrayBuffer` of exactly `bytes.len()` bytes, like
/// `new Uint8Array(view)` in the TypeScript decoder.
fn create_uint8_array(js: Js, bytes: &[u8]) -> Outcome<sys::napi_value> {
	let mut data = ptr::null_mut();
	let mut buffer = ptr::null_mut();
	if bytes.len() >= LARGE_BYTES_LENGTH {
		// A Node.js Buffer copy gets a dedicated, uninitialized ArrayBuffer (pooling only happens in JavaScript);
		// re-wrap that ArrayBuffer as a plain Uint8Array.
		let mut node_buffer = ptr::null_mut();
		check(js.env, unsafe {
			sys::napi_create_buffer_copy(
				js.env,
				bytes.len(),
				bytes.as_ptr() as *const _,
				&mut data,
				&mut node_buffer,
			)
		})?;
		let mut kind = 0;
		let mut length = 0;
		let mut offset = 0;
		check(js.env, unsafe {
			sys::napi_get_typedarray_info(
				js.env,
				node_buffer,
				&mut kind,
				&mut length,
				&mut data,
				&mut buffer,
				&mut offset,
			)
		})?;
		let mut buffer_length = 0;
		check(js.env, unsafe {
			sys::napi_get_arraybuffer_info(js.env, buffer, ptr::null_mut(), &mut buffer_length)
		})?;
		if offset == 0 && buffer_length == bytes.len() {
			let mut array = ptr::null_mut();
			check(js.env, unsafe {
				sys::napi_create_typedarray(
					js.env,
					sys::TypedarrayType::uint8_array,
					bytes.len(),
					buffer,
					0,
					&mut array,
				)
			})?;
			return Ok(array);
		}
	}
	check(js.env, unsafe {
		sys::napi_create_arraybuffer(js.env, bytes.len(), &mut data, &mut buffer)
	})?;
	if !bytes.is_empty() {
		// SAFETY: the new ArrayBuffer has exactly `bytes.len()` writable bytes at `data`.
		unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), data as *mut u8, bytes.len()) };
	}
	let mut array = ptr::null_mut();
	check(js.env, unsafe {
		sys::napi_create_typedarray(
			js.env,
			sys::TypedarrayType::uint8_array,
			bytes.len(),
			buffer,
			0,
			&mut array,
		)
	})?;
	Ok(array)
}

/// Non-ASCII strings of at least this many bytes are transcoded to UTF-16 before being handed to V8.
const LONG_TEXT_LENGTH: usize = 1024;

/// Strings longer than this are created directly instead of through the JSON skeleton.
const INLINE_TEXT_LIMIT: usize = 256;

enum Step<'a> {
	Key(&'a str),
	Index(u32),
}

/// A value left out of the JSON skeleton (as `null`) and assigned after parsing.
struct Patch<'a> {
	path: Vec<Step<'a>>,
	value: &'a ValueRef<'a>,
}

/// Decoded values are turned into JavaScript by writing them as JSON and calling V8's `JSON.parse`, which
/// creates objects far faster than one Node-API call per property. `JSON.parse` defines own data properties
/// (so `"__proto__"` stays an ordinary key) and orders keys like `Object.defineProperty`, matching the
/// TypeScript decoder. Byte strings and long strings are patched in afterwards.
struct Skeleton<'a> {
	json: Vec<u8>,
	path: Vec<Step<'a>>,
	patches: Vec<Patch<'a>>,
}

impl<'a> Skeleton<'a> {
	fn write(&mut self, value: &'a ValueRef<'a>) {
		match value {
			ValueRef::Null => self.json.extend_from_slice(b"null"),
			ValueRef::Bool(true) => self.json.extend_from_slice(b"true"),
			ValueRef::Bool(false) => self.json.extend_from_slice(b"false"),
			ValueRef::Number(number) => write_number(&mut self.json, *number),
			ValueRef::Text(text) if text.len() <= INLINE_TEXT_LIMIT => write_json_string(&mut self.json, text),
			ValueRef::Text(_) | ValueRef::Bytes(_) => {
				self.json.extend_from_slice(b"null");
				let path = self
					.path
					.iter()
					.map(|step| match step {
						Step::Key(key) => Step::Key(key),
						Step::Index(index) => Step::Index(*index),
					})
					.collect();
				self.patches.push(Patch { path, value });
			}
			ValueRef::Array(items) => {
				self.json.push(b'[');
				for (index, item) in items.iter().enumerate() {
					if index > 0 {
						self.json.push(b',');
					}
					self.path.push(Step::Index(index as u32));
					self.write(item);
					self.path.pop();
				}
				self.json.push(b']');
			}
			ValueRef::Map(entries) => {
				self.json.push(b'{');
				for (index, (key, entry)) in entries.iter().enumerate() {
					if index > 0 {
						self.json.push(b',');
					}
					write_json_string(&mut self.json, key);
					self.json.push(b':');
					self.path.push(Step::Key(key));
					self.write(entry);
					self.path.pop();
				}
				self.json.push(b'}');
			}
		}
	}
}

/// Writes a finite number so that `JSON.parse` returns the identical double, including `-0`.
fn write_number(json: &mut Vec<u8>, number: f64) {
	use std::io::Write;
	if number.fract() == 0.0 && number.abs() < 9_007_199_254_740_992.0 && !(number == 0.0 && number.is_sign_negative())
	{
		let _ = write!(json, "{}", number as i64);
	} else {
		// `Display` prints the shortest digits that round-trip, without an exponent.
		let _ = write!(json, "{number}");
	}
}

fn write_json_string(json: &mut Vec<u8>, text: &str) {
	const HEX: &[u8; 16] = b"0123456789abcdef";
	json.push(b'"');
	let bytes = text.as_bytes();
	let mut start = 0;
	for (index, &byte) in bytes.iter().enumerate() {
		let escape: &[u8] = match byte {
			b'"' => b"\\\"",
			b'\\' => b"\\\\",
			0x00..=0x1f => b"",
			_ => continue,
		};
		json.extend_from_slice(&bytes[start..index]);
		if escape.is_empty() {
			json.extend_from_slice(&[
				b'\\',
				b'u',
				b'0',
				b'0',
				HEX[(byte >> 4) as usize],
				HEX[(byte & 0xf) as usize],
			]);
		} else {
			json.extend_from_slice(escape);
		}
		start = index + 1;
	}
	json.extend_from_slice(&bytes[start..]);
	json.push(b'"');
}

/// Creates a value that the skeleton does not express: a byte string or a string.
fn leaf_to_js(js: Js, value: &ValueRef<'_>) -> Outcome<sys::napi_value> {
	let env = js.env;
	let mut result = ptr::null_mut();
	match value {
		ValueRef::Text(text) => result = js.text(text)?,
		ValueRef::Bytes(bytes) => result = create_uint8_array(js, bytes)?,
		ValueRef::Null => check(env, unsafe { sys::napi_get_null(env, &mut result) })?,
		ValueRef::Bool(value) => check(env, unsafe { sys::napi_get_boolean(env, *value, &mut result) })?,
		ValueRef::Number(value) => check(env, unsafe { sys::napi_create_double(env, *value, &mut result) })?,
		ValueRef::Array(_) | ValueRef::Map(_) => unreachable!("containers are built by JSON.parse"),
	}
	Ok(result)
}

fn define_data_property(js: Js, object: sys::napi_value, key: &str, value: sys::napi_value) -> Outcome<()> {
	let descriptor = sys::napi_property_descriptor {
		utf8name: ptr::null(),
		name: js.text(key)?,
		method: None,
		getter: None,
		setter: None,
		value,
		attributes: sys::PropertyAttributes::writable
			| sys::PropertyAttributes::enumerable
			| sys::PropertyAttributes::configurable,
		data: ptr::null_mut(),
	};
	check(js.env, unsafe {
		sys::napi_define_properties(js.env, object, 1, &descriptor)
	})
}

fn to_js(js: Js, value: &ValueRef<'_>) -> Outcome<sys::napi_value> {
	if !matches!(value, ValueRef::Array(_) | ValueRef::Map(_)) {
		return leaf_to_js(js, value);
	}
	let mut skeleton = Skeleton {
		json: Vec::new(),
		path: Vec::new(),
		patches: Vec::new(),
	};
	skeleton.write(value);
	// SAFETY: the skeleton is built from `&str` pieces and ASCII syntax, so it is valid UTF-8.
	let json = unsafe { std::str::from_utf8_unchecked(&skeleton.json) };
	let text = js.text(json)?;
	let mut undefined = ptr::null_mut();
	check(js.env, unsafe { sys::napi_get_undefined(js.env, &mut undefined) })?;
	let root = js.call(json_parse(js)?, undefined, &[text])?;
	for patch in &skeleton.patches {
		let _scope = js.open_scope()?;
		let (last, parents) = patch.path.split_last().expect("patched values are inside a container");
		let mut parent = root;
		for step in parents {
			parent = match step {
				Step::Key(key) => js.get(parent, js.text(key)?)?,
				Step::Index(index) => js.element(parent, *index)?,
			};
		}
		let leaf = leaf_to_js(js, patch.value)?;
		match last {
			Step::Key(key) => define_data_property(js, parent, key, leaf)?,
			Step::Index(index) => check(js.env, unsafe { sys::napi_set_element(js.env, parent, *index, leaf) })?,
		}
	}
	Ok(root)
}

thread_local! {
	/// `JSON.parse` as it was when first needed, so later reassignments of the global do not affect decoding.
	static JSON_PARSE: std::cell::Cell<Option<(sys::napi_env, sys::napi_ref)>> = const { std::cell::Cell::new(None) };
}

fn json_parse(js: Js) -> Outcome<sys::napi_value> {
	if let Some((env, reference)) = JSON_PARSE.get()
		&& env == js.env
	{
		let mut value = ptr::null_mut();
		check(js.env, unsafe {
			sys::napi_get_reference_value(js.env, reference, &mut value)
		})?;
		return Ok(value);
	}
	let mut global = ptr::null_mut();
	check(js.env, unsafe { sys::napi_get_global(js.env, &mut global) })?;
	let parse = js.named(js.named(global, "JSON")?, "parse")?;
	let mut reference = ptr::null_mut();
	check(js.env, unsafe {
		sys::napi_create_reference(js.env, parse, 1, &mut reference)
	})?;
	JSON_PARSE.set(Some((js.env, reference)));
	Ok(parse)
}

fn limits(max_byte_length: u32, max_container_length: u32, max_depth: u32) -> Limits {
	Limits {
		max_byte_length: max_byte_length as usize,
		max_container_length: max_container_length as usize,
		max_depth: max_depth as usize,
	}
}

/// Encodes a JavaScript value. Limits must already be validated by the caller.
#[napi(js_name = "cborEncode")]
pub fn cbor_encode(
	env: Env,
	value: Unknown<'_>,
	max_byte_length: u32,
	max_container_length: u32,
	max_depth: u32,
) -> napi::Result<sys::napi_value> {
	let js = Js { env: env.raw() };
	let run = || -> Outcome<sys::napi_value> {
		let limits = limits(max_byte_length, max_container_length, max_depth);
		let mut encoder = Encoder {
			js,
			realm: Realm::new(js)?,
			limits,
			writer: Writer::new(limits.max_byte_length),
			ancestors: Vec::new(),
			scratch: Vec::new(),
		};
		encoder.encode_value(value.value().value, 0)?;
		create_uint8_array(js, &encoder.writer.finish())
	};
	run().map_err(|failure| throw(js.env, failure))
}

/// Decodes exactly one CBOR item. Limits must already be validated by the caller.
#[napi(js_name = "cborDecode")]
pub fn cbor_decode(
	env: Env,
	bytes: Uint8Array,
	max_byte_length: u32,
	max_container_length: u32,
	max_depth: u32,
) -> napi::Result<sys::napi_value> {
	let js = Js { env: env.raw() };
	let run = || -> Outcome<sys::napi_value> {
		let value = cbor::decode_ref(&bytes, &limits(max_byte_length, max_container_length, max_depth))?;
		to_js(js, &value)
	};
	run().map_err(|failure| throw(js.env, failure))
}

/// Prefixes a payload with its unsigned 32-bit big-endian length.
#[napi(js_name = "frameEncode")]
pub fn frame_encode(env: Env, payload: Uint8Array) -> napi::Result<sys::napi_value> {
	let js = Js { env: env.raw() };
	let run = || -> Outcome<sys::napi_value> { create_uint8_array(js, &framing::encode_frame(&payload)?) };
	run().map_err(|failure| throw(js.env, failure))
}

/// Incremental length-prefixed frame splitter.
#[napi(js_name = "NativeFrameDecoder")]
pub struct NativeFrameDecoder {
	inner: framing::FrameDecoder,
}

#[napi]
impl NativeFrameDecoder {
	#[napi(constructor)]
	pub fn new(max_frame_length: u32) -> Self {
		Self {
			inner: framing::FrameDecoder::new(max_frame_length as usize),
		}
	}

	#[napi]
	pub fn push(&mut self, env: Env, chunk: Uint8Array) -> napi::Result<Vec<sys::napi_value>> {
		let js = Js { env: env.raw() };
		let mut run = || -> Outcome<Vec<sys::napi_value>> {
			let mut frames = Vec::new();
			let mut failure = None;
			self.inner.push_with(&chunk, |frame| {
				if failure.is_none() {
					match create_uint8_array(js, frame) {
						Ok(array) => frames.push(array),
						Err(error) => failure = Some(error),
					}
				}
			})?;
			match failure {
				Some(error) => Err(error),
				None => Ok(frames),
			}
		};
		run().map_err(|failure| throw(js.env, failure))
	}

	#[napi]
	pub fn end(&mut self, env: Env) -> napi::Result<()> {
		self.inner.end().map_err(|error| throw(env.raw(), error.into()))
	}
}
