# Measurements for the open questions in "Runvil: How WebFetch Should Work"

Measured on 2026-11-20 against the design in [2026-11-19-web-fetch-field-survey.md](2026-11-19-web-fetch-field-survey.md). The Rust libraries were run directly on 15 saved pages and 4 PDFs. The earlier survey of how other harnesses fetch is [2026-11-20-web-fetch-harness-survey.md](2026-11-20-web-fetch-harness-survey.md).

## Method

- Fixtures: 15 saved pages. They are the #23 profile, lobste.rs, Hacker News, a Discourse forum's latest list, a DuckDuckGo results page, the Rust blog, NPR, Wikipedia, the Rust book, docs.rs, two profile pages, an Adafruit product page, the runvil-harness GitHub page, and crates.io (a JavaScript-only page).
- Scoring: each page lists phrases from its main content (49 in all) and boilerplate phrases (37 in all). An output scores a hit for each content phrase it contains and a leak for each boilerplate phrase.
- `runvil_current` is `extract_html` from plugin-web at `4afed45f`: readabilityrs, falling back to `htmd` on the raw page under 100 characters.
- `survey_design` is steps 2 to 6 of the survey's Extraction decision. It runs the cleaning pass (with `lol_html`), then readabilityrs 0.1.4 on the cleaned HTML, then the 250 / 3,000 / 20% fallback to `htmd` on the cleaned HTML.

## Results

| Method | Content phrases kept | Pages with every phrase | Boilerplate leaked | Output |
|---|---|---|---|---|
| `runvil_current`, readabilityrs 0.1.3 + htmd 0.5.5 (today's pins) | 35/49 (71%) | 7/15 | 6/37 | 211K chars |
| `runvil_current`, readabilityrs 0.1.4 + htmd 0.6.0 | 35/49 (71%) | 7/15 | 6/37 | 200K |
| `survey_design` | 43/49 (88%) | 10/15 | 9/37 | 233K |
| `survey_design`, keeping `<noscript>` | 45/49 (92%) | 11/15 | 9/37 | 250K |
| Ratio fallback (extract < 500 chars or < 25% of the page with nav, header and footer stripped) | 44/49 (90%) | 10/15 | 4/37 | 230K |
| `rs-trafilatura` 0.2.2, `favor_recall` | 45/49 (92%) | 11/15 | 5/37 | 141K |

## Answers to the open questions

1. **Do the 250 / 3,000 / 20% thresholds suit readabilityrs?** Mostly, yes. They fix all three #23 cases:

   | Page | Today | Survey design |
   |---|---|---|
   | Profile | 145 chars, 1/3 phrases | 3/3 |
   | lobste.rs | 133 chars, 0/4 | 4/4 |
   | DuckDuckGo | 0/3 | 3/3 |

   Content kept goes from 71% to 88%.
2. **`<noscript>` removal loses a whole page.** Discourse ships its server-rendered topic list inside `<noscript>`. With the cleaning pass, `users.rust-lang.org/latest` comes out as 0 characters, which step 7 turns into the "no readable text" failure. Keeping `<noscript>` restores it (88% → 92%) and adds no leak on the other 14 pages.
3. **G3, relative links: confirmed.** Today's output has 144 relative links on 9 of 15 pages. After the cleaning pass, it has none.
4. **G4, script text: confirmed.** On crates.io the whole-page fallback outputs the `window.onerror=...` handler and the `__sveltekit_...` bootstrap. Neither appears after the cleaning pass.
5. **readabilityrs 0.1.3 → 0.1.4 and htmd 0.5.5 → 0.6.0.** Scores are identical on all 15 pages. Only the Discourse page differs in size (27,702 → 17,429 chars), with the same phrases kept.
6. **Keeping nav, header and footer costs boilerplate.** Leaks go from 6 to 9. The new leaks are GitHub's "Navigation Menu", DuckDuckGo's region list and "Sign in" on the profile. This is the expected price of keeping links.
7. **Conversion time on an 8 MiB page.** The test page is a synthetic 8.2 MiB file: the Wikipedia article's body repeated 24 times. Each figure is a single run:

   | Step | Time |
   |---|---|
   | readabilityrs 0.1.3 | 136.6 s |
   | readabilityrs 0.1.4 | 117.5 s |
   | `survey_design`, whole pipeline | 115.3 s |
   | htmd | 0.19 s |
   | dom_smoothie | 0.26 s |
   | rs-trafilatura | 1.25 s |

   On the 343 KiB original, readabilityrs takes 0.38 s and htmd 7 ms. A page 24 times larger is about 310 times slower in readabilityrs, so its time grows roughly with the square of the page size. htmd grows linearly. At the 8 MiB body cap, one fetch would hold a thread for about 2 minutes. So conversion needs a blocking thread and a size limit on what goes into readabilityrs, for example skipping to the whole-page path above a few hundred KiB. Otherwise it needs a deadline.

   The cause is three regexes in readabilityrs that look for a closing tag on the void elements `<link>`, `<input>` and `<embed>`, and scan to the end of the page once per tag. The fix is upstream in [theiskaa/readabilityrs#27](https://github.com/theiskaa/readabilityrs/pull/27). With it, readabilityrs takes 6.6 s on the 8 MiB page instead of 117 s, and its time grows linearly.
8. **Rust PDF libraries (the survey studied none).** pdf_oxide, unpdf, pdf-extract and oxidize-pdf all read a two-column paper in the correct order. oxidize-pdf matched all 18 test phrases and the others 16 or 17. Only unpdf produced Markdown tables for both of the Federal Reserve statistical releases.

## Limits

- 15 pages is a quick check, not a benchmark.
- The 8 MiB page is synthetic, and each figure is a single run.
- The cleaning pass was written with `lol_html` rather than the survey's html5ever + rcdom. The hidden-element and link rules are the same.
