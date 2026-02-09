# 🤝 Final Consensus | Hotplate: Hướng phát triển tiếp theo | 2025-07-15

## Tổng quan
- **Chủ đề**: Hướng phát triển Hotplate sau Phase 4 (MCP Server 11/11 tools hoàn thành)
- **Số vòng thảo luận**: 2
- **Ngày bắt đầu → Đồng thuận**: 2025-07-15 → 2025-07-15
- **Participants**: GPT (Visionary), Gemini (Pragmatist)
- **Điều phối**: Orchestra

---

## Kết luận đồng thuận

### 1. Protocol Refactor — `broadcast<BrowserCommand>` enum + Structured JSON WS

**Quyết định**: Refactor protocol từ string-based sang structured JSON. Làm ĐẦU TIÊN trong sprint tiếp theo.

**Lý do**:
- *Visionary*: Foundation cost tăng theo thời gian. Protocol là API contract giữa 3 layers. Infrastructure before features.
- *Pragmatist*: Trigger condition met — đã xác nhận thêm 2+ tools mới. Refactor trước = tools mới viết trên protocol mới ngay, không port lại.

**Hành động tiếp theo**:
1. Thiết kế `enum BrowserCommand` với `#[serde(tag = "type", rename_all = "snake_case")]`
2. Refactor `server.rs` forwarding logic — bỏ `starts_with()` chain
3. Refactor `livereload.js` — `JSON.parse` + handler map thay `if/else if`
4. Gate: All 11 existing tools pass trên protocol mới → unlock tool development
5. **Effort cap**: 2-3 ngày max. MVP = enum + JSON parse. Không scope creep vào versioning/capability negotiation.

---

### 2. 3 MCP Tools Mới — navigate, click, user_events

**Quyết định**: Build 3 tools mới, nâng tổng lên **14 MCP tools**.

**Lý do**:
- *Visionary*: `user_events` là unique differentiator — AI passive observe user behavior. `navigate` essential cho dev-loop. `click` là dev-convenience (named eval).
- *Pragmatist*: `user_events` = Playwright MCP KHÔNG có (chỉ send, không listen). `navigate` = `location.href`, 0.5 ngày. `click` = `el.click()`, 0.5 ngày. Tổng ~3-5 ngày, ROI rõ ràng.

**Hành động tiếp theo**:

| Tool | Implementation | Effort | Ràng buộc |
|------|---------------|--------|-----------|
| `hotplate_navigate` | `location.href = url` trong `livereload.js` | 0.5-1 ngày | Đơn giản, không full Playwright navigation |
| `hotplate_click` | `document.querySelector(sel).click()` trong `livereload.js` | 0.5 ngày | **Không bao giờ upgrade** — document limitation rõ: "Dev-convenience only. For complex interactions, use Playwright MCP." |
| `hotplate_user_events` | Capture click/input/submit/change events + `UserEventBuffer` | 2-3 ngày | Passive listener, không poll. `cssPath()` helper cho stable selectors |

---

### 3. Self-healing Dev Loop — Structured Error Reporting

**Quyết định**: Upgrade `hotplate_console` với error classification, parsed stack traces, deduplication.

**Lý do**:
- *Cả hai đồng ý*: Hotplate là tool DUY NHẤT kết hợp live-reload + MCP + browser runtime = closed feedback loop. Self-healing dev loop là lý do Hotplate tồn tại. Ước tính x10 productivity gain. Cần structured errors để AI fix chính xác hơn.

**Hành động tiếp theo**:
1. `ConsoleEntry` thêm `error_type` (TypeError/SyntaxError/ReferenceError/NetworkError)
2. Parse stack trace → `parsed_stack: [{file, line, col, function}]`
3. Error deduplication bằng hash (message + source + line)
4. Viết `.hotplate/agent-prompt.md` — system prompt template cho AI agents: "Dùng hotplate_console sau mỗi reload, nếu có error → đọc source → fix → verify"
5. Draft blog: "AI fixes your bugs in real-time with Hotplate"

---

### 4. Positioning — "Design for B, Market as A"

**Quyết định**: Dual positioning strategy.

**Lý do**:
- *Visionary*: Dev server là commodity trong 3-5 năm. Vite + MCP plugin sẽ cover 80% use case. "AI-controlled browser runtime" là category mới, defensible moat.
- *Pragmatist*: VS Code Marketplace audience tìm "live server", không hiểu "browser runtime". Marketing A cho adoption ban đầu. Design B cho architecture extensibility.

**Hành động tiếp theo**:

| Channel | Message |
|---------|---------|
| VS Code Marketplace | "⚡ Live-reload dev server with built-in AI tools (MCP). Zero config HTTPS." |
| GitHub README | "Smart dev server with AI superpowers. The only dev server your AI agent can see." + subtle hint: "Live Server successor × AI-native browser runtime" |
| Blog / Dev.to | "How AI fixes your bugs in real-time: The self-healing dev loop" |
| MCP directories | "Browser runtime for AI agents. 14 MCP tools. Screenshot, DOM, eval, user events." |

**Mốc chuyển đổi sang positioning B full**: Khi đạt 10K+ installs + ≥3 community blog posts + MCP mainstream trong ≥2 IDEs.

---

### 5. Deferred Decisions — Render Tool + State Store

**Quyết định**: Defer cả hai, review evidence-based.

| Tool | Status | Trigger để build | Review deadline |
|------|--------|-----------------|-----------------|
| `hotplate_render` | ⏸️ Deferred | DOM pollution, race condition, hoặc >3 tool calls per render cycle khi demo bio-direct bằng inject+eval | Post-sprint (4-6 tuần) |
| `state_get/set` | ⏸️ Deferred | AI agent hack `window.__state` trong eval, hoặc community request, hoặc bio-direct demo cần persist state giữa >2 screens | 4-6 tuần |

