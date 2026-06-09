// convert — HTML->EPUB (extraction readability), CBR->CBZ, métadonnées/couverture EPUB.
use std::io::{Cursor, Read, Write};
use std::path::Path;
use zip::write::SimpleFileOptions;
use zip::{CompressionMethod, ZipArchive, ZipWriter};

// ============================ HTML -> EPUB ============================

/// Extraction "readability" légère : retire script/style/nav/header/footer/aside,
/// garde le bloc de contenu le plus dense, renvoie du XHTML propre.
fn extract_readable(html: &str) -> String {
    use scraper::{Html, Selector};
    let doc = Html::parse_document(html);
    // candidats : article, main, [role=main], #content, .content, body
    let candidates = ["article", "main", "[role=main]", "#content", ".content", "body"];
    let drop = Selector::parse("script,style,nav,header,footer,aside,noscript,iframe,form").unwrap();
    let mut best_html = String::new();
    let mut best_len = 0usize;
    for sel in candidates {
        if let Ok(s) = Selector::parse(sel) {
            if let Some(el) = doc.select(&s).next() {
                let text_len: usize = el.text().map(|t| t.trim().len()).sum();
                if text_len > best_len {
                    best_len = text_len;
                    best_html = el.inner_html();
                }
            }
        }
    }
    if best_html.is_empty() {
        best_html = html.to_string();
    }
    // nettoie le fragment retenu
    let frag = Html::parse_fragment(&best_html);
    let mut out = String::new();
    for node in frag.tree.nodes() {
        if let scraper::node::Node::Element(e) = node.value() {
            let _ = e; // (on reconstruit via inner_html du root)
        }
    }
    // retire les éléments indésirables du fragment
    let cleaned = {
        let f = Html::parse_fragment(&best_html);
        let root = f.root_element();
        // scraper ne mute pas l'arbre ; on filtre par sérialisation simple :
        let mut buf = String::new();
        serialize_filtered(&root, &drop, &mut buf);
        buf
    };
    if cleaned.trim().is_empty() { out = best_html; } else { out = cleaned; }
    sanitize_xhtml(&out)
}

fn serialize_filtered(el: &scraper::ElementRef, drop: &scraper::Selector, out: &mut String) {
    for child in el.children() {
        match child.value() {
            scraper::node::Node::Text(t) => out.push_str(&escape_xml(t)),
            scraper::node::Node::Element(e) => {
                if let Some(cref) = scraper::ElementRef::wrap(child) {
                    if drop.matches(&cref) { continue; }
                    let name = e.name();
                    // ne garde que des balises de contenu sûres
                    let keep = matches!(name,
                        "p"|"br"|"h1"|"h2"|"h3"|"h4"|"h5"|"h6"|"ul"|"ol"|"li"|"blockquote"|
                        "strong"|"b"|"em"|"i"|"a"|"img"|"figure"|"figcaption"|"pre"|"code"|"div"|"span"|"hr");
                    if keep {
                        out.push('<'); out.push_str(name);
                        if name == "img" {
                            if let Some(src) = e.attr("src") { out.push_str(&format!(" src=\"{}\"", escape_xml(src))); }
                            if let Some(alt) = e.attr("alt") { out.push_str(&format!(" alt=\"{}\"", escape_xml(alt))); }
                            out.push_str("/>");
                            continue;
                        }
                        if name == "a" {
                            if let Some(href) = e.attr("href") { out.push_str(&format!(" href=\"{}\"", escape_xml(href))); }
                        }
                        out.push('>');
                        serialize_filtered(&cref, drop, out);
                        out.push_str("</"); out.push_str(name); out.push('>');
                    } else {
                        serialize_filtered(&cref, drop, out);
                    }
                }
            }
            _ => {}
        }
    }
}

fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;").replace('<', "&lt;").replace('>', "&gt;").replace('"', "&quot;")
}
fn sanitize_xhtml(s: &str) -> String {
    // br/img/hr déjà auto-fermés par notre sérialiseur ; rien d'autre à faire.
    s.to_string()
}

