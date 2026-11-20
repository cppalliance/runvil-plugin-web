use std::{collections::BTreeMap, fs, path::Path, time::Instant};
const NOISE_TAGS: &[&str] = &["script","style","noscript","svg","nav","header","footer","aside","form","iframe","template","button","dialog"];
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

type Method = (&'static str, fn(&str, &str) -> String);
fn main() {
    let args: Vec<String> = std::env::args().collect();
    let (fix, out) = (Path::new(&args[1]), Path::new(&args[2]));
    let criteria: BTreeMap<String, serde_json::Value> = args
        .get(3)
        .map(|p| serde_json::from_str(&fs::read_to_string(p).unwrap()).unwrap())
        .unwrap_or_default();

    let methods: Vec<Method> = vec![
        ("runvil_current_pinned", runvil_current),
        ("runvil_ratio_pinned", runvil_ratio),
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
