//! Rust port of the `fd` search behind the coding-agent `find` tool.
//!
//! The tool runs `fd --glob --color=never --hidden [--no-require-git] --max-results N [--full-path] -- PATTERN PATH`.
//! [`find`] reproduces that invocation with the crates fd itself is built on (`ignore`, `globset`, `regex`): the
//! same glob-to-regex translation, smart case, ignore files (`.gitignore`, `.ignore`, `.fdignore`, git excludes,
//! the global fd ignore file), output format (absolute path plus a trailing separator for directories) and error
//! messages (fd's stderr). Matching logic follows fd 10 (`src/main.rs`, `src/walk.rs`, `src/regex_helper.rs`).
//!
//! One intentional difference: results are always sorted. fd sorts only when the search finishes within 100 ms,
//! otherwise it streams results in walk order.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};

use etcetera::BaseStrategy;
use globset::GlobBuilder;
use ignore::{WalkBuilder, WalkState};
use regex::bytes::{Regex, RegexBuilder};
use regex_syntax::ParserBuilder;
use regex_syntax::hir::{Capture, Class, Hir, HirKind, Literal, Repetition};

/// Arguments of one fd invocation.
#[derive(Debug, Clone)]
pub struct FindOptions {
	/// Glob pattern, matched against the file name, or against the absolute path when `full_path` is set.
	pub pattern: String,
	/// Directory to search. Results are printed as `search_path` joined with the relative entry path.
	pub search_path: PathBuf,
	/// `--full-path`.
	pub full_path: bool,
	/// `false` passes `--no-require-git`: read `.gitignore` files outside git repositories too.
	pub require_git: bool,
	/// `--max-results`. `None` (or fd's `0`) means unlimited.
	pub max_results: Option<usize>,
}

/// A failure fd reports on stderr, formatted exactly as fd prints it (without the trailing newline).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FindError(pub String);

impl std::fmt::Display for FindError {
	fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
		f.write_str(&self.0)
	}
}

impl std::error::Error for FindError {}

fn fd_error(message: impl std::fmt::Display) -> FindError {
	FindError(format!("[fd error]: {message}"))
}

struct Match {
	path: PathBuf,
	is_dir: bool,
}

/// Runs the search and returns fd's output lines, sorted. Returns early with the matches found so far once
/// `cancel` is set.
pub fn find(options: &FindOptions, cancel: &AtomicBool) -> Result<Vec<String>, FindError> {
	find_with_budget(options, cancel, SEQUENTIAL_BUDGET)
}

fn find_with_budget(options: &FindOptions, cancel: &AtomicBool, budget: usize) -> Result<Vec<String>, FindError> {
	let search_path = search_path(&options.search_path)?;
	ensure_pattern_is_not_a_path(options)?;
	let regex = build_regex(&options.pattern)?;
	let mut walker = build_walker(&search_path, options.require_git);
	let search = Search {
		options,
		regex,
		max_results: options.max_results.filter(|&max| max > 0),
		cancel,
		matches: Mutex::new(Vec::new()),
		done: AtomicBool::new(false),
	};

	// Starting threads costs more than walking a small tree, so walk on this thread first and switch to the
	// parallel walker (starting over) only if the tree turns out to be large.
	let mut finished = !is_large_tree(&search_path);
	if finished {
		for (visited, entry) in walker.build().enumerate() {
			// The sequential walker reports traversal errors differently from fd's parallel walker; let the
			// parallel walker handle them.
			if entry.is_err() {
				finished = false;
				break;
			}
			if matches!(search.visit(entry), WalkState::Quit) {
				break;
			}
			if visited >= budget {
				finished = false;
				remember_tree_size(&search_path, true);
				break;
			}
		}
		if finished {
			remember_tree_size(&search_path, false);
		}
	}
	if !finished {
		search.matches.lock().expect("matches lock").clear();
		walker.threads(default_threads()).build_parallel().run(|| {
			let search = &search;
			Box::new(move |entry| search.visit(entry))
		});
	}

	let mut matches = search.matches.into_inner().expect("matches lock");
	matches.sort_by(|a, b| a.path.cmp(&b.path));
	let separator = path_separator();
	Ok(matches
		.into_iter()
		.map(|entry| {
			let mut line = entry.path.to_string_lossy().into_owned();
			if separator != std::path::MAIN_SEPARATOR_STR {
				line = line.replace(std::path::MAIN_SEPARATOR, separator);
			}
			if entry.is_dir {
				line.push_str(separator);
			}
			line
		})
		.collect())
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
const SEQUENTIAL_BUDGET: usize = 500;

struct Search<'a> {
	options: &'a FindOptions,
	regex: Regex,
	max_results: Option<usize>,
	cancel: &'a AtomicBool,
	matches: Mutex<Vec<Match>>,
	done: AtomicBool,
}

