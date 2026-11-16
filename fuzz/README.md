# QWE Fuzz Targets

These targets exercise production parsers and state machines from
`qwe-runtime`. They are not standalone implementations.

```sh
just fuzz-sandbox-control 10000
just fuzz-terminal-output 10000
just fuzz-path-resolution 10000
```

Both commands require `cargo-fuzz` and a nightly toolchain. Generated corpora,
artifacts, and build output remain under the ignored `fuzz/` directories.

- `sandbox_control`: fragmented and coalesced Pane control frames;
- `terminal_output`: fragmented and coalesced untrusted terminal output.
- `path_resolution`: arbitrary non-UTF-8 paths against a root containing
  relative and escaping symlinks, loops, and deep nesting.

The normal workspace suite retains deterministic tests for the same production
types, so CI does not depend on the fuzz toolchain.
