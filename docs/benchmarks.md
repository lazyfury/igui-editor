# Benchmarks

The Markdown preview is the most expensive interactive path: every edit bumps
the document revision, which invalidates the parsed block list and the preview
layout. The benchmark target measures each stage separately and also drives the
real app synchronously for paint/typing measurements.

## Run

```bash
scripts/bench.sh
# equivalent:
cargo bench --features bench --bench markdown
```

The bench feature enables the headless recorder and builds an optimized
`bench` profile. No OS window is opened.

## Fixture

The generated fixture is ~129 KB / 4,800 lines: 240 sections with headings,
wrapping paragraphs, inline styles/links, lists, fenced code, and tables. It is
intentionally larger than the 48 KB preview-debounce threshold.

## What is measured

| Measurement | What it runs |
|---|---|
| `markdown parse` | `pulldown-cmark` + block folding only |
| `preview layout` | parse once, then block → draw-primitive layout with a deterministic measurer |
| `preview paint` | real `EditorApp` update/layout/paint for steady-state frames (cache hit), counting draw commands |
| `typing (16 ms frame)` | one text insertion per frame at ~60 fps, real app pipeline; sleep is excluded from the reported work time |

## Reference numbers

macOS arm64, release/bench profile, 0.1.0:

```text
fixture: 128885 bytes, 4800 lines
markdown parse            1.5 ms/iter
preview layout            2.0 ms/iter      ~19k primitives
preview paint             0.04 ms/iter      ~410 visible commands
typing (16 ms frame)      1.1 ms/frame work
```

Before the preview coalescing change, the same typing benchmark was ~7.0 ms of
work per frame because it parsed and laid out the full document on every edit.
The current path:

1. caches parsed blocks per `(document id, revision)`;
2. coalesces full parse/layout for Markdown above 48 KB with a 120 ms trailing
   debounce;
3. keeps the previous layout visible while typing and wakes the frame loop at
   the deadline (`PreviewUi::rebuild_pending`);
4. culls off-screen primitives before issuing draw commands;
5. shares block anchors through `Rc` instead of cloning them per frame.

## Budgets

- Typing benchmark work must stay **< 2 ms/frame** on the fixture above.
- A regression over **20%** in parse/layout/paint requires a note in the commit
  and an update to this document.
- The shipped `.app` binary should stay **< 5 MB** on macOS arm64; packaging
  uses `--no-default-features` so the self-check recorder is not linked.

## Profiling tips

- Run `scripts/bench.sh` before and after a preview change.
- `sample <pid> 5` works on a locally launched macOS build when the issue only
  appears with a real window.
- The self-check is the correctness gate; the benchmark is the performance
  gate. Both must pass.
