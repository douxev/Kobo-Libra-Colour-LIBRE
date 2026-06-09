// comic-panelize — desktop preprocessor: CBZ -> panel-per-page CBZ (guided view) + sidecar.
// Detection runs ONCE here (off-device); Nickel just reads the panel-per-page CBZ.
use comic::pipeline::{process_cbz, Opts};
use comic::render::RenderOpts;
use comic::Direction;
use std::path::{Path, PathBuf};

fn usage() -> ! {
    eprintln!(
        "usage:\n  \
         comic-panelize <input.cbz> [-o out.cbz] [--sidecar f.json] [options]\n  \
         comic-panelize --batch <dir> [--out-dir <dir>] [options]\n\n\
         options:\n  \
         --direction ltr|rtl     reading direction (default ltr; rtl = manga)\n  \
         --canvas WxH            target screen size (default 1264x1680)\n  \
         --padding N             pixels around each panel (default 12)\n  \
         --no-upscale            don't enlarge small panels\n  \
         --full-page             prepend the whole page before its panels\n  \
         --quality N             output JPEG quality (default 88)\n  \
         --force                 re-detect even if a valid sidecar cache exists\n"
    );
    std::process::exit(2);
}

struct Args {
    input: Option<PathBuf>,
    output: Option<PathBuf>,
    sidecar: Option<PathBuf>,
    batch: Option<PathBuf>,
    out_dir: Option<PathBuf>,
    opts: Opts,
}

fn parse_canvas(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once(['x', 'X', '*'])?;
    Some((w.trim().parse().ok()?, h.trim().parse().ok()?))
}

fn parse_args() -> Args {
    let mut a = Args {
        input: None, output: None, sidecar: None, batch: None, out_dir: None,
        opts: Opts { direction: Direction::Ltr, render: RenderOpts::default(), force: false },
    };
    let argv: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < argv.len() {
        match argv[i].as_str() {
            "-o" | "--output" => { i += 1; a.output = argv.get(i).map(PathBuf::from); }
            "--sidecar" => { i += 1; a.sidecar = argv.get(i).map(PathBuf::from); }
            "--batch" => { i += 1; a.batch = argv.get(i).map(PathBuf::from); }
            "--out-dir" => { i += 1; a.out_dir = argv.get(i).map(PathBuf::from); }
            "--direction" => { i += 1; if argv.get(i).map(|s| s.as_str()) == Some("rtl") { a.opts.direction = Direction::Rtl; } }
            "--canvas" => { i += 1; if let Some((w, h)) = argv.get(i).and_then(|s| parse_canvas(s)) { a.opts.render.canvas_w = w; a.opts.render.canvas_h = h; } }
            "--padding" => { i += 1; if let Some(p) = argv.get(i).and_then(|s| s.parse().ok()) { a.opts.render.padding = p; } }
            "--quality" => { i += 1; if let Some(q) = argv.get(i).and_then(|s| s.parse().ok()) { a.opts.render.jpeg_quality = q; } }
            "--no-upscale" => a.opts.render.upscale = false,
            "--full-page" => a.opts.render.include_full_page = true,
            "--force" => a.opts.force = true,
            "-h" | "--help" => usage(),
            s if !s.starts_with('-') && a.input.is_none() => a.input = Some(PathBuf::from(s)),
            _ => {}
        }
        i += 1;
    }
    a
}

fn default_out(input: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "out".into());
    input.with_file_name(format!("{}.panels.cbz", stem))
}
fn default_sidecar(input: &Path) -> PathBuf {
    let stem = input.file_stem().map(|s| s.to_string_lossy().to_string()).unwrap_or_else(|| "out".into());
    input.with_file_name(format!("{}.panels.json", stem))
}

fn run_one(input: &Path, output: &Path, sidecar: &Path, opts: &Opts) -> i32 {
    match process_cbz(input, output, sidecar, opts) {
        Ok((sc, n)) => {
            let panels: usize = sc.pages.iter().map(|p| p.panels.len()).sum();
            println!("{} -> {} ({} pages, {} panels)", input.display(), output.display(), n, panels);
            0
        }
        Err(e) => { eprintln!("ERROR {}: {}", input.display(), e); 1 }
    }
}

fn main() {
    let a = parse_args();
    if let Some(dir) = &a.batch {
        let out_dir = a.out_dir.clone().unwrap_or_else(|| dir.clone());
        let _ = std::fs::create_dir_all(&out_dir);
        let mut rc = 0;
        let entries = match std::fs::read_dir(dir) {
            Ok(e) => e,
            Err(e) => { eprintln!("cannot read {}: {}", dir.display(), e); std::process::exit(1); }
        };
        for ent in entries.flatten() {
            let p = ent.path();
            let fname = p.file_name().map(|s| s.to_string_lossy().to_lowercase()).unwrap_or_default();
            // Ne pas re-paneliser une sortie déjà générée.
            if fname.ends_with(".panels.cbz") { continue; }
            if p.extension().map(|e| e.eq_ignore_ascii_case("cbz")).unwrap_or(false) {
                let stem = p.file_stem().unwrap().to_string_lossy().to_string();
                let out = out_dir.join(format!("{}.panels.cbz", stem));
                let sc = out_dir.join(format!("{}.panels.json", stem));
                rc |= run_one(&p, &out, &sc, &a.opts);
            }
        }
        std::process::exit(rc);
    }
    let input = match a.input { Some(p) => p, None => usage() };
    let output = a.output.unwrap_or_else(|| default_out(&input));
    let sidecar = a.sidecar.unwrap_or_else(|| default_sidecar(&input));
    std::process::exit(run_one(&input, &output, &sidecar, &a.opts));
}
