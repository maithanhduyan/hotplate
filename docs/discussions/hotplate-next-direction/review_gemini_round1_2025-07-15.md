# 🔍 Review: Hotplate Next Direction — Gemini (Pragmatist)

**Reviewer**: Gemini (Pragmatist)
**Date**: 2025-07-15
**Document reviewed**: `docs/discussions/hotplate-next-direction/00_brief_2025-07-15.md`
**Roadmap reviewed**: `docs/new_roadmap.md`

---

## Stance tổng quát

Roadmap hiện tại có **vision đúng hướng** nhưng **scope quá lớn**, trộn lẫn 3 sản phẩm khác nhau vào 1 lộ trình. Với 1-2 developer, cần **cắt 80% roadmap** và focus vào thứ có thể ship trong 2-4 tuần, tạo differentiation thật sự so với Playwright MCP và Live Server.

---

## Q1: Protocol Refactor — `broadcast<String>` → `broadcast<BrowserCommand>`

### Verdict: **Cần làm, nhưng KHÔNG phải ưu tiên số 1**

**Phân tích code thực tế:**

Nhìn vào `server.rs` — logic forwarding hiện tại:

```rust
let msg = if changed_path.starts_with("inject:")
    || changed_path.starts_with("screenshot:")
    || changed_path.starts_with("dom_query:")
    || changed_path.starts_with("eval:") {
    changed_path
} else { ... };
```

Và `livereload.js` — chuỗi `if/else if` dài:

```javascript
if (d === "reload") location.reload();
else if (d.startsWith("css:")) ...
else if (d.startsWith("inject:js:")) ...
else if (d.startsWith("screenshot:")) ...
```

**String-based protocol hiện tại có vấn đề gì?**
- **Không có bug nào** vì chỉ có 6-7 message types
- **Parsing đơn giản**, không có edge case nguy hiểm
- **Nhưng**: mỗi tool mới = thêm 1 `starts_with` check ở cả Rust và JS. Không scale được nếu thêm 5-10 tool nữa

**Effort estimate:**
- Rust side: Tạo `enum BrowserCommand`, impl Serialize/Deserialize, đổi `broadcast<String>` → `broadcast<BrowserCommand>`. **~2 ngày** cho 1 dev quen codebase.
- JS side: Parse JSON thay vì `startsWith`. **~0.5 ngày**.
- **Breaking change**: Không break external API (MCP tools giữ nguyên interface). Chỉ break internal WS protocol giữa server ↔ browser agent — và cả 2 đều ship cùng binary, nên **zero external breaking change**.

**Recommendation:**
- Làm **khi cần thêm tool mới** (navigate, click). Đừng refactor chỉ để refactor.
- Nếu chỉ giữ 11 tools hiện tại, string-based **đủ tốt**.
- Nếu thêm 3+ tools → refactor trước, vì debt tích lũy nhanh.

**Timeline**: 2-3 ngày effort. Làm ngay trước khi thêm tool mới.

---

## Q2: Navigate/Click/Input tools — Build hay dùng Playwright MCP?

### Verdict: **KHÔNG build. Dùng Playwright MCP.**

**Lý do cực kỳ thực tế:**

| Tiêu chí | Hotplate build | Playwright MCP |
|---|---|---|
| **Effort** | 2-4 tuần (navigate, click, type, wait, assert) | 0 — đã có |
| **Quality** | Toy-level. Không xử lý được: iframe, shadow DOM, cross-origin, file upload, drag-drop | Production-grade, tested hàng triệu lần |
| **Selector engine** | `querySelectorAll` đơn giản | CSS, XPath, text, role, test-id, auto-waiting |
| **Maintenance** | Tự maintain mãi mãi | Microsoft team maintain |

**Cái Hotplate sẽ phải build nếu tự làm click tool:**

Từ `livereload.js` — `dom_query` hiện chỉ dùng `querySelectorAll`, trả về tag + text + attributes. Muốn build `click` tool, phải:
1. Resolve selector → element
2. Scroll into view
3. Dispatch `mousedown`, `mouseup`, `click` events đúng thứ tự
4. Handle focus, blur
5. Xử lý `<select>`, `<input>`, contenteditable
6. Wait for navigation / network idle sau click

**Đây là 6-12 tháng work** để đạt mức đáng tin cậy. Playwright đã mất 4+ năm.

**Trade-off:**
- Hotplate có latency thấp hơn Playwright (WS trực tiếp vs CDP bridge). Nhưng latency không phải bottleneck — **AI thinking time** mới là bottleneck (500ms-5s per tool call).
- AI agent có thể dùng **cả 2 MCP servers cùng lúc**: Hotplate cho live-reload + inject + screenshot + eval + console, Playwright cho navigate + click + type.

**Recommendation: KHÔNG duplicate. Complement.**

---

## Q3: User Event Bus + UI Render — Bio-direct Vision

