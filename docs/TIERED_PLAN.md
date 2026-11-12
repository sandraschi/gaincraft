# GainCraft - Tiered Execution Plan

**Parent Standard:** [`RUST_ARTCRAFT_STANDARD.md`](../../mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md)  
**Menu Bar Standard:** [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md)  
**Architecture Model:** 4-Tier ArtCraft Incremental Delivery  
**Status:** Tiers 1-4 done in 0.3.0 (publishing a GitHub release is a separate, manual step)

---

## Architecture Flow

```
Tier 1: Core Engine ──> Tier 2: MCP / CLI ──> Tier 3: Desktop UI ──> Tier 4: Menu Bar, Help & Packaging
(gaincraft-core)       (gaincraft-mcp)       (gaincraft-app)       (Hardened Workstation)
```

---

## Tier Specifications

### Tier 1 - Core Customer Health & CTA Engine (`gaincraft-core`)
- [x] L1 `Command::dispatch` universal customer health execution bus.
- [x] SQLite WAL database schema for accounts, daily usage, signals, and CTAs (schema 2, in-place migration from 1).
- [x] Explainable deterministic health scoring (adoption, recency, sentiment, modifier).
- [x] Five automated CTA rules with lifecycle tracking (open, resolved, dismissed), plus manual and termination CTAs.
- [x] Cross-craft event drain (`contract.executed` / `amended` / `terminated` from ContractCraft, `signal.negative` / `positive` from DeskCraft/FormCraft).
- [x] Churn audit and backtest of retired accounts; health report export; database backup.
- [x] Strict privacy boundary: account aggregates only, zero employee surveillance.
- [x] `unsafe_code = "forbid"` (workspace lint, since 0.3.0) and zero-panic clippy lints.

### Tier 2 - Headless Driver & MCP (`gaincraft-mcp`)
- [x] Unified CLI `gaincraft` with `--json` output; every tool is a verb (test).
- [x] MCP stdio server over JSON-RPC 2.0 (20 tools); stdout is JSON only (live probe).
- [x] Usage CSV import with validation and idempotent upsert.
- [x] Headless `--smoke` verification without window initialization.

### Tier 3 - Desktop Workstation UI (`gaincraft-app`)
- [x] Immediate-mode `egui` 0.37 / `wgpu` workstation (`craft-biz-ui`).
- [x] Accounts table with health score pills and sparkline trends.
- [x] Customer 360 cockpit view with four-component breakdown and a 12-month health trend.
- [x] CTA cockpit Kanban board and renewal dates timeline.
- [x] CTA titles in the window's language (DE/EN).

### Tier 4 - Craft Menu Bar Standard, Help Suite & Hardened Verification
- [x] Universal 8-menu bar (`File`, `Edit`, `View`, `Insert`, `Modify`, `Commands`, `Window`, `Help`) with `switch_on_hover` (shared in `craft-biz-ui::menu`).
- [x] Mandatory 5-dialog Help suite (`Command Palette...`, `Keyboard Shortcuts...`, `Project Files & Docs...`, `Licenses & Notices...`, `About GainCraft`).
- [x] Fleet UI/UX Pattern 2 adherence: quick view row mapped to View items, panel visibility managed via `View` / `Window`.
- [x] Tofu Guard verification: every menu label, shortcut row and navigation label is checked against the loaded fonts (DE and EN); the guard itself is tested against a glyph no bundled font has.
- [x] Headless `--smoke` parity validation: menu items name real tools and arguments, no duplicate shortcuts.
- [x] Standalone release binaries, per-user NSIS installer, mcpb bundle (`scripts/release.ps1`, `just release-dry`).
