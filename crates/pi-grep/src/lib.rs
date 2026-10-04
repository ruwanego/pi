//! Rust port of the ripgrep search behind the coding-agent `grep` tool.
//!
//! The tool runs `rg --json --line-number --color=never --hidden [--ignore-case] [--fixed-strings] [--glob GLOB] --
//! PATTERN PATH` and reads the `match` messages. [`grep`] reproduces that invocation with the crates ripgrep 15 is
//! built on (`ignore`, `grep-regex`, `grep-searcher`) and the configuration from ripgrep's `crates/core/flags/hiargs.rs`:
//! the same matcher options, binary detection (quit on NUL for traversed files, convert for an explicit file), BOM
//! sniffing, memory maps only for an explicit file, ignore files (`.gitignore` inside git repositories, `.ignore`,
//! `.rgignore`, git excludes), `--glob` rooted at the process working directory, and ripgrep's stderr messages.
//!
//! Files are reported in the order their searches finish, as ripgrep prints them. ripgrep configuration files are
//! not read: the grep tool keeps ripgrep when `RIPGREP_CONFIG_PATH` is set.

use std::collections::HashSet;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};

use grep_regex::{RegexMatcher, RegexMatcherBuilder};
use grep_searcher::{BinaryDetection, MmapChoice, Searcher, SearcherBuilder, Sink, SinkMatch};
use ignore::overrides::{Override, OverrideBuilder};
use ignore::{WalkBuilder, WalkState};

/// Arguments of one ripgrep invocation.
#[derive(Debug, Clone)]
pub struct GrepOptions {
	pub pattern: String,
	/// File or directory to search.
	pub search_path: PathBuf,
	/// `--glob`.
	pub glob: Option<String>,
	/// `--ignore-case`.
	pub ignore_case: bool,
	/// `--fixed-strings`.
	pub fixed_strings: bool,
	/// Stop after this many matches, like the tool killing ripgrep. `None` means unlimited.
	pub max_matches: Option<usize>,
	/// Lines of context the grep tool shows around each match; see [`GrepMatch::context`]. `0` for none.
	pub context: usize,
}

/// One `match` message: `path.text`, `line_number` and `lines.text`. Paths and lines that are not valid UTF-8 are
/// `None`, because ripgrep reports them as `bytes` instead of `text`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepMatch {
	pub path: Option<String>,
	pub line_number: u64,
	pub line: Option<String>,
	/// With `context > 0`, the lines the grep tool shows for this match, read from the file the way the tool
	/// reads it. `None` if the file could not be read.
	pub context: Option<ContextBlock>,
}

/// Lines `start..start + lines.len()` (1-based) of a file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ContextBlock {
	pub start: u64,
	pub lines: Vec<String>,
}

#[derive(Debug, Default)]
pub struct GrepOutput {
	pub matches: Vec<GrepMatch>,
	/// Whether `max_matches` was reached; the tool kills ripgrep at that point and ignores its exit status.
	pub limit_reached: bool,
	/// ripgrep's stderr lines (with the `rg: ` prefix).
	pub messages: Vec<String>,
	/// Whether ripgrep would exit with status 2 (an error message was emitted).
	pub errored: bool,
}

/// A failure that makes ripgrep exit before searching, formatted as ripgrep prints it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GrepError(pub String);

impl std::fmt::Display for GrepError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for GrepError {}

fn rg_message(message: impl std::fmt::Display) -> String {
	format!("rg: {message}")
}

/// Runs the search. Returns early with the matches found so far once `cancel` is set.
pub fn grep(options: &GrepOptions, cancel: &AtomicBool) -> Result<GrepOutput, GrepError> {
	grep_with_budget(options, cancel, SEQUENTIAL_BUDGET)
}

