# @ruwanego/pi-native

Rust N-API bridge for pi. The Rust source lives in `crates/pi-native` at the repository root.

```ts
import native from "@ruwanego/pi-native";

native.rustVersion();
```

`npm run build` compiles the crate with `cargo` and writes `prebuilds/<platform>-<arch>/pi-native.node`.
The addon is loaded lazily on first call.

## Protocol codec

`protocolCodec` is a Rust implementation of `@earendil-works/pi-protocol` CBOR and framing (`crates/pi-protocol`).
Install it with `setNativeCodec(protocolCodec)` from `@earendil-works/pi-protocol`; it is not enabled by default.

## Find

`findFiles` is a Rust implementation of the `fd` search behind the coding-agent `find` tool (`crates/pi-find`). It uses
fd's own matching and ignore crates and settings, and returns fd's output lines and error text. Install it with
`setNativeFind(findFiles)` from `@earendil-works/pi-coding-agent`; it is not enabled by default. Unlike fd, results are
always sorted (fd sorts only searches that finish within 100 ms).

## Grep

`grepFiles` is a Rust implementation of the ripgrep search behind the coding-agent `grep` tool (`crates/pi-grep`). It
uses ripgrep's own crates and settings and returns ripgrep's match messages, stderr text and error status. Install it
with `setNativeGrep(grepFiles)` from `@earendil-works/pi-coding-agent`; it is not enabled by default. Unlike ripgrep,
matching files are reported in path order, and ripgrep configuration files (`RIPGREP_CONFIG_PATH`) are not read.

## Streaming JSON

`parseStreamingJsonFast` is a Rust fast path for pi-ai's `parseStreamingJson` (`crates/pi-json`), which pi-ai calls
with every growing prefix of streamed tool-call arguments. It handles complete JSON and well-formed prefixes of an
object or array and returns `undefined` for anything else, so pi-ai runs its TypeScript implementation. Install it
with `setNativeStreamingJsonParser(parseStreamingJsonFast)` from `@earendil-works/pi-ai`.

## Enable everything

`PI_RUST=1 pi` switches every Rust implementation on at startup. From the SDK, call `enableRustFeatures()` from
`@earendil-works/pi-coding-agent`; it throws without changing anything if the addon is not built.