### Verdict: **Product pivot cực kỳ rủi ro. Không nên làm ngay.**

**Phân tích thẳng:**

Roadmap đề xuất biến Hotplate thành "AI kiosk runtime" cho bệnh viện, khách sạn, hành chính. Nghe ấn tượng, nhưng:

**Vấn đề 1 — Target audience thay đổi 180°:**
- Hiện tại: web developer (dùng VS Code, biết MCP) → **hàng triệu** người
- Mới: hospital IT admin muốn deploy AI kiosk → **hàng trăm** tổ chức, sale cycle dài, compliance phức tạp (HIPAA, GDPR)

**Vấn đề 2 — Cạnh tranh khác hoàn toàn:**
- Kiosk runtime: cạnh tranh với **KioWare**, **Provisio**, **SiteKiosk** — enterprise products, hàng chục năm
- AI chatbot UI: cạnh tranh với **Voiceflow**, **Botpress**, **Dialogflow CX** — mature platforms, free tiers

**Vấn đề 3 — Engineering reality:**
Từ code hiện tại → hospital kiosk cần:
- Multi-user session management (hiện tại Hotplate **không có** concept session/user)
- Offline mode + local model fallback
- Security sandbox (CSP, iframe isolation)
- Audit logging (medical compliance)
- **Persistence** (hiện tại `AppState` chỉ in-memory)

Đây không phải feature addition. Đây là **viết lại 70% codebase** cho use case khác.

**Tuy nhiên**, ý tưởng có 1 kernel hay: AI inject UI → user interact → AI đọc event → respond. Nhưng cái này **đã hoạt động được** với tools hiện tại:

```
1. AI dùng hotplate_inject(html) → render form
2. AI dùng hotplate_eval("document.querySelector('#btn').click()") → simulate
3. AI dùng hotplate_console() → đọc log
4. AI dùng hotplate_eval("getFormData()") → lấy user input
```

**Recommendation:**
- **Không pivot**. Giữ identity là dev tool.
- Bio-direct **có thể demo** bằng tools hiện tại + eval. Viết 1 blog post demo, không cần build new tools.
- Nếu muốn explore: build **1 MCP tool** (`hotplate_user_events`) — stream click/input events qua WS. Effort: 2-3 ngày. Đây là thí nghiệm rẻ.

---

## Q4: Self-healing Dev Loop — Killer Feature?

### Verdict: **Đây là hướng đúng nhất. Nhưng Hotplate đã có 80% cần thiết.**

**Flow đề xuất:**
```
file change → reload → error → AI đọc log → AI patch → reload → verify
```

**Cái Hotplate ĐÃ CÓ:**
1. ✅ File change → reload (`watcher.rs` + `server.rs`)
2. ✅ Error capture (`livereload.js` — `window.onerror`, `console.error`)
3. ✅ AI đọc log (`hotplate_console` tool)
4. ✅ AI đọc DOM (`hotplate_dom` tool)
5. ✅ AI eval (`hotplate_eval` tool)
6. ✅ AI inject fix (`hotplate_inject` tool)

**Cái THIẾU:**
- **Không có tool nào ghi file**. AI dùng MCP khác (filesystem MCP, hoặc IDE) để patch source code. Điều này **ổn** — separation of concerns.
- **Thiếu structured error → root cause mapping**. Console logs là raw text. AI phải tự parse. Có thể cải thiện bằng cách parse error stack traces tốt hơn trong `hotplate_console` response.

**So sánh competitor:**
- **Cursor / Windsurf**: Có AI fix nhưng **không có browser context**. Chúng đọc terminal output, không đọc được runtime DOM/console.
- **Playwright MCP**: Có browser context nhưng **không có live-reload loop**. Mỗi change phải restart.
- **Hotplate**: **Duy nhất** có cả live-reload loop + browser runtime context + MCP. Đây là **real differentiator**.

**Recommendation:**
- Đây không phải "feature cần build" — nó là **use case cần document và promote**.
- Viết system prompt cho AI agent: "Dùng hotplate_console sau mỗi reload, nếu có error → đọc source → fix → inject/patch → verify".
- Cải thiện `hotplate_console` output: thêm parsed stack trace, source file mapping.
- Effort: **3-5 ngày** cho console improvement + documentation + demo.

---

## Q5: Top 3 Features Nên Build Tiếp

### Với 1-2 developer, ROI cao nhất:

**#1: `hotplate_user_events` — Stream user interactions (2-3 ngày)**

Thêm vào `livereload.js`:
```javascript
document.addEventListener("click", e => {
  send({ kind: "user_action", action: "click",
         selector: getSelector(e.target), text: e.target.textContent?.slice(0,100) });
});
```

Thêm `UserEventBuffer` tương tự `ConsoleLogBuffer` trong `server.rs`.
Thêm `hotplate_user_events` tool trong `mcp.rs`.