fn grep_with_budget(options: &GrepOptions, cancel: &AtomicBool, budget: usize) -> Result<GrepOutput, GrepError> {
	let cwd = current_dir()?;
	let matcher = build_matcher(options)?;
	let overrides = build_overrides(&cwd, options.glob.as_deref())?;
	let is_one_file = !options.search_path.is_dir();
	let mmap = if is_one_file && options.search_path.is_file() {
		// SAFETY: same choice as ripgrep; a file truncated while mapped can make the process abort.
		unsafe { MmapChoice::auto() }
	} else {
		MmapChoice::never()
	};
	let mut searcher_builder = SearcherBuilder::new();
	searcher_builder.line_number(true).memory_map(mmap);
	let searcher = searcher_builder.build();

	let mut walk_builder = WalkBuilder::new(&options.search_path);
	walk_builder
		.overrides(overrides)
		.hidden(false)
		.parents(true)
		.ignore(true)
		.git_global(true)
		.git_ignore(true)
		.git_exclude(true)
		.require_git(true)
		.follow_links(false)
		.current_dir(&cwd)
		.add_custom_ignore_filename(".rgignore");

	let state = State {
		matcher: &matcher,
		max_matches: options.max_matches,
		context: options.context,
		count: AtomicUsize::new(0),
		cancel,
		files: Mutex::new(Vec::new()),
		messages: Mutex::new(Vec::new()),
		errored: AtomicBool::new(false),
	};

	// Starting threads costs more than searching a small tree, so search on this thread first and switch to
	// ripgrep's parallel walker (starting over) only if the tree turns out to be large. ripgrep itself
	// uses one thread for a single file.
	let mut finished = is_one_file || !is_large_tree(&options.search_path);
	if finished {
		let mut sequential_searcher = searcher.clone();
		for (visited, result) in walk_builder.build().enumerate() {
			// The sequential walker words traversal errors differently from ripgrep's parallel walker; let the
			// parallel walker report them.
			if result.is_err() {
				finished = false;
				break;
			}
			if matches!(state.visit(&mut sequential_searcher, result), WalkState::Quit) {
				break;
			}
			if !is_one_file && visited >= budget {
				finished = false;
				remember_tree_size(&options.search_path, true);
				break;
			}
		}
		if finished && !is_one_file {
			remember_tree_size(&options.search_path, false);
		}
	}
	if !finished {
		state.reset();
		let threads = std::thread::available_parallelism().map_or(1, |n| n.get()).min(12);
		walk_builder.threads(threads).build_parallel().run(|| {
			let state = &state;
			let mut searcher = searcher.clone();
			Box::new(move |result| state.visit(&mut searcher, result))
		});
	}

	let limit_reached = options
		.max_matches
		.is_some_and(|max| state.count.load(Ordering::Relaxed) >= max);
	// Files in the order their searches finished, which is the order ripgrep prints them.
	let files = state.files.into_inner().expect("files lock");
	let mut matches: Vec<GrepMatch> = files.into_iter().flat_map(|(_, matches)| matches).collect();
	if let Some(max) = options.max_matches {
		matches.truncate(max);
	}
	Ok(GrepOutput {
		matches,
		limit_reached,
		messages: state.messages.into_inner().expect("messages lock"),
		errored: state.errored.load(Ordering::Relaxed),
	})
}

/// Search paths whose last search did not finish within the sequential budget. Later searches of the same path
/// start with the parallel walker instead of spending the budget again. This only affects speed.
static LARGE_TREES: Mutex<Option<HashSet<PathBuf>>> = Mutex::new(None);
const LARGE_TREES_CAPACITY: usize = 1024;

fn is_large_tree(path: &Path) -> bool {
	LARGE_TREES
		.lock()
		.expect("large trees lock")
		.as_ref()
		.is_some_and(|trees| trees.contains(path))
}

fn remember_tree_size(path: &Path, large: bool) {
	let mut trees = LARGE_TREES.lock().expect("large trees lock");
	let trees = trees.get_or_insert_with(HashSet::new);
	if !large {
		trees.remove(path);
		return;
	}
	if trees.len() >= LARGE_TREES_CAPACITY {
		trees.clear();
	}
	trees.insert(path.to_path_buf());
}

/// How many entries a search visits on the calling thread before switching to the parallel walker. Counting
/// entries instead of time keeps the choice (and the remembered tree size) independent of machine load.
const SEQUENTIAL_BUDGET: usize = 250;

struct State<'a> {
	matcher: &'a RegexMatcher,
	max_matches: Option<usize>,
	context: usize,
	count: AtomicUsize,
	cancel: &'a AtomicBool,
	files: Mutex<Vec<(PathBuf, Vec<GrepMatch>)>>,
	messages: Mutex<Vec<String>>,
	errored: AtomicBool,
}

