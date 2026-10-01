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
use pi_protocol::cbor::{self, CborError, Limits, Value, Writer};
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

	fn encode_string(&mut self, value: sys::napi_value) -> Outcome<()> {
		let units = self.js.utf16(value)?;
		cbor::encode_text_utf16(&mut self.writer, &units, &self.limits)?;
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

fn create_uint8_array(js: Js, bytes: &[u8]) -> Outcome<sys::napi_value> {
	let mut data = ptr::null_mut();
	let mut buffer = ptr::null_mut();
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

fn to_js(js: Js, value: &Value) -> Outcome<sys::napi_value> {
	let env = js.env;
	let mut result = ptr::null_mut();
	match value {
		Value::Null => check(env, unsafe { sys::napi_get_null(env, &mut result) })?,
		Value::Bool(value) => check(env, unsafe { sys::napi_get_boolean(env, *value, &mut result) })?,
		Value::Number(value) => check(env, unsafe { sys::napi_create_double(env, *value, &mut result) })?,
		Value::Text(value) => result = js.string(value)?,
		Value::Bytes(value) => result = create_uint8_array(js, value)?,
		Value::Array(items) => {
			check(env, unsafe { sys::napi_create_array(env, &mut result) })?;
			for (index, item) in items.iter().enumerate() {
				let _scope = js.open_scope()?;
				let item = to_js(js, item)?;
				check(env, unsafe { sys::napi_set_element(env, result, index as u32, item) })?;
			}
		}
		Value::Map(entries) => {
			check(env, unsafe { sys::napi_create_object(env, &mut result) })?;
			for (key, entry) in entries {
				let _scope = js.open_scope()?;
				// defineProperty semantics, so keys such as "__proto__" stay ordinary data properties.
				let descriptor = sys::napi_property_descriptor {
					utf8name: ptr::null(),
					name: js.string(key)?,
					method: None,
					getter: None,
					setter: None,
					value: to_js(js, entry)?,
					attributes: sys::PropertyAttributes::writable
						| sys::PropertyAttributes::enumerable
						| sys::PropertyAttributes::configurable,
					data: ptr::null_mut(),
				};
				check(env, unsafe { sys::napi_define_properties(env, result, 1, &descriptor) })?;
			}
		}
	}
	Ok(result)
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
		let value = cbor::decode(&bytes, &limits(max_byte_length, max_container_length, max_depth))?;
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
			let frames = self.inner.push(&chunk)?;
			frames.iter().map(|frame| create_uint8_array(js, frame)).collect()
		};
		run().map_err(|failure| throw(js.env, failure))
	}

	#[napi]
	pub fn end(&mut self, env: Env) -> napi::Result<()> {
		self.inner.end().map_err(|error| throw(env.raw(), error.into()))
	}
}