impl Search<'_> {
	/// fd's per-entry filter (`walk.rs`, `spawn_senders`) for the options the find tool uses.
	fn visit(&self, entry: Result<ignore::DirEntry, ignore::Error>) -> WalkState {
		if self.done.load(Ordering::Relaxed) || self.cancel.load(Ordering::Relaxed) {
			return WalkState::Quit;
		}
		let (path, is_dir) = match entry {
			// Skip the root directory entry.
			Ok(entry) if entry.depth() == 0 => return WalkState::Continue,
			Ok(entry) => {
				let is_dir = entry.file_type().is_some_and(|file_type| file_type.is_dir());
				(entry.into_path(), is_dir)
			}
			Err(ignore::Error::WithPath { path, err }) if is_broken_symlink(&path, &err) => (path, false),
			// fd hides filesystem errors unless --show-errors is passed.
			Err(_) => return WalkState::Continue,
		};

		let haystack = if self.options.full_path {
			path.as_os_str()
		} else {
			match path.file_name() {
				Some(name) => name,
				None => return WalkState::Continue,
			}
		};
		if !self.regex.is_match(&os_str_bytes(haystack)) {
			return WalkState::Continue;
		}

		let mut matches = self.matches.lock().expect("matches lock");
		if self.max_results.is_some_and(|max| matches.len() >= max) {
			self.done.store(true, Ordering::Relaxed);
			return WalkState::Quit;
		}
		matches.push(Match { path, is_dir });
		if self.max_results.is_some_and(|max| matches.len() >= max) {
			self.done.store(true, Ordering::Relaxed);
			return WalkState::Quit;
		}
		WalkState::Continue
	}
}

/// fd's `Opts::search_paths` + `normalize_path` for a single path argument.
fn search_path(path: &Path) -> Result<PathBuf, FindError> {
	let is_existing_directory = path.is_dir() && (path.file_name().is_some() || std::fs::canonicalize(path).is_ok());
	if !is_existing_directory {
		return Err(FindError(format!(
			"[fd error]: Search path '{}' is not a directory.\n[fd error]: No valid search paths given.",
			path.to_string_lossy()
		)));
	}
	// fd turns "." into "./" as a workaround for https://github.com/BurntSushi/ripgrep/pull/2711.
	Ok(if path == Path::new(".") {
		PathBuf::from("./")
	} else {
		path.to_path_buf()
	})
}

fn ensure_pattern_is_not_a_path(options: &FindOptions) -> Result<(), FindError> {
	let pattern = &options.pattern;
	if !options.full_path && pattern.contains(std::path::MAIN_SEPARATOR) && Path::new(pattern).is_dir() {
		let separator = std::path::MAIN_SEPARATOR;
		return Err(fd_error(format!(
			"The search pattern '{pattern}' contains a path-separation character ('{separator}') \
			 and will not lead to any search results.\n\n\
			 If you want to search for all files inside the '{pattern}' directory, use a match-all pattern:\n\n  \
			 fd . '{pattern}'\n\n\
			 Instead, if you want your pattern to match the full file path, use:\n\n  \
			 fd --full-path '{pattern}'"
		)));
	}
	Ok(())
}

/// fd's `build_pattern_regex` (glob mode) and `build_regex`, with smart case.
fn build_regex(pattern: &str) -> Result<Regex, FindError> {
	let regex = if pattern.is_empty() {
		String::new()
	} else {
		let glob = GlobBuilder::new(pattern)
			.literal_separator(true)
			.build()
			.map_err(fd_error)?;
		glob.regex().to_owned()
	};
	let case_sensitive = pattern_has_uppercase_char(&regex);
	RegexBuilder::new(&regex)
		.case_insensitive(!case_sensitive)
		.dot_matches_new_line(true)
		.build()
		.map_err(|error| {
			fd_error(format!(
				"{error}\n\nNote: You can use the '--fixed-strings' option to search for a \
				 literal string instead of a regular expression. Alternatively, you can \
				 also use the '--glob' option to match on a glob pattern."
			))
		})
}

/// fd's walker configuration for `--hidden` with every ignore source enabled.
fn build_walker(search_path: &Path, require_git: bool) -> WalkBuilder {
	let mut builder = WalkBuilder::new(search_path);
	builder
		.hidden(false)
		.ignore(true)
		.parents(true)
		.git_ignore(true)
		.git_global(true)
		.git_exclude(true)
		.require_git(require_git)
		.follow_links(false)
		.same_file_system(false)
		.max_depth(None);
	builder.add_custom_ignore_filename(".fdignore");
	if let Ok(base_dirs) = etcetera::choose_base_strategy() {
		let global_ignore_file = base_dirs.config_dir().join("fd").join("ignore");
		// fd prints malformed-pattern errors here but keeps searching; the output is unaffected.
		if global_ignore_file.is_file() {
			let _ = builder.add_ignore(global_ignore_file);
		}
	}
	builder
}