/// Construit un EPUB2 minimal valide à partir d'un fragment HTML.
pub fn html_to_epub(html: &str, title: &str, author: &str) -> Result<Vec<u8>, String> {
    let body = extract_readable(html);
    let t = escape_xml(title);
    let a = escape_xml(author);
    let uid = format!("urn:opds-sync:{}", simple_hash(&format!("{}{}", title, author)));

    let chapter = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<!DOCTYPE html>\n<html xmlns=\"http://www.w3.org/1999/xhtml\">\
<head><title>{t}</title><meta charset=\"utf-8\"/></head><body><h1>{t}</h1>\n{body}\n</body></html>");
    let opf = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<package xmlns=\"http://www.idpf.org/2007/opf\" version=\"2.0\" unique-identifier=\"bookid\">\
<metadata xmlns:dc=\"http://purl.org/dc/elements/1.1/\" xmlns:opf=\"http://www.idpf.org/2007/opf\">\
<dc:title>{t}</dc:title><dc:creator opf:role=\"aut\">{a}</dc:creator><dc:language>fr</dc:language>\
<dc:identifier id=\"bookid\">{uid}</dc:identifier></metadata>\
<manifest><item id=\"ch\" href=\"chapter.xhtml\" media-type=\"application/xhtml+xml\"/>\
<item id=\"ncx\" href=\"toc.ncx\" media-type=\"application/x-dtbncx+xml\"/></manifest>\
<spine toc=\"ncx\"><itemref idref=\"ch\"/></spine></package>");
    let ncx = format!(
        "<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<ncx xmlns=\"http://www.daisy.org/z3986/2005/ncx/\" version=\"2005-1\">\
<head><meta name=\"dtb:uid\" content=\"{uid}\"/></head><docTitle><text>{t}</text></docTitle>\
<navMap><navPoint id=\"n1\" playOrder=\"1\"><navLabel><text>{t}</text></navLabel><content src=\"chapter.xhtml\"/></navPoint></navMap></ncx>");
    let container = "<?xml version=\"1.0\"?>\n<container version=\"1.0\" xmlns=\"urn:oasis:names:tc:opendocument:xmlns:container\">\
<rootfiles><rootfile full-path=\"OEBPS/content.opf\" media-type=\"application/oebps-package+xml\"/></rootfiles></container>";

    let buf = Vec::new();
    let mut zw = ZipWriter::new(Cursor::new(buf));
    // mimetype EN PREMIER, non compressé (exigence EPUB)
    zw.start_file("mimetype", SimpleFileOptions::default().compression_method(CompressionMethod::Stored))
        .map_err(|e| e.to_string())?;
    zw.write_all(b"application/epub+zip").map_err(|e| e.to_string())?;
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, content) in [
        ("META-INF/container.xml", container.to_string()),
        ("OEBPS/content.opf", opf),
        ("OEBPS/toc.ncx", ncx),
        ("OEBPS/chapter.xhtml", chapter),
    ] {
        zw.start_file(name, opts).map_err(|e| e.to_string())?;
        zw.write_all(content.as_bytes()).map_err(|e| e.to_string())?;
    }
    let cur = zw.finish().map_err(|e| e.to_string())?;
    Ok(cur.into_inner())
}

fn simple_hash(s: &str) -> u64 {
    let mut h = 1469598103934665603u64;
    for b in s.bytes() { h ^= b as u64; h = h.wrapping_mul(1099511628211); }
    h
}

// ============================ CBR -> CBZ ============================

/// Extrait un .cbr (RAR) et le réécrit en .cbz (ZIP) — pour la lecture BD dans Nickel.
pub fn cbr_to_cbz(input: &Path, output: &Path) -> Result<(), String> {
    use unrar::Archive;
    let mut images: Vec<(String, Vec<u8>)> = Vec::new();
    let mut archive = Archive::new(input).open_for_processing().map_err(|e| format!("ouverture RAR : {}", e))?;
    while let Some(header) = archive.read_header().map_err(|e| format!("lecture RAR : {}", e))? {
        let is_file = header.entry().is_file();
        let name = header.entry().filename.to_string_lossy().replace('\\', "/");
        archive = if is_file {
            let (data, rest) = header.read().map_err(|e| format!("extraction RAR : {}", e))?;
            let lower = name.to_lowercase();
            if lower.ends_with(".jpg") || lower.ends_with(".jpeg") || lower.ends_with(".png")
                || lower.ends_with(".gif") || lower.ends_with(".webp") || lower.ends_with(".bmp") {
                images.push((name, data));
            }
            rest
        } else {
            header.skip().map_err(|e| format!("skip RAR : {}", e))?
        };
    }
    if images.is_empty() {
        return Err("aucune image trouvée dans le CBR".to_string());
    }
    images.sort_by(|a, b| a.0.cmp(&b.0));
    let f = std::fs::File::create(output).map_err(|e| e.to_string())?;
    let mut zw = ZipWriter::new(f);
    let opts = SimpleFileOptions::default().compression_method(CompressionMethod::Stored); // images déjà compressées
    for (name, data) in images {
        let base = name.rsplit('/').next().unwrap_or(&name).to_string();
        zw.start_file(base, opts).map_err(|e| e.to_string())?;
        zw.write_all(&data).map_err(|e| e.to_string())?;
    }
    zw.finish().map_err(|e| e.to_string())?;
    Ok(())
}