---

### 6. Những gì KHÔNG làm

| Feature | Lý do loại bỏ | Ai champion loại |
|---------|---------------|------------------|
| Full click/type/fill tools | Playwright MCP đã mature, không duplicate | Cả hai |
| Workflow engine | Quá sớm, chưa validate use case | Cả hai |
| VS Code control panel | Effort/impact ratio tệ | Cả hai |
| Session replay | Cool nhưng low priority, ít dev cần hàng ngày | Cả hai |
| DOM snapshot graph | `hotplate_dom` hiện tại đủ cho hầu hết cases | Cả hai |
| Product pivot sang kiosk/hospital | Khác product, khác audience, khác business model | Cả hai |

---

## Lộ trình thực hiện

| Giai đoạn | Timeline | Hành động | Ưu tiên |
|-----------|----------|-----------|---------|
| **Sprint 1 (ngay)** | 2 tuần (10 ngày) | Protocol refactor + 3 tools mới + structured errors + docs | P0 |
| **Validation** | 4-6 tuần sau Sprint 1 | Bio-direct demo bằng `user_events` + `inject` + `eval`. Thu thập feedback. | P0 |
| **Sprint 2** | Tháng 9-10 2025 | Dựa trên validation: `render` tool (nếu cần) + `state_store` (nếu cần) + blog posts + marketing push | P1 |
| **H2 2025** | 6 tháng | Reach 10K installs. Community building. Refine positioning. | P1 |
| **2026** | 1 năm | Nếu MCP mainstream: shift positioning sang B. Plan B ready (standalone browser MCP layer). | P2 |
| **2027-2028** | 2-3 năm | Nếu B validated: multi-session, enterprise pilots, bio-direct seeds | P2 |

### Sprint 1 — Day-by-day Plan (đồng thuận cả hai agent)

| Day | Task | Deliverable |
|-----|------|-------------|
| 1 | Rust: `enum BrowserCommand`, Serialize/Deserialize, broadcast update | Enum compiles, unit tests pass |
| 2 | JS: `livereload.js` JSON parse + handler map. Regression test 11 tools | All existing tools pass on new protocol |
| 3 | `server.rs` + `mcp.rs` forwarding update. Gate check | Protocol refactor COMPLETE |
| 4 | `hotplate_navigate` — full implementation + test | Tool #12 working |
| 5 | `hotplate_click` (0.5 day) + integration test | Tool #13 working |
| 6 | `hotplate_user_events` — browser capture (click/input/change/submit) | Events captured in browser |
| 7 | `hotplate_user_events` — server buffer + MCP tool + test | Tool #14 working, end-to-end |
| 8 | Structured error reporting: error classification, parsed stack | `hotplate_console` upgraded |
| 9 | Error dedup + `since_last_call` option | Dedup working |
| 10 | Integration test all 14 tools + docs + README update + build | Sprint DONE ✅ |

---

## Trade-offs đã chấp nhận

1. **Click tool cực đơn giản vs Playwright-level**: Chấp nhận `el.click()` chỉ cover 90% dev-loop cases. 10% phức tạp → redirect sang Playwright MCP. *Tại sao cả hai chấp nhận*: 0.5 ngày effort, không tạo maintenance burden nếu document limitation rõ + anti-scope-creep policy.

2. **Protocol refactor trong sprint delivery vs riêng**: Chấp nhận bundle thay vì sprint riêng cho protocol. *Tại sao cả hai chấp nhận*: GPT được protocol đầu tiên (infrastructure first). Gemini được delivery gắn liền (không refactor treo).

3. **Dual positioning A/B vs single identity**: Chấp nhận messaging phức tạp hơn nhưng reach rộng hơn. *Tại sao cả hai chấp nhận*: GPT được architecture hướng B (long-term). Gemini được marketing A (short-term adoption).

4. **Defer render tool + state store**: Chấp nhận chưa build bio-direct primitives. *Tại sao cả hai chấp nhận*: Validate bằng existing tools (inject + eval) trước. 1 buổi demo chi phí gần zero. Build có evidence > build có assumption.

5. **KHÔNG duplicate Playwright**: Chấp nhận Hotplate không bao giờ là browser automation tool. *Tại sao cả hai chấp nhận*: Complement > compete. MCP ecosystem design = mỗi server làm tốt 1 việc.

---

## Appendix: Lịch sử thảo luận

| Round | GPT Review | Gemini Review | Synthesis | Đồng thuận |
|-------|-----------|---------------|-----------|------------|
| 1 | [review_gpt_round1](review_gpt_round1_2025-07-15.md) | [review_gemini_round1](review_gemini_round1_2025-07-15.md) | [synthesis_round1](synthesis_round1_2025-07-15.md) | 50% (6/12) |
| 2 | [review_gpt_round2](review_gpt_round2_2025-07-15.md) | [review_gemini_round2](review_gemini_round2_2025-07-15.md) | [synthesis_round2](synthesis_round2_2025-07-15.md) | 100% (12/12) |

---

## Một câu tóm tắt

> **Hotplate = Smart dev server với AI superpowers hôm nay, browser runtime cho AI agents ngày mai. Sprint tiếp theo: protocol refactor + 3 tools mới (navigate, click, user_events) + structured error reporting = 14 MCP tools + self-healing dev loop foundation. 2 tuần, ship thứ nhỏ nhưng differentiated.**
