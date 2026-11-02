# Raw POC Results

Store structured results under a platform directory and then `rust/`, `go/`,
and `cpp/`. Use one JSON file per case and run:

```text
results/<platform>/<language>/<case-id>-<source-short-sha>-<run-id>.json
```

Follow [`../test-spec/result.schema.json`](../test-spec/result.schema.json).
Large logs, screen recordings, and profiles may be stored beside the JSON file
and referenced from `evidence`. Temporary files belong in `tmp/`.

Raw PTY logs, captured stdout/stderr, and environment snapshots are ignored by
Git because they may contain local paths or terminal content. Keep them locally
beside the structured result that references them.

Do not overwrite failed runs. Preserve reruns so failure rates and regressions
remain visible.

Historical unsplit runs are archived under
`results/macos-arm64/legacy/` and must not be mixed with Linux results.