// ============================ Métadonnées EPUB ============================

pub struct BookMeta {
    pub title: String,
    pub author: String,
    pub language: String,
    pub cover: Option<Vec<u8>>,
    pub cover_mime: String,
}

/// Lit titre/auteur/langue + couverture depuis un .epub (ZIP + OPF).
pub fn epub_meta(path: &Path) -> Result<BookMeta, String> {
    let f = std::fs::File::open(path).map_err(|e| e.to_string())?;
    let mut zip = ZipArchive::new(f).map_err(|e| e.to_string())?;

    // 1) localiser l'OPF via META-INF/container.xml
    let opf_path = {
        let mut s = String::new();
        zip.by_name("META-INF/container.xml").map_err(|e| e.to_string())?.read_to_string(&mut s).map_err(|e| e.to_string())?;
        let doc = roxmltree::Document::parse(&s).map_err(|e| e.to_string())?;
        doc.descendants().find(|n| n.has_tag_name("rootfile"))
            .and_then(|n| n.attribute("full-path")).map(|s| s.to_string())
            .ok_or_else(|| "rootfile introuvable".to_string())?
    };
    // 2) lire l'OPF
    let mut opf = String::new();
    zip.by_name(&opf_path).map_err(|e| e.to_string())?.read_to_string(&mut opf).map_err(|e| e.to_string())?;
    let doc = roxmltree::Document::parse(&opf).map_err(|e| e.to_string())?;
    let txt = |tag: &str| doc.descendants().find(|n| n.has_tag_name(tag)).and_then(|n| n.text()).unwrap_or("").trim().to_string();
    let title = txt("title");
    let author = txt("creator");
    let language = { let l = txt("language"); if l.is_empty() { "fr".into() } else { l } };

    // 3) couverture : <meta name="cover" content="ID"> -> item href ; sinon item avec properties=cover-image
    let base_dir = Path::new(&opf_path).parent().map(|p| p.to_string_lossy().to_string()).unwrap_or_default();
    let mut cover_href = None;
    let cover_id = doc.descendants().find(|n| n.has_tag_name("meta") && n.attribute("name") == Some("cover"))
        .and_then(|n| n.attribute("content"));
    for item in doc.descendants().filter(|n| n.has_tag_name("item")) {
        let id = item.attribute("id").unwrap_or("");
        let props = item.attribute("properties").unwrap_or("");
        if Some(id) == cover_id || props.contains("cover-image") {
            cover_href = item.attribute("href").map(|h| h.to_string());
            break;
        }
    }
    let (cover, cover_mime) = if let Some(href) = cover_href {
        let full = if base_dir.is_empty() { href.clone() } else { format!("{}/{}", base_dir, href) };
        let full = full.replace("//", "/");
        let mut data = Vec::new();
        let mime = match zip.by_name(&full) {
            Ok(mut zf) => { zf.read_to_end(&mut data).ok(); guess_mime(&full) }
            Err(_) => String::new(),
        };
        if data.is_empty() { (None, String::new()) } else { (Some(data), mime) }
    } else { (None, String::new()) };

    Ok(BookMeta { title, author, language, cover, cover_mime })
}

fn guess_mime(path: &str) -> String {
    let p = path.to_lowercase();
    if p.ends_with(".png") { "image/png" }
    else if p.ends_with(".gif") { "image/gif" }
    else if p.ends_with(".webp") { "image/webp" }
    else { "image/jpeg" }.to_string()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn html_to_epub_roundtrip() {
        let html = "<html><body><nav>menu</nav><article><h2>Titre</h2><p>Bonjour <b>monde</b> & <script>x</script> fin.</p></article><footer>pied</footer></body></html>";
        let epub = html_to_epub(html, "Mon Article", "exemple.com").expect("epub");
        assert!(epub.len() > 100, "epub non vide");
        // mimetype présent (nom "mimetype" à l'offset 30, contenu juste après)
        assert!(epub.windows(20).any(|w| w == b"application/epub+zip"), "mimetype EPUB présent");
        // écrire + relire les métadonnées
        let tmp = std::env::temp_dir().join("convert_test.epub");
        std::fs::write(&tmp, &epub).unwrap();
        let meta = epub_meta(&tmp).expect("meta");
        assert_eq!(meta.title, "Mon Article");
        assert_eq!(meta.author, "exemple.com");
        let _ = std::fs::remove_file(&tmp);
    }
}