fn default_threads() -> usize {
	std::thread::available_parallelism()
		.map_or(1, |threads| threads.get())
		.min(64)
}

/// Broken symlinks surface as walk errors when links are followed; fd reports them as entries.
fn is_broken_symlink(path: &Path, error: &ignore::Error) -> bool {
	matches!(error, ignore::Error::Io(io_error) if io_error.kind() == std::io::ErrorKind::NotFound)
		&& path
			.symlink_metadata()
			.is_ok_and(|metadata| metadata.file_type().is_symlink())
}

/// fd's default output separator: `/` under MSYS on Windows, the platform separator otherwise.
fn path_separator() -> &'static str {
	if cfg!(windows) && std::env::var("MSYSTEM").is_ok_and(|msystem| !msystem.is_empty()) {
		"/"
	} else {
		std::path::MAIN_SEPARATOR_STR
	}
}

#[cfg(unix)]
fn os_str_bytes(input: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
	use std::os::unix::ffi::OsStrExt;
	std::borrow::Cow::Borrowed(input.as_bytes())
}

#[cfg(not(unix))]
fn os_str_bytes(input: &std::ffi::OsStr) -> std::borrow::Cow<'_, [u8]> {
	match input.to_string_lossy() {
		std::borrow::Cow::Owned(string) => std::borrow::Cow::Owned(string.into_bytes()),
		std::borrow::Cow::Borrowed(string) => std::borrow::Cow::Borrowed(string.as_bytes()),
	}
}

/// Whether a regex contains a literal uppercase character (fd's smart case).
fn pattern_has_uppercase_char(pattern: &str) -> bool {
	let mut parser = ParserBuilder::new().utf8(false).build();
	parser
		.parse(pattern)
		.map(|hir| hir_has_uppercase_char(&hir))
		.unwrap_or(false)
}

fn hir_has_uppercase_char(hir: &Hir) -> bool {
	match hir.kind() {
		HirKind::Literal(Literal(bytes)) => match std::str::from_utf8(bytes) {
			Ok(s) => s.chars().any(|c| c.is_uppercase()),
			Err(_) => bytes.iter().any(|b| char::from(*b).is_uppercase()),
		},
		HirKind::Class(Class::Unicode(ranges)) => ranges
			.iter()
			.any(|r| r.start().is_uppercase() || r.end().is_uppercase()),
		HirKind::Class(Class::Bytes(ranges)) => ranges
			.iter()
			.any(|r| char::from(r.start()).is_uppercase() || char::from(r.end()).is_uppercase()),
		HirKind::Capture(Capture { sub, .. }) | HirKind::Repetition(Repetition { sub, .. }) => {
			hir_has_uppercase_char(sub)
		}
		HirKind::Concat(hirs) | HirKind::Alternation(hirs) => hirs.iter().any(hir_has_uppercase_char),
		_ => false,
	}
}

#[cfg(test)]
mod tests {
	use super::*;
	use std::fs;

	struct TempDir(PathBuf);

	impl TempDir {
		fn new(name: &str) -> Self {
			let path = std::env::temp_dir().join(format!("pi-find-{name}-{}", std::process::id()));
			let _ = fs::remove_dir_all(&path);
			fs::create_dir_all(&path).unwrap();
			Self(path)
		}