impl State<'_> {
	fn done(&self) -> bool {
		self.cancel.load(Ordering::Relaxed)
			|| self
				.max_matches
				.is_some_and(|max| self.count.load(Ordering::Relaxed) >= max)
	}

	/// Forgets everything found so far, before the search starts over.
	fn reset(&self) {
		self.count.store(0, Ordering::Relaxed);
		self.files.lock().expect("files lock").clear();
		self.messages.lock().expect("messages lock").clear();
		self.errored.store(false, Ordering::Relaxed);
	}

	fn message(&self, message: String, error: bool) {
		if error {
			self.errored.store(true, Ordering::Relaxed);
		}
		self.messages.lock().expect("messages lock").push(message);
	}

	/// ripgrep's `HaystackBuilder::build_from_result` followed by `SearchWorker::search`.
	fn visit(&self, searcher: &mut Searcher, result: Result<ignore::DirEntry, ignore::Error>) -> WalkState {
		if self.done() {
			return WalkState::Quit;
		}
		let entry = match result {
			Ok(entry) => entry,
			Err(error) => {
				self.message(rg_message(error), true);
				return WalkState::Continue;
			}
		};
		if let Some(error) = entry.error() {
			self.message(rg_message(error), false);
		}
		let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir())
			|| (entry.path_is_symlink() && entry.path().is_dir());
		let is_explicit = entry.depth() == 0 && !is_dir;
		let is_file = entry.file_type().is_some_and(|file_type| file_type.is_file());
		if !is_explicit && !is_file {
			return WalkState::Continue;
		}

		searcher.set_binary_detection(if is_explicit {
			BinaryDetection::convert(b'\x00')
		} else {
			BinaryDetection::quit(b'\x00')
		});
		let path = entry.path();
		let mut sink = Collect {
			state: self,
			path: path.to_str().map(str::to_owned),
			matches: Vec::new(),
		};
		if let Err(error) = searcher.search_path(self.matcher, path, &mut sink) {
			self.message(rg_message(format!("{}: {error}", path.display())), true);
		}
		if !sink.matches.is_empty() {
			let mut matches = sink.matches;
			if self.context > 0 {
				add_context(path, self.context, &mut matches);
			}
			self.files
				.lock()
				.expect("files lock")
				.push((path.to_path_buf(), matches));
		}
		if self.done() {
			WalkState::Quit
		} else {
			WalkState::Continue
		}
	}
}

struct Collect<'s, 'a> {
	state: &'s State<'a>,
	path: Option<String>,
	matches: Vec<GrepMatch>,
}

impl Sink for Collect<'_, '_> {
	type Error = io::Error;

	fn matched(&mut self, _searcher: &Searcher, mat: &SinkMatch<'_>) -> Result<bool, io::Error> {
		if self.state.done() {
			return Ok(false);
		}
		self.state.count.fetch_add(1, Ordering::Relaxed);
		self.matches.push(GrepMatch {
			path: self.path.clone(),
			line_number: mat.line_number().unwrap_or(0),
			line: std::str::from_utf8(mat.bytes()).ok().map(str::to_owned),
			context: None,
		});
		Ok(!self.state.done())
	}
}

/// Fills in [`GrepMatch::context`] the way the grep tool's `formatBlock` reads a file: decoded as UTF-8 with
/// replacement characters, `\r\n` and lone `\r` treated as line breaks, lines clamped to the file.
fn add_context(path: &Path, context: usize, matches: &mut [GrepMatch]) {
	let Ok(bytes) = std::fs::read(path) else {
		return;
	};
	let text = String::from_utf8_lossy(&bytes)
		.replace("\r\n", "\n")
		.replace('\r', "\n");
	let lines: Vec<&str> = text.split('\n').collect();
	for m in matches {
		let line_number = m.line_number as usize;
		let start = line_number.saturating_sub(context).max(1);
		let end = (line_number + context).min(lines.len());
		m.context = Some(ContextBlock {
			start: start as u64,
			lines: (start..=end)
				.map(|current| lines.get(current - 1).copied().unwrap_or("").to_owned())
				.collect(),
		});
	}
}

/// ripgrep's `current_dir`: the process working directory, or `$PWD` if it was deleted.
fn current_dir() -> Result<PathBuf, GrepError> {
	match std::env::current_dir() {
		Ok(cwd) => Ok(cwd),
		Err(error) => match std::env::var_os("PWD").filter(|cwd| !cwd.is_empty()) {
			Some(cwd) => Ok(PathBuf::from(cwd)),
			None => Err(GrepError(rg_message(format!(
				"failed to get current working directory: {error}\ndid your CWD get deleted?"
			)))),
		},
	}
}

