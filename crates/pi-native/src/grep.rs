//! N-API bindings for `pi-grep`.
//!
//! Like `find`, a search runs on the libuv thread pool and `cancel()` stops it early; the pending promise then
//! resolves with the matches found so far, which the caller discards.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Error, Status, Task};
use napi_derive::napi;
use pi_grep::GrepOptions;

/// Arguments of one ripgrep invocation, see `pi_grep::GrepOptions`.
#[napi(object, js_name = "NativeGrepOptions")]
pub struct NativeGrepOptions {
	pub pattern: String,
	pub search_path: String,
	pub glob: Option<String>,
	pub ignore_case: bool,
	pub fixed_strings: bool,
	/// Stop after this many matches. Must be a positive integer.
	pub max_matches: f64,
	/// Lines of context to return around each match; absent or `0` for none.
	pub context: Option<f64>,
}

/// One ripgrep `match` message. `path` and `line` are absent when ripgrep would report them as bytes.
#[napi(object, js_name = "NativeGrepMatch")]
pub struct NativeGrepMatch {
	pub path: Option<String>,
	pub line_number: f64,
	pub line: Option<String>,
	/// With context requested: the first line number of `context_lines`, as the grep tool reads the file.
	pub context_start: Option<f64>,
	pub context_lines: Option<Vec<String>>,
}

#[napi(object, js_name = "NativeGrepResult")]
pub struct NativeGrepResult {
	pub matches: Vec<NativeGrepMatch>,
	/// Whether the search stopped at `maxMatches`.
	pub limit_reached: bool,
	/// ripgrep's stderr text, one message per line.
	pub stderr: String,
	/// Whether ripgrep would exit with status 2.
	pub errored: bool,
}

/// One cancellable search.
#[napi(js_name = "NativeGrepSearch")]
pub struct NativeGrepSearch {
	options: GrepOptions,
	cancelled: Arc<AtomicBool>,
}

#[napi]
impl NativeGrepSearch {
	#[napi(constructor)]
	pub fn new(options: NativeGrepOptions) -> Self {
		Self {
			options: GrepOptions {
				pattern: options.pattern,
				search_path: PathBuf::from(options.search_path),
				glob: options.glob,
				ignore_case: options.ignore_case,
				fixed_strings: options.fixed_strings,
				// Saturating float-to-int conversion; values past usize::MAX are effectively unlimited.
				max_matches: Some(options.max_matches as usize),
				context: options.context.map_or(0, |context| context as usize),
			},
			cancelled: Arc::new(AtomicBool::new(false)),
		}
	}

	/// Resolves to the search result, or rejects with ripgrep's stderr text when it would exit before searching.
	#[napi(ts_return_type = "Promise<NativeGrepResult>")]
	pub fn run(&self) -> AsyncTask<GrepTask> {
		AsyncTask::new(GrepTask {
			options: self.options.clone(),
			cancelled: Arc::clone(&self.cancelled),
		})
	}

	#[napi]
	pub fn cancel(&self) {
		self.cancelled.store(true, Ordering::Relaxed);
	}
}

pub struct GrepTask {
	options: GrepOptions,
	cancelled: Arc<AtomicBool>,
}

impl Task for GrepTask {
	type Output = pi_grep::GrepOutput;
	type JsValue = NativeGrepResult;

	fn compute(&mut self) -> napi::Result<Self::Output> {
		pi_grep::grep(&self.options, &self.cancelled).map_err(|error| Error::new(Status::GenericFailure, error.0))
	}

	fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
		Ok(NativeGrepResult {
			matches: output
				.matches
				.into_iter()
				.map(|m| {
					let (context_start, context_lines) = match m.context {
						Some(block) => (Some(block.start as f64), Some(block.lines)),
						None => (None, None),
					};
					NativeGrepMatch {
						path: m.path,
						line_number: m.line_number as f64,
						line: m.line,
						context_start,
						context_lines,
					}
				})
				.collect(),
			limit_reached: output.limit_reached,
			stderr: output.messages.join("\n"),
			errored: output.errored,
		})
	}
}
