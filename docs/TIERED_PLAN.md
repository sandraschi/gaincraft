# GainCraft — Tiered Execution Plan

**Parent Standard:** [`RUST_ARTCRAFT_STANDARD.md`](../../mcp-central-docs/standards/RUST_ARTCRAFT_STANDARD.md)  
**Menu Bar Standard:** [`CRAFT_MENU_BAR_STANDARD.md`](../../mcp-central-docs/standards/rust-artcraft/CRAFT_MENU_BAR_STANDARD.md)  
**Architecture Model:** 4-Tier ArtCraft Incremental Delivery  
**Status:** Active  

---

## Architecture Flow

```
Tier 1: Core Engine ──> Tier 2: MCP / CLI ──> Tier 3: Desktop UI ──> Tier 4: Menu Bar, Help & Packaging
(gaincraft-core)       (gaincraft-mcp)       (gaincraft-app)       (Hardened Workstation)
```

---

## Tier Specifications

### Tier 1 — Core Customer Health & CTA Engine (`gaincraft-core`)
- [x] L1 `Command::dispatch` universal customer health execution bus.
- [x] SQLite WAL database schema for accounts, daily usage, signals, and CTAs.
- [x] Explainable deterministic health scoring (adoption, recency, sentiment, modifier).
- [x] Five automated CTA rules with lifecycle tracking (open, in work, resolved).
- [x] Cross-craft event drain (`contract.executed` from ContractCraft, `signal.negative` from DeskCraft/FormCraft).
- [x] Strict privacy boundary: account aggregates only, zero employee surveillance.
- [x] `#![forbid(unsafe_code)]` and zero-panic clippy lints.

### Tier 2 — Headless Driver & FastMCP (`gaincraft-mcp`)
- [x] Unified CLI `gaincraft-cli` with `--json` output.
- [x] FastMCP stdio server communicating over JSON-RPC 2.0 (16 tools).
- [x] Usage CSV import with validation and idempotent upsert.
- [x] Headless `--smoke` verification without window initialization.

### Tier 3 — Desktop Workstation UI (`gaincraft-app`)
- [x] Immediate-mode `egui` 0.37 / `wgpu` workstation (`craft-biz-ui`).
- [x] Accounts table with health score pills and sparkline trends.
- [x] Customer 360 cockpit view with four-component breakdown.
- [x] CTA cockpit Kanban board and renewal dates timeline.

### Tier 4 — Craft Menu Bar Standard, Help Suite & Hardened Verification
- [ ] Universal 8-menu bar (`File`, `Edit`, `View`, `Insert`, `Modify`, `Commands`, `Window`, `Help`) with `switch_on_hover`.
- [ ] Mandatory 5-dialog Help suite (`Command Palette...`, `Keyboard Shortcuts...`, `Project Files & Docs...`, `Licenses & Notices...`, `About GainCraft`).
- [ ] Fleet UI/UX Pattern 2 adherence: viewport-centric navigation, top button row mapped to View toggles, panel visibility managed via `View` / `Window`.
- [ ] Tofu Guard verification (100% font glyph coverage, zero tofu placeholder boxes).
- [ ] Headless `--smoke` parity validation.
- [ ] Standalone release binary packaging and NSIS installer.