**Tại sao #1**: Đây là thứ **Playwright MCP KHÔNG có** (nó chỉ send actions, không listen). Biến Hotplate thành 2-way channel: AI gửi lệnh VÀ nhận user behavior. Mở ra self-healing loop + bio-direct demo mà không cần pivot.

**#2: Console/Error improvement — Structured error reporting (2-3 ngày)**

Hiện tại `ConsoleEntry` ở `server.rs` lưu raw string. Cải thiện:
- Parse stack trace → extract file + line
- Detect error type (SyntaxError, TypeError, NetworkError)
- Thêm source map resolution (nếu có)
- Trả về structured JSON thay vì text dump

**Tại sao #2**: Trực tiếp enable self-healing loop. AI parse error tốt hơn → fix chính xác hơn → productivity gain rõ ràng.

**#3: Protocol refactor + `hotplate_navigate` (3-5 ngày)**

Refactor `broadcast<String>` → `broadcast<BrowserCommand>` (xem Q1). Và thêm **1 tool đơn giản**: `hotplate_navigate` — chỉ cần `location.href = url` trong browser. Không cần full Playwright-level navigation.

**Tại sao #3**: Navigate là tool hữu ích nhất trong danh sách missing — cho phép AI chuyển trang mà không cần Playwright. Và protocol refactor lúc này có lý do thực tế (thêm tool mới).

**Tổng effort: ~2 tuần cho cả 3.**

### Loại bỏ (ROI thấp hoặc scope quá lớn):
- ❌ Click/type tools — dùng Playwright MCP
- ❌ Workflow engine — quá sớm, chưa validate use case
- ❌ Session replay — cool nhưng ít dev cần hàng ngày
- ❌ VS Code control panel — effort lớn (UI extension), impact nhỏ
- ❌ DOM snapshot graph — `hotplate_dom` hiện tại đủ cho hầu hết cases

---

## Q6: Positioning — Hotplate là gì?

### Verdict: **A+ (Dev server thông minh, MCP-native)**. Không phải B, không phải C.

**Lý do chọn A:**

| Option | TAM (Total Addressable Market) | Cạnh tranh | Effort | Adoption friction |
|---|---|---|---|---|
| A: Smart dev server | Hàng triệu web dev | Live Server (obsolete), Vite (khác segment) | Thấp — build on existing | Thấp — VS Code extension, 1-click |
| B: AI browser runtime | ~10K AI/automation engineers | Playwright MCP (dominant) | Cao — phải match Playwright quality | Trung bình |
| C: Bio-direct kiosk runtime | ~1K enterprise buyers | KioWare, Voiceflow | Cực cao — cần rewrite | Cực cao — enterprise sales |

**Positioning cụ thể:**

> **Hotplate = Live Server successor với AI superpowers.**
> Dev server duy nhất cho phép AI đọc browser state, tự debug, và inject fixes — realtime.

**Tại sao positioning này thắng:**
1. **Distribution channel sẵn**: VS Code Marketplace. Live Server có 40M+ installs. Hotplate thay thế trực tiếp.
2. **Zero-friction adoption**: Install extension → chạy. Không cần config. Không cần Playwright setup.
3. **Unique value**: Không ai khác kết hợp live-reload + MCP + browser telemetry. Đây là **blue ocean nhỏ nhưng thật**.
4. **MCP-native differentiator**: Khi dev dùng Claude/GPT với MCP, Hotplate là dev server duy nhất AI có thể "nhìn thấy" browser.

**Không chọn B vì:**
Playwright đã thắng. Team Microsoft, 5+ năm, full browser APIs. Hotplate không thể và **không nên** cạnh tranh trực diện. Complement, không compete.

**Không chọn C vì:**
Chuyển từ dev tool sang enterprise kiosk = khác product, khác team, khác business model. 1-2 dev không thể làm.

---

## Tóm tắt Stance

| Câu hỏi | Stance |
|---|---|
| Q1: Protocol refactor | ✅ Làm, nhưng chỉ khi thêm tool mới. 2-3 ngày. |
| Q2: Navigate/Click/Input | ❌ Không duplicate Playwright. Chỉ build `navigate` (đơn giản). |
| Q3: Bio-direct / UI render | ⚠️ Demo bằng tools hiện tại. Không pivot. Thêm `user_events` tool là thí nghiệm rẻ. |
| Q4: Self-healing loop | ✅ **Killer differentiator**. Đã có 80%. Document + improve console output. |
| Q5: Top 3 | 1) `user_events` tool, 2) Structured error reporting, 3) Protocol refactor + `navigate`. ~2 tuần total. |
| Q6: Positioning | **A: Smart dev server, MCP-native.** Live Server successor cho thời đại AI. |

**Nguyên tắc Pragmatist**: Ship thứ nhỏ nhưng differentiated. Đừng build Playwright. Đừng build kiosk OS. Build **thứ duy nhất bạn có**: dev server mà AI nhìn thấy browser.
