# Documentation index

Progressive disclosure: read the smallest document that answers your question.
`AGENTS.md` at the repository root is the routing table for coding agents.

| Need | Read |
|---|---|
| Run, build, package, verify | [../README.md](../README.md) |
| User-facing behavior | [features.md](features.md) |
| Shortcuts / profiles | [keys.md](keys.md) |
| Architecture and decisions | [design.md](design.md) |
| Command/keymap design | [keymaps.md](keymaps.md) |
| Git/PTY/LSP service boundaries | [services.md](services.md) |
| Full test coverage | [verification.md](verification.md) |
| Markdown/preview benchmark | [benchmarks.md](benchmarks.md) |
| Source ownership | [code-map.md](code-map.md) |
| macOS packaging/signing | [packaging.md](packaging.md) |
| Milestones and gaps | [roadmap.md](roadmap.md) |
| Review/optimization log | [review.md](review.md) |
| Current improvement work plan | [improvement-plan.md](improvement-plan.md) |
| Session 5 reduction work plan | [reduction-plan.md](reduction-plan.md) |
| Delivered improvement history (cycles 1–3) | [improvement-log.md](improvement-log.md) |
| Minimap design and delivery notes | [minimap.md](minimap.md) |
| Color emoji rendering | [emoji.md](emoji.md) |
| Color emoji + VS Code file-icon theme plan | [emoji-and-file-icons-plan.md](emoji-and-file-icons-plan.md) |

## Maintenance rules

- Keep this index and the root `AGENTS.md` file-routing table current when a
  module moves or a new document is added.
- Prefer one canonical home per fact. Link to it instead of copying paragraphs
  into the README.
- The README is intentionally short (overview, quick start, verification,
  performance summary, links). Put details under `docs/`.
- Architecture decisions belong in `design.md`; interaction/keymap decisions in
  `keymaps.md`; service boundaries in `services.md`.
- Benchmark methodology and budgets belong in `benchmarks.md`.
