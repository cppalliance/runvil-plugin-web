//! Runs PDF-to-text/markdown crates over saved PDFs and scores them.
//! Usage: pdf-bench <pdf_dir> <out_dir> <criteria.json>
use std::{collections::BTreeMap, fs, path::Path, time::Instant};

fn pdf_oxide_md(p: &Path) -> Result<String, String> {
    let doc = pdf_oxide::PdfDocument::open(p).map_err(|e| e.to_string())?;
    doc.to_markdown_all(&Default::default()).map_err(|e| e.to_string())
}
fn unpdf_md(p: &Path) -> Result<String, String> {
    let doc = unpdf::parse_file(p).map_err(|e| e.to_string())?;
    unpdf::render::to_markdown(&doc, &unpdf::render::RenderOptions::default()).map_err(|e| e.to_string())
}
fn pdf_extract_txt(p: &Path) -> Result<String, String> {
    let bytes = fs::read(p).map_err(|e| e.to_string())?;
    pdf_extract::extract_text_from_mem(&bytes).map_err(|e| e.to_string())
}
fn oxidize_md(p: &Path) -> Result<String, String> {
    let doc = oxidize_pdf::parser::PdfDocument::open(p).map_err(|e| e.to_string())?;
    doc.to_markdown().map_err(|e| e.to_string())
}

/// Collapse whitespace so phrases that wrap across lines still match.
fn norm(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ").to_lowercase()
}

fn main() {
    let a: Vec<String> = std::env::args().collect();
    let (dir, out) = (Path::new(&a[1]), Path::new(&a[2]));
    let crit: BTreeMap<String, serde_json::Value> = serde_json::from_str(&fs::read_to_string(&a[3]).unwrap()).unwrap();
    let methods: Vec<(&str, fn(&Path) -> Result<String, String>)> = vec![
        ("pdf_oxide", pdf_oxide_md),
        ("unpdf", unpdf_md),
        ("pdf_extract", pdf_extract_txt),
        ("oxidize_pdf", oxidize_md),
    ];
    let mut files: Vec<_> = fs::read_dir(dir).unwrap().filter_map(|e| e.ok()).map(|e| e.path()).collect();
    files.sort();
    println!("method\tpdf\tok\tchars\tms\tphrase_hit\tphrase_total\tmissing");
    for (name, f) in &methods {
        fs::create_dir_all(out.join(name)).unwrap();
        for p in &files {
            let stem = p.file_stem().unwrap().to_string_lossy().to_string();
            let t = Instant::now();
            let pp = p.clone();
            let ff = *f;
            let res = std::panic::catch_unwind(move || ff(&pp)).unwrap_or_else(|_| Err("PANIC".into()));
            let ms = t.elapsed().as_secs_f64() * 1000.0;
            let (ok, text) = match res { Ok(t) => ("ok", t), Err(e) => ("ERR", format!("<<{e}>>")) };
            fs::write(out.join(name).join(format!("{stem}.md")), &text).unwrap();
            let n = norm(&text);
            let phrases: Vec<String> = crit.get(&stem).and_then(|c| c["include"].as_array())
                .map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default();
            let missing: Vec<_> = phrases.iter().filter(|s| !n.contains(&norm(s))).cloned().collect();
            println!("{name}\t{stem}\t{ok}\t{}\t{ms:.0}\t{}\t{}\t{}", text.chars().count(),
                phrases.len() - missing.len(), phrases.len(), missing.join(" | "));
        }
    }
}