/// ripgrep's `HiArgs::matcher_rust` with the default engine, case and line terminator settings.
fn build_matcher(options: &GrepOptions) -> Result<RegexMatcher, GrepError> {
	let mut builder = RegexMatcherBuilder::new();
	builder
		.multi_line(true)
		.unicode(true)
		.octal(false)
		.fixed_strings(options.fixed_strings)
		.case_insensitive(options.ignore_case)
		.line_terminator(Some(b'\n'))
		.dot_matches_new_line(false)
		.ban_byte(Some(b'\x00'));
	builder
		.build_many(&[&options.pattern])
		.map_err(|error| GrepError(rg_message(suggest(error.to_string()))))
}

/// ripgrep's `suggest_other_engine` (for a build with PCRE2), `suggest_multiline` and `suggest_text`.
fn suggest(message: String) -> String {
	if message.contains("backreferences") || message.contains("look-around") {
		return format!(
			"{message}\n\nConsider enabling PCRE2 with the --pcre2 flag, which can handle backreferences\nand look-around."
		);
	}
	let message = if message.contains("the literal") && message.contains("not allowed") {
		format!(
			"{message}\n\nConsider enabling multiline mode with the --multiline flag (or -U for short).\nWhen multiline mode is enabled, new line characters can be matched."
		)
	} else {
		message
	};
	if message.contains("pattern contains \"\\0\"") {
		format!(
			"{message}\n\nConsider enabling text mode with the --text flag (or -a for short). Otherwise,\nbinary detection is enabled and matching a NUL byte is impossible."
		)
	} else {
		message
	}
}

