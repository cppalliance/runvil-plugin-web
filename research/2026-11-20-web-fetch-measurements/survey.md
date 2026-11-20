# Web fetch in agent harnesses: a survey

Prepared for `runvil-plugin-web` (follow-up to [runvil-harness#23](https://github.com/cppalliance/runvil-harness/issues/23)).
Sources were read on 2026-11-19. Code links are pinned to the commit read; crate statistics come from the crates.io API on the same day. "Not found" means the source was searched and the behavior is not there or not documented. Sections 1–6 come from reading code and docs; library benchmark figures there are the projects' own. Section 7 has local measurements of the Rust HTML-extraction and PDF crates on saved real pages and PDFs, including the pages from #23.

## Summary

1. **Three architectures exist.** (a) The client fetches and returns markdown to the main model: Zed, MCP fetch, Continue, Cline, Aider, Runvil today. (b) The client fetches, then a second, cheaper model call answers a prompt over the page and only that answer reaches the main model: Claude Code, and Gemini CLI's fallback. (c) The vendor fetches server-side: Codex CLI (search tool with `open_page`), Gemini CLI's default path (`urlContext`).
2. **Main-content extraction is the minority.** Only MCP fetch (Readability.js, readabilipy fallback), Continue (Readability.js) and Runvil (readabilityrs) run a readability step. Zed and Cline strip chrome by tag name; Gemini CLI, Aider and Goose's recommended `curl | html2text` convert the whole page.
3. **Readability-style extraction fails on non-article pages** (profiles, listings, search results, docs indexes), which is exactly #23. The mature extractors all add a second path: Readability.js retries with relaxed flags and keeps the longest attempt; Trafilatura cascades to readability-lxml, jusText, a baseline rescue, then recall escalation; Defuddle retries at 200 and 50 words; legible falls back to a cleaned body. Runvil's fallback fires only when the extract is under 100 characters.
4. **Rust has credible options for each piece.** HTML→markdown: `htmd` (Turndown model, 8.8M downloads in 90 days) or Zed's in-tree converter. Extraction: `dom_smoothie` (closest Readability.js port, has `is_probably_readable`), `rs-trafilatura` (best F1 on the ScrapingHub article benchmark, 0.970, plus a page-type classifier), `legible` (built for listings and indexes, young). PDF: `pdf_oxide`, `unpdf`, `pdf-extract`, or `pdfium-render` with a native binary. Office: `calamine` (spreadsheets), `docx-rs`/`docx-lite`.
5. **No harness converts PDFs in its fetch path.** Two pass the binary to a multimodal model (Gemini CLI direct mode as `inlineData`; the Claude API accepts PDF `document` blocks). Hosted readers convert: Jina Reader with PDF.js and LibreOffice, Firecrawl with a Rust fast path and ML fallbacks.
6. **Long pages:** a character window plus an offset the model can continue from (Claude Code 100K + `offset`, MCP fetch `max_length` + `start_index`, OpenHands 30K + `start_from_char`) is the pattern that works; hard truncation (Continue 20K, Cline 50K, Gemini 250K) loses the tail.
7. **SSRF defense is rare and uneven.** Pinning the connection to the checked IP on every redirect hop: Gemini CLI, MCP fetch, and Runvil's guarded resolver. Zed checks DNS but documents a rebinding window. Others have none found.
8. **Content negotiation is emerging:** Claude Code, Gemini CLI direct mode, MCP fetch and Goose's old tool send `Accept` headers that prefer `text/markdown`.
9. **Caching is rare:** only Claude Code (15 minutes) and Codex's default cached search index.
10. **Measured (section 7):** on 15 saved pages, Runvil's current logic kept 71% of the main-content phrases and reproduced #23 on the profile, lobste.rs and search pages. A ratio-based fallback raised that to 90% with fewer boilerplate leaks. `rs-trafilatura` with `favor_recall` reached 92% with the smallest output. For PDFs, all four pure-Rust crates read two-column papers in order; only `unpdf` produced Markdown tables on both table-heavy reports.
11. **Prompt-injection handling** is mostly absent in fetch code. Gemini CLI wraps output in escaped `<untrusted_context>` tags; Claude Code and Codex cite their summarizing or cached layer as the defense.

## 1. Runvil today

`crates/plugin-web` in [runvil-harness](https://github.com/cppalliance/runvil-harness/tree/master/crates/plugin-web) at `4afed45f`:

- **Client:** `reqwest` (gzip, brotli) built once with a `GuardedResolver` (connections only to addresses the policy allows), a per-hop redirect policy, no proxy, no cookie store, no `Referer`, no default credentials. Each fetch is spawned onto the Host's tokio runtime and aborted when the call is dropped (`fetch.rs`).
- **Routing by `Content-Type` before the body is read** (`response.rs`): HTML/XHTML go to extraction; JSON/XML are read all-or-nothing under the byte cap; other `text/*` is truncated at the cap; everything else (PDF, images, octet-stream, archives) is refused without downloading.
- **Decoding:** the header charset is authoritative through `encoding_rs`; a `<meta charset>` is not consulted.
- **HTML:** `readabilityrs` article extraction, converted with `htmd`; whole-page `htmd` fallback when the extract is under `MIN_CONTENT_LEN = 100` characters; `raw` skips extraction.
- **Output:** a provenance header (`url`, `truncated`, and an `extraction:` line naming the path taken: readability, whole-page fallback, raw HTML, or plain text), then text truncated to `max_chars`. No offset or continuation.
- **#23:** on profile-style pages readabilityrs returns one small container that is longer than 100 characters, so the fallback never fires.

## 2. Comparison

| Harness | Fetcher | HTML → text | Main-content extraction | Non-HTML | Size / paging | Cache | Second model call | SSRF |
|---|---|---|---|---|---|---|---|---|
| Claude Code | client (library undocumented) | markdown (library undocumented) | not documented | not documented | 100K chars + `offset` | 15 min | yes, "most" fetches | localhost and dotless hosts refused; remote hostname blocklist |
| Codex CLI | server (search tool `open_page`, `web.run`) | server | server | server | `lineno` / `find` | cached index by default | n/a | n/a (no local fetch) |
| Gemini CLI | server `urlContext`; local fallback (undici) | `html-to-text` | none | direct mode: PDF/images to model as `inlineData` | 10 MB, 250K chars | none | yes by default | DNS check, IP pinned, fails closed |
| Cursor | undocumented | undocumented | undocumented | undocumented | undocumented | undocumented | undocumented | `WebFetch(domain)` rules |
| Zed | client (own HTTP client) | own `html_to_markdown` crate | tag stripping + Wikipedia handlers | text, JSON; rest through HTML path | none | none | no | DNS gate, rebinding window |
| MCP fetch | client (httpx) | `markdownify` | Readability.js (Node) / readabilipy | raw passthrough | 5 MiB; `max_length` + `start_index` | none | no | IP pinned per hop; robots.txt |
| Goose | none built in (`curl \| html2text` skill; tool removed Aug 2026) | `html2text` | none | none | `head -c 15000` | none | no | none |
| Aider | Playwright, httpx fallback | pandoc | none | raw | none | none | no | none (user-initiated) |
| OpenHands | browser-use (Chromium) | browser-use "clean markdown" | browser-use | not found | 30K + `start_from_char` | none | via web-researcher subagent | domain allow-list |
| Cline | native fetch; puppeteer for `@url` | regex strip; Turndown for `@url` | tag stripping | JSON pretty-printed, rest raw | 5 MB, 50K chars | none | no | none found |
| Continue | client | `node-html-markdown` | Readability.js | none | 20K chars | none | no | none found |
| **Runvil** | client (`reqwest`) | `htmd` | `readabilityrs` + fallback under 100 chars | refused | byte cap + `max_chars` | none | no | guarded resolver, per-hop policy |

## 3. Harnesses in detail

### Claude Code (WebFetch)

Closed source; from the [tools reference](https://code.claude.com/docs/en/tools-reference#webfetch-tool-behavior), [permissions](https://code.claude.com/docs/en/permissions#webfetch) and [data usage](https://code.claude.com/docs/en/data-usage#webfetch-domain-safety-check) docs.

- HTTP upgraded to HTTPS. Same-host redirects followed; a cross-host redirect is returned to the model as text naming both URLs, and the model must call again. 5-minute total deadline. `User-Agent` starts with `Claude-User`. `Accept` prefers markdown.
- HTML converted to markdown; "the conversion step is not configurable"; library undocumented. PDF handling in WebFetch: not documented.
- Reads up to 100,000 characters per call; longer pages report how much went unread, with an `offset` parameter to continue.
- 15-minute response cache.
- "For most fetches, it then runs the prompt against the content in a separate model call, and Claude receives the result of that call rather than the raw page." The docs call this lossy by design and suggest `curl` for raw content.
- Refuses `localhost` and hosts without a dot. The hostname is checked against an Anthropic blocklist before each fetch. Domain rules `WebFetch(domain:*.example.com)`; a built-in set of preapproved documentation domains.

### OpenAI Codex CLI

- No client-side fetch. The workspace has no fetch tool and no HTML-conversion crate in `Cargo.lock` ([openai/codex @ f6a1b61](https://github.com/openai/codex/tree/f6a1b619125edfb3eeaf19866493889e32f97e22)).
- Hosted Responses `web_search` tool ([hosted_spec.rs](https://github.com/openai/codex/blob/f6a1b619125edfb3eeaf19866493889e32f97e22/codex-rs/core/src/tools/hosted_spec.rs#L14-L46)) whose actions include `open_page` and `find_in_page`, and a `web.run` extension ([description](https://github.com/openai/codex/blob/f6a1b619125edfb3eeaf19866493889e32f97e22/codex-rs/ext/web-search/web_run_description.md)) with `open`, `click`, `find`, `screenshot`, posted to the provider's `alpha/search` endpoint ([tool.rs](https://github.com/openai/codex/blob/f6a1b619125edfb3eeaf19866493889e32f97e22/codex-rs/ext/web-search/src/tool.rs#L130)).
- Default `web_search = "cached"` serves an OpenAI-maintained index, documented as reducing prompt-injection exposure ([config](https://developers.openai.com/codex/config-basic#web-search-mode)).

### Gemini CLI (`web_fetch`)

[web-fetch.ts @ acfb42d](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/tools/web-fetch.ts)

- Default: sends the prompt and URLs to a Gemini Flash call with `urlContext` ([L775-811](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/tools/web-fetch.ts#L775-L811)); Google fetches the pages.
- Fallback: local Node fetch, 10 s timeout, retries, 10 requests per host per minute, GitHub `blob` URLs rewritten to raw. HTML through `html-to-text`, then a second model call with the user's instructions ([L467-485](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/tools/web-fetch.ts#L467-L485)).
- Experimental direct mode skips the model: `Accept: text/markdown, …, application/pdf;q=0.7`; markdown, text and JSON returned as-is; PDF, images and video returned as base64 `inlineData` ([L599-760](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/tools/web-fetch.ts#L599-L760)).
- 10 MB download cap, 250,000-character output cap split fairly across URLs. No cache.
- Private and reserved addresses refused after DNS resolution, failing closed; a `safeLookup` connector validates and pins every connection ([fetch.ts](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/utils/fetch.ts#L501-L537)). Output wrapped in escaped `<untrusted_context>` tags ([textUtils.ts](https://github.com/google-gemini/gemini-cli/blob/acfb42d0dfc111fd1d5c5d45535daf6aa7d8696b/packages/core/src/utils/textUtils.ts#L182-L190)).

### Cursor

Docs only. A web search tool, "direct link retrieval" ([security](https://cursor.com/docs/agent/security)), `WebFetch(domainOrPattern)` permission rules in the CLI ([permissions](https://cursor.com/docs/cli/reference/permissions)), and a separate agent-controlled browser with an origin allow-list ([browser](https://cursor.com/docs/agent/tools/browser)). Fetching, conversion, limits and caching: undocumented.

### Zed

[fetch_tool.rs @ 6ae76a6](https://github.com/zed-industries/zed/blob/6ae76a6a0a06b0fc59bab3b3183d573230d2609e/crates/agent/src/tools/fetch_tool.rs)

- Plain GET with Zed's HTTP client; redirects followed one hop at a time up to 20, re-authorizing each host.
- In-tree [`html_to_markdown`](https://github.com/zed-industries/zed/tree/6ae76a6a0a06b0fc59bab3b3183d573230d2609e/crates/html_to_markdown) on html5ever with pluggable `TagHandler`s; `WebpageChromeRemover` drops `head`, `script`, `style`, `nav`; extra handlers for Wikipedia URLs. The closest Rust precedent for per-site handling.
- `text/plain` returned as-is, JSON pretty-printed in a fence, everything else (PDF included) through the HTML path. No size cap, no cache.
- IP literals and loopback refused; DNS checked against private ranges, with a code comment acknowledging a rebinding window ([L177-192](https://github.com/zed-industries/zed/blob/6ae76a6a0a06b0fc59bab3b3183d573230d2609e/crates/agent/src/tools/fetch_tool.rs#L177-L192)).

### MCP reference `fetch` server

[server.py @ c799c85](https://github.com/modelcontextprotocol/servers/blob/c799c8552b0f27b7a33a7b9233773da144dddcf5/src/fetch/src/mcp_server_fetch/server.py)

- httpx streaming, manual redirects, `Accept: text/markdown` first. robots.txt enforced for model-initiated fetches on every hop ([L761-817](https://github.com/modelcontextprotocol/servers/blob/c799c8552b0f27b7a33a7b9233773da144dddcf5/src/fetch/src/mcp_server_fetch/server.py#L761-L817)).
- Readability.js under Node when available, readabilipy otherwise, then `markdownify` ([L343-381](https://github.com/modelcontextprotocol/servers/blob/c799c8552b0f27b7a33a7b9233773da144dddcf5/src/fetch/src/mcp_server_fetch/server.py#L343-L381)).
- Non-HTML returned raw with a note. 5 MiB body cap with decompression-bomb protection. `max_length` (default 5000) and `start_index`, with a truncation hint telling the model the next index ([L1094-1106](https://github.com/modelcontextprotocol/servers/blob/c799c8552b0f27b7a33a7b9233773da144dddcf5/src/fetch/src/mcp_server_fetch/server.py#L1094-L1106)).
- Private, loopback, link-local and metadata addresses refused; every hop checked and pinned ([L156-227](https://github.com/modelcontextprotocol/servers/blob/c799c8552b0f27b7a33a7b9233773da144dddcf5/src/fetch/src/mcp_server_fetch/server.py#L156-L227)).

### Goose

- No built-in fetch since [PR #11198](https://github.com/aaif-goose/goose/pull/11198) (August 2026) removed `web_scrape` because "plain shell/curl does this in one step". A built-in skill tells the model to run `curl -sL … | uvx html2text --ignore-links` and read the first 15,000 bytes ([web_search.md](https://github.com/aaif-goose/goose/blob/bf074d6d114690de6cbe7edc925cb26efdf6f16a/crates/goose/src/skills/builtins/web_search.md#L48-L58)); docs point to the MCP fetch server.
- The removed tool saved responses to a cache directory and returned the file path rather than the content ([old mod.rs](https://github.com/aaif-goose/goose/blob/a68007960845f4ba9d4ff1b1dc05102c2fdc7875/crates/goose-mcp/src/computercontroller/mod.rs#L601-L689)).

### Aider (`/web`)

[scrape.py @ 5dc9490](https://github.com/Aider-AI/aider/blob/5dc9490bb35f9729ef2c95d00a19ccd30c26339c/aider/scrape.py)

- Playwright Chromium first (`networkidle`, 5 s), httpx fallback. BeautifulSoup "slimdown" (drops SVGs, `data:` URIs, attributes except `href`), then pandoc. Whole page, no extraction, no limits; user-initiated only.

### OpenHands

- V1 ([software-agent-sdk @ 45cf668](https://github.com/OpenHands/software-agent-sdk/tree/45cf668c5e2af38c6cc8c2eb6ae4f0d6f828e4c3)): browser tools on `browser-use`; `browser_get_content` returns "main content… in clean markdown", capped at 30,000 characters and cut at a paragraph or sentence break, with `start_from_char` ([server.py](https://github.com/OpenHands/software-agent-sdk/blob/45cf668c5e2af38c6cc8c2eb6ae4f0d6f828e4c3/openhands-tools/openhands/tools/browser_use/server.py#L257-L340)). No native lightweight fetch; MCP fetch is the documented example.
- V0 used BrowserGym and gave the model an accessibility tree plus a screenshot.

### Cline

- SDK `fetch_web_content` ([web-fetch.ts](https://github.com/cline/cline/blob/eff43e19b267e171f597ab03733bfed25090f72d/sdk/packages/core/src/extensions/tools/executors/web-fetch.ts#L102-L192)): native fetch, 30 s, 5 MB cap, regex tag stripping to text, 50,000-character truncation. A per-URL `prompt` parameter is appended to the output; no model call.
- VS Code `@url`: puppeteer, Cheerio removes `script`, `style`, `nav`, `footer`, `header`, then Turndown ([UrlContentFetcher.ts](https://github.com/cline/cline/blob/eff43e19b267e171f597ab03733bfed25090f72d/apps/vscode/src/services/browser/UrlContentFetcher.ts#L1-L61)).

### Continue

- `fetch_url_content` requires permission; `@mozilla/readability` on JSDOM, then `node-html-markdown` ([URLContextProvider.ts](https://github.com/continuedev/continue/blob/5522c6f44ca0ac3528b37244818fbfa39b5af470/core/context/providers/URLContextProvider.ts#L1-L56)); hard cut at 20,000 characters with a truncation warning.

## 4. Extraction engines

### Algorithms

- **Readability.js** (Apache-2.0): scores candidate nodes. If the winner has fewer than `charThreshold` (500) characters it re-parses with `FLAG_STRIP_UNLIKELYS`, then `FLAG_WEIGHT_CLASSES`, then `FLAG_CLEAN_CONDITIONALLY` turned off, and returns the longest attempt if all stay short. `isProbablyReaderable()` is a separate pre-check (`minContentLength = 140`, `minScore = 20`).
- **Trafilatura** (Python, Apache-2.0; the Python pipelines' choice): its own rule-based extractor, then readability-lxml and jusText, then a baseline rescue over the original tree, then recall escalation with relaxed thresholds; triggered below `MIN_EXTRACTED_SIZE = 250`. `favor_precision` / `favor_recall` shift the balance. Its own evaluation (990 documents): F-score 0.926 standard, readability-lxml 0.853, jusText 0.862, newspaper4k 0.803 (`docs/evaluation.rst`).
- **jusText** (BSD-2), **Boilerpipe** (Java, inactive since 2018), **newspaper4k** (MIT), **go-trafilatura** (Apache-2.0).

### Hosted and LLM-oriented readers

| Tool | How it extracts | License |
|---|---|---|
| Jina Reader (`r.jina.ai`) | headless Chrome or `curl-impersonate` chosen per request; Readability by default, skippable with `x-respond-with: markdown`; Turndown for markdown | Apache-2.0 |
| ReaderLM-v2 | 1.5B model, HTML to markdown/JSON, 512K context | CC-BY-NC-4.0 (non-commercial) |
| Firecrawl | `onlyMainContent` removes elements by selector (`header`, `footer`, `nav`, `aside`, `.sidebar`, `.ad`…), not Readability; Rust `transformHtml` with a cheerio fallback | AGPL-3.0 |
| Crawl4AI | `raw_markdown` = whole page; `fit_markdown` prunes by text density, link density and tag weight, or BM25 against a query | Apache-2.0 |
| Defuddle (Obsidian Web Clipper) | "more forgiving" than Readability; retries without partial selectors below 200 words, then without hidden-element and low-score removal below 50 words ("possible index page"); reads schema.org | MIT |

### Benchmark

[ScrapingHub article-extraction-benchmark](https://github.com/scrapinghub/article-extraction-benchmark) article-body F1, as published: rs_trafilatura 0.970, go_trafilatura 0.960, trafilatura 2.0.0 0.958, readability_js 0.947, readability-lxml 0.922, readability_rs (kumabook) 0.873, dom_smoothie 0.14 0.865, readabilityrs 0.1.2 0.832 (precision 0.745), llm_readability 0.829, jusText 0.804, boilerpipe_rs 0.739. The set is news and blog articles only, so it says nothing about the page types in #23.

### Why non-article pages fail

Readability scoring rewards long, comma-heavy paragraphs and penalizes link density and "unlikely" class names. Profiles, listings and search results are many short, link-heavy cards, so either one card wins or the cards are pruned. Mitigations seen in the wild:

1. a pre-gate (`isProbablyReaderable`; dom_smoothie and Trafilatura ship one; not found in readabilityrs);
2. a length-triggered cascade with relaxed rules (Readability.js flags, Trafilatura recall escalation, Defuddle retries);
3. page-type classification (rs-trafilatura: XGBoost over article, forum, product, collection, listing, documentation, service, plus a confidence score with "pages below 0.80 … candidates for LLM fallback");
4. a conservative cleaned-body fallback (legible; Runvil's existing fallback, at a much lower threshold);
5. skipping extraction on request (Jina `x-respond-with: markdown`, Crawl4AI `raw_markdown`, Runvil `raw`).

### Rust crates

| Crate | Approach | License | Last release | Downloads (total / 90 d) | Notes |
|---|---|---|---|---|---|
| readabilityrs | Readability.js port | Apache-2.0 | 0.1.4, 2026-08-10 | 491K / 108K | passes 119/130 Mozilla tests; `char_threshold`; no readerable check found |
| dom_smoothie | follows Readability.js closely | MIT | 0.19.0, 2026-10-27 | 1.03M / 682K | `is_probably_readable`, alternative candidate mode, markdown text mode; active |
| readability (kumabook) | arc90 port | MIT | 0.3.0, 2023-12 | 1.68M / 340K | stale |
| legible | Readability-inspired, semantic candidates + cleaned-body fallback | Apache-2.0 | 0.5.1, 2026-08-08 | 49K / 37K | targets docs, indexes, listings, short pages; 4 stars; not benchmarked |
| rs-trafilatura | trafilatura port + page-type classifier | MIT OR Apache-2.0 | 0.2.2, 2026-04-03 | 17K | top benchmark F1; young |
| llm_readability (spider) | kumabook rewrite | MIT | 0.0.17, 2026-04-30 | 100K / 15K | |
| htmd | HTML → markdown, Turndown model | Apache-2.0 | 0.6.0, 2026-11-18 | 11.8M / 8.8M | passes Turndown's tests; tables |
| fast_html2md (spider) | HTML → markdown on lol_html, streaming | MIT | 0.0.66, 2026-11-16 | 435K / 129K | |
| html2md | HTML → markdown | GPL-3.0+ | 0.2.17 | 1.2M | copyleft; avoid |
| html2text | HTML → text | MIT | 0.17.4 | 7.4M | text, not markdown |
| scraper / lol_html | DOM + selectors / streaming rewriter | ISC / BSD-3 | current | 37M / 7.6M | building blocks |

### JavaScript-rendered pages

`chromiumoxide` (async CDP, tokio, slow build), `headless_chrome` (sync, can download Chromium), `fantoccini` (WebDriver, separate driver process), spider's `chrome` feature (chromiumoxide fork, `CHROME_URL` for a remote browser). None bundles a browser; containers force a choice between configuring the sandbox and `--no-sandbox`. Jina's design (plain HTTP first, browser only when needed) keeps the common path cheap. Among the harnesses, only Aider uses a browser for fetch by default; OpenHands and Cline treat the browser as a separate interactive tool.

## 5. Non-HTML formats

### What harnesses do

| Harness | Non-HTML behavior |
|---|---|
| Claude Code WebFetch | not documented |
| Claude API | PDF `document` blocks: each page as image plus extracted text; 128 MB / 600 pages; DOCX/XLSX not supported in document blocks ([PDF support](https://platform.claude.com/docs/en/build-with-claude/pdf-support)) |
| Gemini CLI direct mode | PDF, images, video sent to the model as base64 `inlineData` |
| MCP fetch | raw passthrough with a note |
| Zed | through the HTML path |
| Jina Reader | PDF.js; Office via LibreOffice; images captioned by a vision model |
| Firecrawl | Rust fast path for text-based PDFs; MinerU / other engines when scanned, low confidence, or complex layout; `maxPages` |

### General converters (non-Rust)

- **MarkItDown** (MIT): pdfminer.six/pdfplumber, mammoth (DOCX), python-pptx, pandas/openpyxl, magika for type detection; OCR via LLM-vision plugin or Azure.
- **Docling** (MIT code; models under their own licenses): layout, reading order, TableFormer tables; Python + torch.
- **marker / surya**: code Apache-2.0, weights under a modified OpenRAIL-M (free below $5M funding/revenue).
- **MinerU**: Apache-2.0 plus extra terms (commercial license above thresholds, attribution for online services).
- **pandoc** (GPL-2.0): DOCX/PPTX/XLSX in, no PDF.
- **Unstructured** (Apache-2.0): needs libmagic, poppler, tesseract, LibreOffice.

### Rust crates

| Crate | Formats | Approach | License | Last release | Downloads (total / 90 d) | Notes |
|---|---|---|---|---|---|---|
| pdf_oxide | PDF | pure Rust, `to_markdown`, tables, headings | MIT OR Apache-2.0 | 0.3.78, 2026-09-08 | 1.7M / 1.0M | self-reported 100% pass on 3,830 PDFs; no OCR |
| unpdf | PDF | pure Rust → markdown/text/JSON, XY-Cut columns, tables | MIT | 0.55.0, 2026-11-15 | 27.5K / 14K | young |
| pdf-extract | PDF | pure Rust on lopdf | MIT | 0.12.1, 2026-09-16 | 8.5M / 4.4M | plain text only |
| pdfium-render | PDF | Pdfium bindings | MIT/Apache-2.0 | 0.9.5, 2026-11-01 | 4.1M / 2.1M | needs a libpdfium binary; can render pages to images |
| oxidize-pdf | PDF | pure Rust, markdown export | MIT | 5.6.1, 2026-11-16 | 206K / 141K | optional Tesseract |
| mupdf | PDF, XPS, EPUB | MuPDF bindings | AGPL-3.0 | 0.8.0 | 1.8M | AGPL; avoid |
| xberg (kreuzberg successor) | ~107 formats | Rust core, optional Pdfium, OCR, layout/table models | MIT | 1.4.2, 2026-11-19 | 73.5K | heavy, feature-gated |
| extractous | Tika formats | Tika via GraalVM | Apache-2.0 | 0.3.0, 2024-12 | 645K | stale |
| calamine | XLS, XLSX, XLSB, ODS | pure Rust | MIT | 0.38.0, 2026-11-02 | 19.0M / 7.6M | maps to markdown tables |
| docx-rs | DOCX | read/write model | MIT | 0.4.22, 2026-07-21 | 4.0M / 786K | rendering to markdown is on the caller |
| docx-lite | DOCX | text, tables, lists | MIT/Apache-2.0 | 0.2.0, 2025-09-27 | 125K / 51K | |
| infer / mime_guess | — | magic bytes / extension | MIT | current | 150M / 355M | type sniffing |

No maintained PPTX-specific crate was found; the options are xberg or reading the zip's slide XML (`a:t` runs).

### Practical concerns

- **Type detection:** servers send `application/octet-stream` for documents; check magic bytes (`%PDF-`, `PK\x03\x04` with `[Content_Types].xml`) as well as the header, and advertise accepted types in `Accept`.
- **Scanned PDFs:** text extraction returns near nothing; report "no text layer (likely scanned)" instead of an empty result, or hand the bytes to a multimodal model.
- **Large documents:** page ranges plus character offsets.
- **Untrusted binaries:** Pdfium and Tika run native code in-process; Office formats are zip files (zip bombs). Cap download and decompressed size, parse with a timeout, consider a subprocess.

## 6. What the plugin contract means for `runvil-plugin-web`

From [runvil-plugin-api](https://github.com/vinniefalco/runvil-plugin-api) (`api-v0.5.0`):

- **A Plugin is a `cdylib` behind a C ABI.** It cannot share the Host's tokio runtime the way `plugin-web` does today (it spawns onto "the Host's runtime handle"). It will need its own runtime (or a worker thread pool) created in `load`, with each tool call's future relaying results from it.
- **The Host polls a tool call once per step without a waker.** A fetch that waits on the network must `await next_step()` between polls so no step blocks and the Host can cancel between steps (`RV_TOOL_CANCEL`). Download, decompression and PDF parsing all need to be chunked this way or run off-thread.
- **The contract has file operations but no network service.** Network policy (SSRF guard, redirects, size caps) therefore stays inside the Plugin. Open question: should the contract gain a Host-mediated HTTP request, so the run's policy applies to network access the way it already applies to files?
- **Files go through the Host.** A fetch could write a large or binary response to the run's files and return a short summary plus the path, letting the model use `read_range` for the rest. Goose's removed tool did this; it also gives PDFs a natural home before conversion.
- **Output trust.** Page text should stay `ToolOutput` untrusted, as today.
- **A separate repository** suits the dependency weight: PDF and Office support (and a browser, if ever) can sit behind Cargo features without touching the Harness.

## 7. Measurements

Run locally on 2026-11-19 (Rust 1.99, release build, 16 cores). Code, criteria and raw results: `runvil-fetch-bench/` (HTML and PDF harnesses, one `main.rs` each).

### HTML extraction

**Fixtures:** 15 pages saved once and replayed offline:
- the `PROFILE` HTML from #23
- articles: the Rust 1.85 release post, an NPR story
- docs: the Rust Book ownership chapter, the reqwest docs.rs page
- Wikipedia: "Readability"
- profiles: a GitHub user page, a cppalliance.org staff page
- listings: Hacker News, lobste.rs (listed in #23), the Rust users forum "latest" (Discourse)
- a DuckDuckGo HTML results page
- an Adafruit product page
- a GitHub repository page
- the crates.io crate page, a JavaScript single-page app

Three other sites refused automated fetches (HTTP 403 or a bot challenge) and were dropped.

**Scoring:** each page lists 1–4 phrases from its main content (for example the person's name and location, story titles, section headings) and 1–4 boilerplate phrases (cookie and nav text, inline CSS/JSON, related-story links, footers). *Recall* is the share of content phrases present. *Leaks* counts boilerplate phrases present. Time is the median of 3 runs per page.

**Methods:**
- `runvil_current`: Runvil's shipped logic, copied from `response.rs`.
- `runvil_ratio_fallback`: the same, but falls back to a cleaned whole page when the extract is under 500 characters or under 25% of the cleaned page.
- `htmd_raw_page`: `htmd` on the whole document, which is Runvil's `raw`.
- `htmd_cleaned_page`: `htmd` with `script`, `style`, `noscript`, `svg`, `nav`, `header`, `footer`, `aside`, `form`, `iframe`, `template`, `button` and `dialog` skipped.
- `dom_smoothie`, `legible`, `rs_trafilatura` (default and `favor_recall`), `llm_readability` (followed by `htmd`): each library alone.
- `dom_smoothie_gated`: dom_smoothie when `is_probably_readable`, with the same ratio fallback, else the cleaned page.

| Method | Content recall | Pages with every phrase | Boilerplate leaks | Median ms / page | Total output (chars) |
|---|---|---|---|---|---|
| runvil_current | 35/49 (71%) | 7/15 | 6/37 | 19.3 | 201K |
| **runvil_ratio_fallback** | **44/49 (90%)** | **10/15** | **4/37** | 20.2 | 230K |
| htmd_raw_page | 48/49 (98%) | 14/15 | 36/37 | 1.8 | 702K |
| htmd_cleaned_page | 45/49 (92%) | 12/15 | 16/37 | 1.4 | 255K |
| dom_smoothie | 36/49 (73%) | 7/15 | 2/37 | 2.4 | 184K |
| dom_smoothie_gated | 42/49 (86%) | 10/15 | 5/37 | 3.7 | 200K |
| legible | 34/49 (69%) | 7/15 | 2/37 | 2.3 | 156K |
| rs_trafilatura | 43/49 (88%) | 10/15 | 5/37 | 14.9 | 142K |
| **rs_trafilatura_recall** | **45/49 (92%)** | **11/15** | 5/37 | 14.8 | **141K** |
| llm_readability | 22/49 (45%) | 3/15 | 5/37 | 3.2 | 116K |

**What the numbers show:**

- **#23 reproduces.** On the issue's `PROFILE` page, `runvil_current` returns only the "Pinned" section (145 characters, with the name and location gone), as the issue describes. On lobste.rs it returns 133 characters with none of the stories, and on the DuckDuckGo page none of the results. Each of these extracts clears the 100-character threshold, so the fallback never fires.
- **The ratio fallback fixes all three** (3/3, 4/4 and 3/3 phrases). Recall goes from 71% to 90% and leaks drop from 6 to 4, at no measurable cost in time. It's a small change to the existing code.
- **`rs_trafilatura` with `favor_recall` gives the best balance:** the highest recall among extractors (92%) and the smallest output (141K characters across the set, against 230K for the ratio fallback). Its misses and leaks fall on different pages: it keeps NPR's related-story links (3 leaks there), while the readability-based methods drop them.
- **Raw `htmd` finds nearly everything but leaks nearly everything** (36/37), including inline CSS and JSON. That matches the issue's note about `raw`. Skipping `script`/`style` and the other noise tags removes over half the leaks (16/37) and keeps 92% recall, so `raw` could skip those tags at least.
- **Skipping `noscript` can lose a whole page.** Discourse forums server-render their content inside `<noscript>`. The cleaned page kept 35 characters of the forum listing, and dom_smoothie and legible returned nothing. readabilityrs and rs-trafilatura kept the topics.
- **`legible` returned empty output** on the Rust Book chapter and the forum page. **`llm_readability`** failed on most listings. Neither is ready to be the default.
- **Speed:** readabilityrs costs about 19 ms per page and rs-trafilatura about 15 ms; dom_smoothie, legible and htmd take 1–4 ms. All are small next to network time.
- **No method recovers the crates.io single-page app**: its content isn't in the HTML at all. That needs either a JavaScript renderer or the site's API.
- **Caveats:** 15 pages with hand-picked phrases is a smoke test, not a benchmark. The phrases come from my reading of each page, and the pages are as fetched on one day.

### PDF conversion

**Fixtures:** "Attention Is All You Need" (arXiv, single column), the BERT paper (ACL Anthology, two columns), and two table-heavy Federal Reserve statistical releases (H.8, H.4.1). The BLS employment report refused automated fetches. `pdfium-render` was not run, because it needs a separately installed libpdfium.

**Scoring:**
- *Phrases:* 3–6 per file, matched after collapsing whitespace.
- *Reading order:* checked with sentences from the middle of the two-column paper's columns.
- *Tables:* counted as Markdown table rows.

| Crate | Parsed all 4 | Phrases | Two-column reading order | Rejoins hyphenated line breaks | Markdown tables (H.8 / H.4.1 rows) | Time per file (ms) |
|---|---|---|---|---|---|---|
| pdf_oxide 0.3.78 | yes | 17/18 | correct | no ("representa-tion") | 0 / 139 | 67–181 |
| unpdf 0.55.0 | yes | 16/18 | correct | yes | **651 / 125** | 15–82 |
| pdf-extract 0.12.1 | yes | 17/18 | correct | no ("representa- tion") | 0 / 0 | 21–178 |
| oxidize-pdf 5.6.1 | yes | **18/18** | correct | yes | 0 / 0 | 13–123 |

- **unpdf's misses are formatting:** it bolds the initials of "**B**idirectional **E**ncoder…", which breaks literal phrase search but not reading. Without tables, oxidize-pdf rows come out as label-then-numbers lines, which stay readable. pdf-extract separates H.8 row labels from their numbers. pdf_oxide puts numbers before the label on H.8 (`8.3 6.7 -0.2 4.0 2 Securities in bank credit`).
- **For tables, unpdf was the only crate that produced Markdown tables on both releases.** For plain prose, all four are usable.
- **Not measured:** scanned PDFs (OCR), encrypted or damaged files, very large documents, and DOCX/XLSX/PPTX.

## 8. Suggested direction

### Stage 1: same scope, fix #23, adopt the contract

1. Move to the plugin contract as above (own runtime, step-chunked fetch, cancellation).
2. Keep the guarded resolver and per-hop policy; they already match the strongest designs found (Gemini CLI, MCP fetch).
3. Replace the 100-character fallback rule with a ratio test: fall back to a cleaned whole page when the extract is under 500 characters or under 25% of the cleaned page. In section 7 this alone took recall from 71% to 90% and fixed the #23 profile, lobste.rs and the search page. Keep reporting the path in the `extraction:` line. Don't skip `<noscript>` in the cleaned page (Discourse puts its content there); skip `script`, `style`, `nav`, `header`, `footer`, `aside`, `form`, `svg` and similar, with Firecrawl's exclusion list as a further template.
4. Evaluate `rs-trafilatura` with `favor_recall` as the extractor. It had the best recall and the smallest output in section 7, but it's a young crate (0.2.2, about 17K downloads), so grow the fixture set (more profiles, listings, docs and news pages, with expected phrases) before switching. Make the same `script`/`style` skipping the default for `raw`.
5. Add `Accept: text/markdown` preference and pass markdown responses through unchanged.
6. Add an `offset` (or `start_index`) parameter and a continuation hint in truncated output.
7. Honor `<meta charset>` when the header has none.

### Stage 2: documents

1. Accept PDF and Office types; sniff magic bytes.
2. PDF behind a feature, with a `pages` parameter and a "no text layer" message for scanned files. In section 7, `unpdf` was the only crate that turned both statistical releases into Markdown tables, and it rejoins hyphenated line breaks. `oxidize-pdf` matched every phrase and was fastest, but its tables are plain lines. Either is a reasonable first choice; test scanned and damaged files before deciding.
3. XLSX/ODS via `calamine`, one sheet per call as markdown tables; DOCX via `docx-rs` or `docx-lite`.
4. Download cap around 10 MB, decompressed-size cap, parse timeout.

### Stage 3: optional and heavy

1. Hand PDFs and images to multimodal models through the gateway (Claude `document` blocks, Gemini `inlineData`) when the run's model supports them.
2. OCR and layout through `xberg` features or `pdfium-render`.
3. JavaScript rendering through a remote browser (`chromiumoxide`/`CHROME_URL` style), opt-in, after plain HTTP fails.
