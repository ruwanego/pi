//! N-API binding for `pi-json`: the fast path of pi-ai's `parseStreamingJson`.
//!
//! Returns the parsed value, or `undefined` when the caller must run the original TypeScript implementation.

#![cfg_attr(test, allow(dead_code))]

use std::cell::RefCell;
use std::ptr;

use napi::{Env, JsValue, Unknown, sys};
use napi_derive::napi;
use pi_json::{Analysis, Decoded, Step};

use crate::protocol::{Failure, Js, Outcome, check, json_parse, throw};

thread_local! {
	/// Reused UTF-8 buffer: tool-call arguments are re-parsed on every streamed delta.
	static INPUT: RefCell<Vec<u8>> = const { RefCell::new(Vec::new()) };
}

fn undefined(js: Js) -> Outcome<sys::napi_value> {
	let mut value = ptr::null_mut();
	check(js.env, unsafe { sys::napi_get_undefined(js.env, &mut value) })?;
	Ok(value)
}

/// Calls `JSON.parse(text)`. A thrown exception is cleared and reported as `None`.
fn try_json_parse(js: Js, text: sys::napi_value) -> Outcome<Option<sys::napi_value>> {
	let parse = json_parse(js)?;
	let receiver = undefined(js)?;
	let mut result = ptr::null_mut();
	let status = unsafe { sys::napi_call_function(js.env, receiver, parse, 1, &text, &mut result) };
	if status == sys::Status::napi_ok {
		return Ok(Some(result));
	}
	let mut pending = false;
	unsafe { sys::napi_is_exception_pending(js.env, &mut pending) };
	if !pending {
		return Err(Failure::Napi(status));
	}
	let mut exception = ptr::null_mut();
	check(js.env, unsafe {
		sys::napi_get_and_clear_last_exception(js.env, &mut exception)
	})?;
	Ok(None)
}

fn decoded_to_js(js: Js, decoded: &Decoded) -> Outcome<sys::napi_value> {
	match decoded {
		Decoded::Utf8(text) => js.text(text),
		Decoded::Utf16(units) => {
			let mut result = ptr::null_mut();
			check(js.env, unsafe {
				sys::napi_create_string_utf16(js.env, units.as_ptr(), units.len() as isize, &mut result)
			})?;
			Ok(result)
		}
	}
}

fn key_to_js(js: Js, raw: &str) -> Outcome<sys::napi_value> {
	let decoded = pi_json::decode_string(raw).expect("analyze only accepts valid keys");
	decoded_to_js(js, &decoded)
}

/// `parseStreamingJson(input)` for inputs `pi-json` understands, otherwise `undefined`.
#[napi(js_name = "parseStreamingJsonFast")]
pub fn parse_streaming_json_fast(env: Env, input: Unknown<'_>) -> napi::Result<sys::napi_value> {
	let js = Js { env: env.raw() };
	let value = input.value().value;
	let run = || -> Outcome<sys::napi_value> {
		INPUT.with_borrow_mut(|buffer| -> Outcome<sys::napi_value> {
			let units = js.utf16_length(value)?;
			js.utf8_into(value, units * 3, buffer)?;
			let Ok(text) = std::str::from_utf8(buffer) else {
				return undefined(js);
			};
			match pi_json::analyze(text) {
				Analysis::Fallback => undefined(js),
				Analysis::Complete => match try_json_parse(js, value)? {
					Some(result) => Ok(result),
					None => undefined(js),
				},
				Analysis::Partial { skeleton, patches } => {
					let skeleton = js.text(&skeleton)?;
					let Some(root) = try_json_parse(js, skeleton)? else {
						return undefined(js);
					};
					for patch in &patches {
						let _scope = js.open_scope()?;
						let (last, parents) = patch.path.split_last().expect("patched strings are inside a container");
						let mut parent = root;
						for step in parents {
							parent = match step {
								Step::Key(key) => js.get(parent, key_to_js(js, key)?)?,
								Step::Index(index) => js.element(parent, *index)?,
							};
						}
						let decoded = pi_json::decode_string(patch.raw).expect("analyze only accepts valid strings");
						let string = decoded_to_js(js, &decoded)?;
						match last {
							Step::Key(key) => check(js.env, unsafe {
								sys::napi_set_property(js.env, parent, key_to_js(js, key)?, string)
							})?,
							Step::Index(index) => {
								check(js.env, unsafe { sys::napi_set_element(js.env, parent, *index, string) })?
							}
						}
					}
					Ok(root)
				}
			}
		})
	};
	run().map_err(|failure| throw(js.env, failure))
}
