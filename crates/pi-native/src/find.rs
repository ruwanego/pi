//! N-API bindings for `pi-find`.
//!
//! A search runs on the libuv thread pool, so the JavaScript thread stays free while the tree is walked.
//! `cancel()` stops the walk early; the pending promise then resolves with the matches found so far, which
//! the caller discards.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use napi::bindgen_prelude::AsyncTask;
use napi::{Env, Error, Status, Task};
use napi_derive::napi;
use pi_find::FindOptions;

/// Arguments of one fd invocation, see `pi_find::FindOptions`.
#[napi(object, js_name = "NativeFindOptions")]
pub struct NativeFindOptions {
	pub pattern: String,
	pub search_path: String,
	pub full_path: bool,
	pub require_git: bool,
	/// `--max-results`; `0` means unlimited. Must already be validated as a non-negative integer.
	pub max_results: f64,
}

/// One cancellable search.
#[napi(js_name = "NativeFindSearch")]
pub struct NativeFindSearch {
	options: FindOptions,
	cancelled: Arc<AtomicBool>,
}

#[napi]
impl NativeFindSearch {
	#[napi(constructor)]
	pub fn new(options: NativeFindOptions) -> Self {
		Self {
			options: FindOptions {
				pattern: options.pattern,
				search_path: PathBuf::from(options.search_path),
				full_path: options.full_path,
				require_git: options.require_git,
				// Saturating float-to-int conversion; values past usize::MAX are effectively unlimited.
				max_results: Some(options.max_results as usize),
			},
			cancelled: Arc::new(AtomicBool::new(false)),
		}
	}

	/// Resolves to fd's output lines, or rejects with fd's stderr text.
	#[napi(ts_return_type = "Promise<string[]>")]
	pub fn run(&self) -> AsyncTask<FindTask> {
		AsyncTask::new(FindTask {
			options: self.options.clone(),
			cancelled: Arc::clone(&self.cancelled),
		})
	}

	#[napi]
	pub fn cancel(&self) {
		self.cancelled.store(true, Ordering::Relaxed);
	}
}

pub struct FindTask {
	options: FindOptions,
	cancelled: Arc<AtomicBool>,
}

impl Task for FindTask {
	type Output = Vec<String>;
	type JsValue = Vec<String>;

	fn compute(&mut self) -> napi::Result<Self::Output> {
		pi_find::find(&self.options, &self.cancelled).map_err(|error| Error::new(Status::GenericFailure, error.0))
	}

	fn resolve(&mut self, _env: Env, output: Self::Output) -> napi::Result<Self::JsValue> {
		Ok(output)
	}
}