fn build_overrides(cwd: &Path, glob: Option<&str>) -> Result<Override, GrepError> {
	let Some(glob) = glob else {
		return Ok(Override::empty());
	};
	let mut builder = OverrideBuilder::new(cwd);
	builder.add(glob).map_err(|error| GrepError(rg_message(error)))?;
	builder.build().map_err(|error| GrepError(rg_message(error)))
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;

	struct TempDir(PathBuf);

	impl TempDir {
		fn new(name: &str) -> Self {
			let path = std::env::temp_dir().join(format!("pi-grep-{name}-{}", std::process::id()));
			let _ = fs::remove_dir_all(&path);
			fs::create_dir_all(&path).unwrap();
			Self(path)
		}

		fn file(&self, relative: &str, contents: &[u8]) -> &Self {
			let path = self.0.join(relative);
			fs::create_dir_all(path.parent().unwrap()).unwrap();
			fs::write(path, contents).unwrap();
			self
		}
	}

	impl Drop for TempDir {
		fn drop(&mut self) {
			let _ = fs::remove_dir_all(&self.0);
		}
	}

	fn options(path: &Path, pattern: &str) -> GrepOptions {
		GrepOptions {
			pattern: pattern.to_owned(),
			search_path: path.to_path_buf(),
			glob: None,
			ignore_case: false,
			fixed_strings: false,
			max_matches: None,
			context: 0,
		}
	}

	fn run(options: &GrepOptions) -> Vec<(String, u64, String)> {
		let output = grep(options, &AtomicBool::new(false)).unwrap();
		output
			.matches
			.into_iter()
			.map(|m| {
				let path = m.path.unwrap();
				let relative = Path::new(&path)
					.strip_prefix(&options.search_path)
					.unwrap_or(Path::new(&path));
				(
					relative.to_string_lossy().replace('\\', "/"),
					m.line_number,
					m.line.unwrap(),
				)
			})
			.collect()
	}

	#[test]
	fn reports_matching_lines_per_file_in_line_order() {
		let dir = TempDir::new("order");
		dir.file("b.txt", b"one\nneedle two\n")
			.file("a/c.txt", b"needle\nx\nneedle again\r\n");
		// Files come in completion order like ripgrep; each file's lines stay in order.
		let mut matches = run(&options(&dir.0, "needle"));
		matches.sort_by(|a, b| a.0.cmp(&b.0).then(a.1.cmp(&b.1)));
		assert_eq!(
			matches,
			[
				("a/c.txt".to_owned(), 1, "needle\n".to_owned()),
				("a/c.txt".to_owned(), 3, "needle again\r\n".to_owned()),
				("b.txt".to_owned(), 2, "needle two\n".to_owned()),
			]
		);
	}

	#[test]
	fn case_literal_and_glob() {
		let dir = TempDir::new("flags");
		dir.file("a.ts", b"Foo.bar\n").file("b.js", b"foo.bar\n");
		let mut opts = options(&dir.0, "foo.bar");
		assert_eq!(run(&opts).len(), 1);
		opts.ignore_case = true;
		assert_eq!(run(&opts).len(), 2);
		opts.pattern = "o.b".to_owned();
		opts.fixed_strings = true;
		assert_eq!(run(&opts).len(), 2);
		opts.pattern = "oxb".to_owned();
		opts.fixed_strings = false;
		assert_eq!(run(&opts).len(), 0);
		opts.pattern = "foo".to_owned();
		opts.glob = Some("*.ts".to_owned());
		assert_eq!(run(&opts), [("a.ts".to_owned(), 1, "Foo.bar\n".to_owned())]);
	}

	#[test]
	fn skips_binary_files_in_directories_but_not_explicit_files() {
		let dir = TempDir::new("binary");
		dir.file("bin.dat", b"needle\0\n").file("text.txt", b"needle\n");
		assert_eq!(run(&options(&dir.0, "needle")).len(), 1);
		assert_eq!(run(&options(&dir.0.join("bin.dat"), "needle")).len(), 1);
	}

	#[test]
	fn transcodes_utf16_with_bom() {
		let dir = TempDir::new("utf16");
		let mut bytes = vec![0xFF, 0xFE];
		for unit in "line\nneedle\n".encode_utf16() {
			bytes.extend_from_slice(&unit.to_le_bytes());
		}
		dir.file("u16.txt", &bytes);
		assert_eq!(
			run(&options(&dir.0, "needle")),
			[("u16.txt".to_owned(), 2, "needle\n".to_owned())]
		);
	}

	#[test]
	fn max_matches_stops_early() {
		let dir = TempDir::new("limit");
		for i in 0..10 {
			dir.file(&format!("f{i}.txt"), b"x\nx\nx\n");
		}
		let mut opts = options(&dir.0, "x");
		opts.max_matches = Some(5);
		let output = grep(&opts, &AtomicBool::new(false)).unwrap();
		assert_eq!(output.matches.len(), 5);
		assert!(output.limit_reached);
	}

	#[test]
	fn errors_match_ripgrep() {
		let dir = TempDir::new("errors");
		assert_eq!(
			grep(&options(&dir.0, "("), &AtomicBool::new(false)).unwrap_err().0,
			"rg: regex parse error:\n    (?:()\n    ^\nerror: unclosed group"
		);
		let mut opts = options(&dir.0, "x");
		opts.glob = Some("[".to_owned());
		assert_eq!(
			grep(&opts, &AtomicBool::new(false)).unwrap_err().0,
			"rg: error parsing glob '[': unclosed character class; missing ']'"
		);
	}

	#[test]
	fn parallel_restart_matches_sequential_search() {
		let dir = TempDir::new("restart");
		for a in 0..20 {
			for b in 0..5 {
				dir.file(&format!("d{a}/f{b}.txt"), b"x\nneedle\ny\nneedle\n");
			}
		}
		let opts = options(&dir.0, "needle");
		let cancel = AtomicBool::new(false);
		let sequential = grep_with_budget(&opts, &cancel, usize::MAX).unwrap();
		let parallel = grep_with_budget(&opts, &cancel, 0).unwrap();
		assert_eq!(sequential.matches.len(), 200);
		let key = |m: &GrepMatch| (m.path.clone(), m.line_number);
		let mut sequential: Vec<_> = sequential.matches.iter().map(key).collect();
		let mut parallel: Vec<_> = parallel.matches.iter().map(key).collect();
		sequential.sort();
		parallel.sort();
		assert_eq!(sequential, parallel);
	}

	#[test]
	fn context_lines_follow_the_tool_line_splitting() {
		let dir = TempDir::new("context");
		dir.file("a.txt", b"one\r\ntwo\rthree\nneedle\nfive\n");
		let mut opts = options(&dir.0, "needle");
		opts.context = 2;
		let output = grep(&opts, &AtomicBool::new(false)).unwrap();
		// "\r" alone splits lines for the tool, but ripgrep numbers "two\rthree" as one line (line 2).
		assert_eq!(output.matches[0].line_number, 3);
		assert_eq!(
			output.matches[0].context,
			Some(ContextBlock {
				start: 1,
				lines: vec![
					"one".into(),
					"two".into(),
					"three".into(),
					"needle".into(),
					"five".into()
				]
			})
		);
	}

	#[test]
	fn cancel_stops_the_search() {
		let dir = TempDir::new("cancel");
		dir.file("a.txt", b"x\n");
		let output = grep(&options(&dir.0, "x"), &AtomicBool::new(true)).unwrap();
		assert!(output.matches.is_empty());
	}
}
