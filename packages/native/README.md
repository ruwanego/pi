# @earendil-works/pi-native

Rust N-API bridge for pi. The Rust source lives in `crates/pi-native` at the repository root.

```ts
import native from "@earendil-works/pi-native";

native.rustVersion();
```

`npm run build` compiles the crate with `cargo` and writes `prebuilds/<platform>-<arch>/pi-native.node`.
The addon is loaded lazily on first call.
