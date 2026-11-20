//! Runs several HTML extraction strategies over saved fixture pages.
//!
//! Usage: extract-bench <fixtures_dir> <out_dir> [criteria.json]
//! Writes <out_dir>/<method>/<page>.md and prints a results table.
//! With criteria.json ({page: {"url":..,"include":[..],"exclude":[..]}}), scores each output:
//! include recall (main-content phrases present) and boilerplate leaks (exclude phrases present).

use std::{collections::BTreeMap, fs, path::Path, time::Instant};

const NOISE_TAGS: &[&str] = &[
    "script", "style", "noscript", "svg", "nav", "header", "footer", "aside", "form", "iframe",
    "template", "button", "dialog",
];

fn htmd_raw(html: &str) -> String {
    htmd::convert(html).unwrap_or_default()
}

fn htmd_cleaned(html: &str) -> String {
    let conv = htmd::HtmlToMarkdown::builder()
        .skip_tags(NOISE_TAGS.to_vec())
        .build();
    conv.convert(html).unwrap_or_default()
}

/// readabilityrs markdown, as Runvil calls it.
fn readabilityrs_md(html: &str, url: &str) -> String {
    use readabilityrs::{Readability, ReadabilityOptions};
    let options = ReadabilityOptions { output_markdown: true, ..ReadabilityOptions::default() };
    Readability::new(html, Some(url), Some(options))
        .ok()
        .and_then(Readability::parse)
        .and_then(|a| a.markdown_content.or_else(|| a.content.and_then(|c| htmd::convert(&c).ok())))
        .unwrap_or_default()
}

/// Runvil's shipped logic (plugin-web response.rs @ 4afed45f).
fn runvil_current(html: &str, url: &str) -> String {
    let md = readabilityrs_md(html, url);
    if md.trim().len() >= 100 { md } else { htmd_raw(html) }
}

/// Proposed: readabilityrs, but fall back to the cleaned whole page when the extract is
/// small in absolute terms or relative to the page's visible text.
fn runvil_ratio(html: &str, url: &str) -> String {
    let md = readabilityrs_md(html, url);
    let body = htmd_cleaned(html);
    let (a, b) = (md.trim().chars().count(), body.trim().chars().count());
    if a >= 500 && (b == 0 || a * 4 >= b) { md } else { body }
}

fn dom_smoothie_md(html: &str, url: &str) -> String {
    let cfg = dom_smoothie::Config { text_mode: dom_smoothie::TextMode::Markdown, ..Default::default() };
    dom_smoothie::Readability::new(html, Some(url), Some(cfg))
        .ok()
        .and_then(|mut r| r.parse().ok())
        .map(|a| a.text_content.to_string())
        .unwrap_or_default()
}

/// Proposed alt: dom_smoothie gated by its readerable check, else cleaned whole page.
fn dom_smoothie_gated(html: &str, url: &str) -> String {
    let readable = dom_smoothie::Readability::new(html, Some(url), None)
        .map(|r| r.is_probably_readable())
        .unwrap_or(false);
    if readable {
        let md = dom_smoothie_md(html, url);
        let body = htmd_cleaned(html);
        let (a, b) = (md.trim().chars().count(), body.trim().chars().count());
        if a >= 500 && (b == 0 || a * 4 >= b) { return md; }
        return body;
    }
    htmd_cleaned(html)
}

fn legible_md(html: &str, url: &str) -> String {
    legible::parse(html, Some(url), None).map(|a| a.markdown_content).unwrap_or_default()
}

fn trafilatura_md(html: &str, url: &str, recall: bool) -> String {
    let opts = rs_trafilatura::Options {
        output_markdown: true,
        favor_recall: recall,
        url: Some(url.to_string()),
        ..Default::default()
    };
    rs_trafilatura::extract_with_options(html, &opts)
        .map(|r| r.content_markdown.unwrap_or(r.content_text))
        .unwrap_or_default()
}

fn llm_readability_md(html: &str, url: &str) -> String {
    let u = url::Url::parse(url).unwrap();
    llm_readability::extractor::extract(&mut html.as_bytes(), &u)
        .ok()
        .and_then(|p| htmd::convert(&p.content).ok())
        .unwrap_or_default()
}