		fn file(&self, relative: &str, contents: &str) -> &Self {
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

	fn run(root: &Path, pattern: &str, full_path: bool, max_results: Option<usize>) -> Result<Vec<String>, FindError> {
		let options = FindOptions {
			pattern: pattern.to_owned(),
			search_path: root.to_path_buf(),
			full_path,
			require_git: false,
			max_results,
		};
		find(&options, &AtomicBool::new(false))
	}

	fn relative(root: &Path, lines: Vec<String>) -> Vec<String> {
		let prefix = format!("{}{}", root.display(), std::path::MAIN_SEPARATOR);
		lines
			.into_iter()
			.map(|line| line.strip_prefix(&prefix).unwrap().replace('\\', "/"))
			.collect()
	}

	#[test]
	fn matches_basename_and_marks_directories() {
		let dir = TempDir::new("basename");
		dir.file("a.ts", "")
			.file("src/b.ts", "")
			.file("src/c.js", "")
			.file("x.ts/inner", "");
		let lines = run(&dir.0, "*.ts", false, None).unwrap();
		assert_eq!(relative(&dir.0, lines), ["a.ts", "src/b.ts", "x.ts/"]);
	}

	#[test]
	fn full_path_patterns_respect_separators() {
		let dir = TempDir::new("full-path");
		dir.file("src/a.spec.ts", "")
			.file("src/deep/b.spec.ts", "")
			.file("lib/c.spec.ts", "");
		let lines = run(&dir.0, "**/src/*.spec.ts", true, None).unwrap();
		assert_eq!(relative(&dir.0, lines), ["src/a.spec.ts"]);
		let lines = run(&dir.0, "**/src/**/*.spec.ts", true, None).unwrap();
		assert_eq!(relative(&dir.0, lines), ["src/a.spec.ts", "src/deep/b.spec.ts"]);
	}

	#[test]
	fn smart_case() {
		let dir = TempDir::new("smart-case");
		dir.file("Readme.md", "").file("readme.txt", "");
		assert_eq!(
			relative(&dir.0, run(&dir.0, "readme*", false, None).unwrap()),
			["Readme.md", "readme.txt"]
		);
		assert_eq!(
			relative(&dir.0, run(&dir.0, "Readme*", false, None).unwrap()),
			["Readme.md"]
		);
	}

	#[test]
	fn hidden_files_and_ignore_files() {
		let dir = TempDir::new("ignore");
		dir.file(".env", "")
			.file(".gitignore", "*.log\n")
			.file("a.log", "")
			.file(".fdignore", "skip.txt\n")
			.file("skip.txt", "")
			.file("keep.txt", "")
			.file("sub/.ignore", "b.txt\n")
			.file("sub/b.txt", "");
		let lines = run(&dir.0, "*", false, None).unwrap();
		assert_eq!(
			relative(&dir.0, lines),
			[".env", ".fdignore", ".gitignore", "keep.txt", "sub/", "sub/.ignore"]
		);
	}

	#[test]
	fn max_results_limits_output() {
		let dir = TempDir::new("limit");
		for i in 0..20 {
			dir.file(&format!("f{i:02}.txt"), "");
		}
		assert_eq!(run(&dir.0, "*.txt", false, Some(5)).unwrap().len(), 5);
		assert_eq!(run(&dir.0, "*.txt", false, Some(0)).unwrap().len(), 20);
	}

	#[test]
	fn empty_pattern_matches_everything() {
		let dir = TempDir::new("empty");
		dir.file("a", "").file("b/c", "");
		assert_eq!(
			relative(&dir.0, run(&dir.0, "", false, None).unwrap()),
			["a", "b/", "b/c"]
		);
	}

	#[test]
	fn errors_match_fd() {
		let dir = TempDir::new("errors");
		assert_eq!(
			run(&dir.0, "[", false, None).unwrap_err().0,
			"[fd error]: error parsing glob '[': unclosed character class; missing ']'"
		);
		let missing = dir.0.join("missing");
		assert_eq!(
			run(&missing, "*", false, None).unwrap_err().0,
			format!(
				"[fd error]: Search path '{}' is not a directory.\n[fd error]: No valid search paths given.",
				missing.display()
			)
		);
	}

	#[test]
	fn parallel_restart_matches_sequential_walk() {
		let dir = TempDir::new("restart");
		dir.file(".gitignore", "*.log\n").file("x.log", "");
		for a in 0..20 {
			for b in 0..5 {
				dir.file(&format!("d{a}/e{b}/f.txt"), "")
					.file(&format!("d{a}/e{b}/g.rs"), "");
			}
		}
		let options = FindOptions {
			pattern: "*".to_owned(),
			search_path: dir.0.clone(),
			full_path: false,
			require_git: false,
			max_results: None,
		};
		let cancel = AtomicBool::new(false);
		let sequential = find_with_budget(&options, &cancel, usize::MAX).unwrap();
		let parallel = find_with_budget(&options, &cancel, 0).unwrap();
		assert_eq!(sequential.len(), 20 * 5 * 2 + 20 * 5 + 20 + 1);
		assert_eq!(sequential, parallel);
	}

	#[test]
	fn cancel_stops_the_walk() {
		let dir = TempDir::new("cancel");
		dir.file("a.txt", "");
		let options = FindOptions {
			pattern: "*".to_owned(),
			search_path: dir.0.clone(),
			full_path: false,
			require_git: false,
			max_results: None,
		};
		assert_eq!(find(&options, &AtomicBool::new(true)).unwrap(), Vec::<String>::new());
	}

	#[test]
	fn uppercase_detection() {
		assert!(pattern_has_uppercase_char("(?-u)^Foo$"));
		assert!(!pattern_has_uppercase_char("(?-u)^foo\\.ts$"));
		assert!(pattern_has_uppercase_char("[A-Z]"));
	}
}
