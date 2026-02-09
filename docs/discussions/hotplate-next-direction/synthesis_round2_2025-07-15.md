# 🎼 Synthesis — Round 2 | 2025-07-15

## Chủ đề: Hotplate — Hướng phát triển tiếp theo

---

## 📊 Bảng đồng thuận

| # | Điểm thảo luận | GPT (Visionary) | Gemini (Pragmatist) | Đồng thuận? |
|---|----------------|-----------------|---------------------|-------------|
| 1 | Protocol Refactor — Cần làm | ✅ Cần | ✅ Cần | ✅ |
| 2 | Protocol Refactor — Timing | Đầu sprint, Day 1-3 | Đầu sprint, max 2 ngày | ✅ |
| 3 | `hotplate_navigate` tool | ✅ Build | ✅ Build | ✅ |
| 4 | `hotplate_click` tool | ✅ Click lite (5 dòng JS) | ✅ Click lite + document limitation | ✅ |
| 5 | `hotplate_user_events` tool | ✅ Build — unique differentiator | ✅ Build — thí nghiệm rẻ | ✅ |
| 6 | `hotplate_render` tool | ✅ Defer — evidence-based | ✅ Defer — evidence-based | ✅ |
| 7 | State store (`state_get/set`) | ✅ Defer — trigger-based, 4-week deadline | ✅ Defer — review sau 6 tuần | ✅ |
| 8 | Bio-direct — không pivot | 🌱 SEED, không pivot | ❌ Không pivot | ✅ |
| 9 | Self-healing loop là killer feature | ✅ "Lý do Hotplate tồn tại" | ✅ "Killer differentiator" | ✅ |
| 10 | Structured error reporting | ✅ Cần upgrade | ✅ Cần upgrade | ✅ |
| 11 | Sprint plan — thứ tự + nội dung | protocol → tools → self-healing | protocol → tools → self-healing | ✅ |
| 12 | Positioning — Dual strategy | ✅ "Design B, Market A", hint B trong README | ✅ "Design B, Market A", A primary 6 tháng | ✅* |

*\*README wording: GPT muốn thêm tagline "Think: Live Server successor × AI-native browser runtime". Gemini muốn A only for 6 months. Tuy nhiên GPT's headline vẫn là A ("Smart dev server with AI superpowers") — chỉ thêm 1 dòng hint. Gemini's condition nói "Không đổi README thành 'AI browser runtime'" — GPT không đổi, chỉ hint nhỏ. **Hai bên compatible.***

---

## ✅ Các điểm đã đồng thuận: 12/12 = 100%

### Tổng hợp quyết định cuối cùng:

1. **Protocol Refactor**: `broadcast<BrowserCommand>` enum + structured JSON WS. Đầu sprint, 2-3 ngày. Gate: 11 tools hiện tại pass trước khi thêm tools mới.

2. **3 MCP tools mới**: `hotplate_navigate` (đơn giản, `location.href`), `hotplate_click` (đơn giản, `el.click()`, document limitation rõ ràng), `hotplate_user_events` (passive listener cho click/input/submit — unique differentiator).

3. **Deferred tools**: `hotplate_render` (defer, evidence-based — trigger: DOM pollution / race condition / >3 tool calls per render). `state_get/set` (defer, trigger-based — build nếu `window.__state` hack xuất hiện hoặc community request. Review sau 4-6 tuần).

4. **Self-healing dev loop**: Upgrade `ConsoleEntry` — error type classification, parsed stack trace, error deduplication. Documentation + system prompt template. Blog post draft.

5. **Positioning**: **"Design for B, Market as A"**. Marketplace: "Smart dev server". Blog/MCP: "Browser runtime for AI agents". README: A headline + subtle B hint. Mốc chuyển đổi: 10K installs + community traction + MCP mainstream.

6. **Sprint plan**: 10 ngày, protocol → navigate + click → user_events → error improvement → docs.

### Điều kiện chung (cả hai đồng ý):
- Effort caps cứng: Protocol = 2-3 ngày max, Click = 0.5 ngày max
- Defer có deadline review: 4-6 tuần
- Marketing A primary cho 6 tháng đầu
- Sprint plan là commitment — không thêm feature giữa sprint
- Protocol refactor hoàn thành + test TRƯỚC khi code tools mới

---

## 📈 Tỷ lệ đồng thuận: 12/12 = 100% ✅

---

## ➡️ Chuyển sang Phase cuối: Tổng kết

Đạt 100% đồng thuận sau 2 rounds. Tiến hành tạo Final Consensus document.