/// The cleaning pass from the runvil-design survey (2026-11-19), step 2:
/// drop comments, head/script/style/(noscript)/template/iframe/object/embed/svg,
/// hidden elements, and make a[href] / img[src] absolute.
fn survey_clean(html: &str, url: &str, keep_noscript: bool) -> String {
    use lol_html::{comments, element, rewrite_str, RewriteStrSettings};
    let base = url::Url::parse(url).unwrap();
    let mut drop = vec!["head", "script", "style", "template", "iframe", "object", "embed", "svg"];
    if !keep_noscript { drop.push("noscript"); }
    let sel = drop.join(", ");
    let b1 = base.clone();
    let b2 = base.clone();
    let fix = |v: &str, b: &url::Url| -> Option<String> {
        if v.starts_with('#') { return None; }
        b.join(v).ok().map(|u| u.to_string())
    };
    rewrite_str(html, RewriteStrSettings {
        element_content_handlers: vec![
            comments!("*", |c| { c.remove(); Ok(()) }),
            element!(sel.as_str(), |el| { el.remove(); Ok(()) }),
            element!("[hidden], [aria-hidden=true], [style]", |el| {
                let hide = el.has_attribute("hidden")
                    || el.get_attribute("aria-hidden").map_or(false, |v| v == "true")
                    || el.get_attribute("style").map_or(false, |s| {
                        let s: String = s.to_lowercase().chars().filter(|c| !c.is_whitespace()).collect();
                        s.contains("display:none") || s.contains("visibility:hidden")
                    });
                if hide { el.remove(); }
                Ok(())
            }),
            element!("a[href]", move |el| {
                let v = el.get_attribute("href").unwrap();
                let l = v.trim().to_lowercase();
                if l.starts_with("javascript:") || l.starts_with("vbscript:") || l.starts_with("data:") {
                    el.remove_and_keep_content();
                } else if let Some(u) = fix(&v, &b1) { el.set_attribute("href", &u).ok(); }
                Ok(())
            }),
            element!("img[src]", move |el| {
                let v = el.get_attribute("src").unwrap();
                let l = v.trim().to_lowercase();
                if l.starts_with("javascript:") || l.starts_with("vbscript:") || l.starts_with("data:") {
                    el.remove();
                } else if !v.starts_with('#') { if let Ok(u) = b2.join(&v) { el.set_attribute("src", u.as_str()).ok(); } }
                Ok(())
            }),
        ],
        ..RewriteStrSettings::new()
    }).unwrap_or_else(|_| html.to_string())
}

/// The survey's step 5-6: readabilityrs on the cleaned HTML; whole-page htmd when
/// m < 250, or m < 3000 and m < 0.2 * w.
fn survey_design(html: &str, url: &str, keep_noscript: bool) -> String {
    let c = survey_clean(html, url, keep_noscript);
    let md = readabilityrs_md(&c, url);
    let m = md.trim().chars().count();
    if m < 250 { return htmd_raw(&c); }
    if m < 3000 {
        let w_md = htmd_raw(&c);
        let w = w_md.trim().chars().count();
        if (m as f64) < 0.2 * (w as f64) { return w_md; }
    }
    md
}

type Method = (&'static str, fn(&str, &str) -> String);

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (fix, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let criteria: BTreeMap<String, serde_json::Value> = args
        .get(3)
        .map(|p| serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap())
        .unwrap_or_default();

    let methods: Vec<Method> = vec![
        ("runvil_current", runvil_current),
        ("runvil_ratio_fallback", runvil_ratio),
        ("htmd_raw_page", |h, _| htmd_raw(h)),
        ("htmd_cleaned_page", |h, _| htmd_cleaned(h)),
        ("dom_smoothie", dom_smoothie_md),
        ("dom_smoothie_gated", dom_smoothie_gated),
        ("legible", legible_md),
        ("rs_trafilatura", |h, u| trafilatura_md(h, u, false)),
        ("rs_trafilatura_recall", |h, u| trafilatura_md(h, u, true)),
        ("llm_readability", llm_readability_md),
        ("survey_design", |h, u| survey_design(h, u, false)),
        ("survey_design_keep_noscript", |h, u| survey_design(h, u, true)),
    ];

    let mut pages: Vec<_> = fs::read_dir(fix).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    pages.sort();

    println!("method\tpage\tchars\tms\tinclude_hit\tinclude_total\texclude_leaks\texclude_total\tmissing\tleaked");
    for (name, f) in &methods {
        fs::create_dir_all(out.join(name)).unwrap();
        for p in &pages {
            let page = p.file_stem().unwrap().to_string_lossy().to_string();
            let html = fs::read_to_string(p).unwrap_or_else(|_| String::from_utf8_lossy(&fs::read(p).unwrap()).into());
            let crit = criteria.get(&page);
            let url = crit.and_then(|c| c["url"].as_str()).unwrap_or("https://example.com/").to_string();
            // median of 3 runs
            let mut times = vec![];
            let mut text = String::new();
            for _ in 0..std::env::var("RUNS").ok().and_then(|v| v.parse::<usize>().ok()).unwrap_or(3) {
                let t = Instant::now();
                let h = html.clone();
                let u = url.clone();
                let ff = *f;
                text = std::panic::catch_unwind(move || ff(&h, &u)).unwrap_or_else(|_| "<<PANIC>>".into());
                times.push(t.elapsed().as_secs_f64() * 1000.0);
            }
            times.sort_by(|a, b| a.partial_cmp(b).unwrap());
            fs::write(out.join(name).join(format!("{page}.md")), &text).unwrap();
            let low = text.to_lowercase();
            let list = |k: &str| -> Vec<String> {
                crit.and_then(|c| c[k].as_array()).map(|a| a.iter().filter_map(|x| x.as_str().map(|s| s.to_string())).collect()).unwrap_or_default()
            };
            let inc = list("include");
            let exc = list("exclude");
            let missing: Vec<_> = inc.iter().filter(|s| !low.contains(&s.to_lowercase())).cloned().collect();
            let leaked: Vec<_> = exc.iter().filter(|s| low.contains(&s.to_lowercase())).cloned().collect();
            eprintln!(
                "{name}\t{page}\t{}\t{:.1}\t{}\t{}\t{}\t{}\t{}\t{}",
                text.chars().count(), times[times.len()/2], inc.len() - missing.len(), inc.len(), leaked.len(), exc.len(),
                missing.join(" | "), leaked.join(" | ")
            );
        }
    }
}
