//! Importa e exporta configs no formato do Equalizer APO.
use super::biquad::{BqParams, BqType, Width};
use super::chain::{BlockCfg, GeqMode, Kind};
use super::geq::{BANDS_15, BANDS_31};
use std::collections::BTreeMap;
use std::fmt::Write;
use std::path::Path;

fn num(s: &str) -> Option<f64> {
    s.replace(',', ".").trim().parse::<f64>().ok().filter(|v| v.is_finite())
}

fn parse_preamp(rest: &str) -> Option<f64> {
    let tok = rest.split('#').next().unwrap_or("").split_whitespace().next()?;
    let tok = tok
        .strip_suffix("dB")
        .or_else(|| tok.strip_suffix("db"))
        .or_else(|| tok.strip_suffix("DB"))
        .or_else(|| tok.strip_suffix("Db"))
        .unwrap_or(tok);
    num(tok)
}

fn split_quoted(s: &str) -> Vec<String> {
    let mut v = Vec::new();
    let mut cur = String::new();
    let mut q = false;
    for c in s.chars() {
        match c {
            '"' => q = !q,
            ' ' | '\t' if !q => {
                if !cur.is_empty() {
                    v.push(std::mem::take(&mut cur));
                }
            }
            _ => cur.push(c),
        }
    }
    if !cur.is_empty() {
        v.push(cur);
    }
    v
}

fn parse_filter(rest: &str) -> Result<(bool, BqParams), String> {
    let t: Vec<&str> = rest.split_whitespace().collect();
    let on = match t.first() {
        Some(&"ON") => true,
        Some(&"OFF") => false,
        _ => return Err(format!("filtro sem ON/OFF: {rest}")),
    };
    let ty = *t.get(1).ok_or("filtro sem tipo")?;
    let kind = match ty {
        "PK" | "PEQ" | "Modal" => BqType::Peak,
        "LP" | "LPQ" => BqType::LowPass,
        "HP" | "HPQ" => BqType::HighPass,
        "BP" => BqType::BandPass,
        "LS" | "LSC" => BqType::LowShelf,
        "HS" | "HSC" => BqType::HighShelf,
        "NO" => BqType::Notch,
        "AP" => BqType::AllPass,
        _ => return Err(format!("tipo de filtro não suportado: {ty}")),
    };
    let after = |k: &str| t.iter().position(|x| *x == k).and_then(|i| t.get(i + 1)).and_then(|v| num(v));
    let mut p = BqParams {
        kind,
        freq: after("Fc").ok_or("filtro sem Fc")?,
        gain: after("Gain").unwrap_or(0.0),
        width: 0.0,
        width_kind: Width::Q,
        corner: kind.is_shelf() && !ty.ends_with('C'),
    };
    if let Some(q) = after("Q") {
        p.width = q;
    }
    if let Some(i) = t.iter().position(|x| *x == "BW") {
        if let Some(v) = t.get(i + 2).and_then(|v| num(v)) {
            p.width = v;
            p.width_kind = Width::Bw;
        }
    }
    if kind.is_shelf() && t.get(3) == Some(&"dB") {
        if let Some(v) = t.get(2).and_then(|v| num(v)) {
            p.width = v;
            p.width_kind = Width::Slope;
        }
    }
    Ok((on, p))
}

fn geq_mode(nodes: &[(f64, f64)]) -> GeqMode {
    let fs: Vec<f64> = nodes.iter().map(|n| n.0).collect();
    if fs == BANDS_31 {
        GeqMode::Bands31
    } else if fs == BANDS_15 {
        GeqMode::Bands15
    } else {
        GeqMode::Free
    }
}

/// Retorna blocos e avisos (linhas ignoradas).
pub fn import(path: &Path) -> Result<(Vec<BlockCfg>, Vec<String>), String> {
    let mut out = Vec::new();
    let mut warn = Vec::new();
    import_into(path, &mut out, &mut warn, 0)?;
    for (i, b) in out.iter_mut().enumerate() {
        b.id = i as u32 + 1;
    }
    Ok((out, warn))
}

fn import_into(path: &Path, out: &mut Vec<BlockCfg>, warn: &mut Vec<String>, depth: usize) -> Result<(), String> {
    let txt = std::fs::read(path).map_err(|e| format!("{}: {e}", path.display()))?;
    let txt = String::from_utf8_lossy(&txt).into_owned();
    let dir = path.parent().unwrap_or(Path::new("."));
    for (ln, raw) in txt.lines().enumerate() {
        let line = raw.trim().trim_start_matches('\u{feff}');
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((cmd, rest)) = line.split_once(':') else { continue };
        let (cmd, rest) = (cmd.trim(), rest.trim());
        let mut add: Vec<(bool, Kind)> = Vec::new();
        let mut push = |enabled, kind| add.push((enabled, kind));
        let res: Result<(), String> = (|| {
            match cmd {
                "Preamp" => push(true, Kind::Preamp { gain_db: parse_preamp(rest).ok_or("preamp inválido")? }),
                c if c.starts_with("Filter") => {
                    let (on, p) = parse_filter(rest)?;
                    push(on, Kind::Filter { p });
                }
                "GraphicEQ" => {
                    let mut nodes = Vec::new();
                    for part in rest.split(';') {
                        let v: Vec<f64> = part.split_whitespace().filter_map(num).collect();
                        if v.len() >= 2 {
                            nodes.push((v[0], v[1]));
                        }
                    }
                    nodes.sort_by(|a, b| a.0.total_cmp(&b.0));
                    push(true, Kind::GraphicEq { mode: geq_mode(&nodes), nodes });
                }
                "LoudnessCorrection" => {
                    let t: Vec<&str> = rest.split_whitespace().collect();
                    let g = |k: &str| t.iter().position(|x| *x == k).and_then(|i| t.get(i + 1)).and_then(|v| num(v));
                    push(
                        g("State").unwrap_or(1.0) != 0.0,
                        Kind::Loudness {
                            ref_level: g("ReferenceLevel").unwrap_or(75.0),
                            ref_offset: g("ReferenceOffset").unwrap_or(0.0),
                            attenuation: g("Attenuation").unwrap_or(1.0),
                        },
                    );
                }
                "Delay" => {
                    let t: Vec<&str> = rest.split_whitespace().collect();
                    let v = t.first().and_then(|v| num(v)).ok_or("delay inválido")?;
                    let ms = if t.get(1).is_some_and(|u| u.eq_ignore_ascii_case("samples")) { v * 1000.0 / crate::SR as f64 } else { v };
                    push(true, Kind::Delay { ms });
                }
                "Convolution" => {
                    let f = rest.trim_matches('"');
                    let p = if Path::new(f).is_absolute() { f.to_string() } else { dir.join(f).to_string_lossy().into_owned() };
                    push(true, Kind::Convolution { file: p });
                }
                "VSTPlugin" => {
                    let parts = split_quoted(rest);
                    let (mut lib, mut chunk, mut params) = (String::new(), None, BTreeMap::new());
                    for kv in parts.chunks(2) {
                        if kv.len() < 2 {
                            continue;
                        }
                        match kv[0].as_str() {
                            "Library" => {
                                lib = if Path::new(&kv[1]).is_absolute() {
                                    kv[1].clone()
                                } else {
                                    format!(r"C:\Program Files\EqualizerAPO\VSTPlugins\{}", kv[1])
                                }
                            }
                            "ChunkData" => chunk = Some(kv[1].clone()),
                            k => {
                                if let Some(v) = num(&kv[1]) {
                                    params.insert(k.to_string(), v as f32);
                                }
                            }
                        }
                    }
                    push(true, Kind::Vst { library: lib, chunk, params });
                }
                "Include" => {}
                "Device" | "Channel" | "Stage" => warn.push(format!("linha {} ignorada (só faz sentido no EAPO): {line}", ln + 1)),
                _ => warn.push(format!("linha {} não suportada: {line}", ln + 1)),
            }
            Ok(())
        })();
        out.extend(add.into_iter().map(|(enabled, kind)| BlockCfg { id: 0, enabled, kind }));
        if res.is_ok() && cmd == "Include" && depth < 8 {
            for f in rest.split(';') {
                let f = f.trim().trim_matches('"');
                if !f.is_empty() {
                    if let Err(e) = import_into(&dir.join(f), out, warn, depth + 1) {
                        warn.push(format!("linha {}: {e}", ln + 1));
                    }
                }
            }
        }
        if let Err(e) = res {
            warn.push(format!("linha {}: {e}", ln + 1));
        }
    }
    Ok(())
}

fn fmt_filter(p: &BqParams) -> String {
    let ty = match p.kind {
        BqType::Peak => "PK",
        BqType::LowPass => if p.width_kind == Width::Q && p.width > 0.0 { "LPQ" } else { "LP" },
        BqType::HighPass => if p.width_kind == Width::Q && p.width > 0.0 { "HPQ" } else { "HP" },
        BqType::BandPass => "BP",
        BqType::LowShelf => if p.corner { "LS" } else { "LSC" },
        BqType::HighShelf => if p.corner { "HS" } else { "HSC" },
        BqType::Notch => "NO",
        BqType::AllPass => "AP",
    };
    let mut s = format!("ON {ty}");
    if p.kind.is_shelf() && p.width_kind == Width::Slope && p.width > 0.0 {
        let _ = write!(s, " {} dB", p.width);
    }
    let _ = write!(s, " Fc {} Hz", p.freq);
    if p.kind.has_gain() {
        let _ = write!(s, " Gain {} dB", p.gain);
    }
    if p.width > 0.0 {
        match p.width_kind {
            Width::Q => {
                let _ = write!(s, " Q {}", p.width);
            }
            Width::Bw if !p.kind.is_shelf() => {
                let _ = write!(s, " BW Oct {}", p.width);
            }
            _ => {}
        }
    }
    s
}

pub fn export(blocks: &[BlockCfg], base: &Path) -> String {
    let mut s = String::from("# Exportado do AMA (Another Mic App)\n");
    for b in blocks {
        let pre = if b.enabled { "" } else { "# " };
        let line = match &b.kind {
            Kind::Aec { .. } => "# AMA: cancelamento de eco (sem equivalente no Equalizer APO)".to_string(),
            Kind::Denoise { atten_db, .. } => format!("# AMA: DeepFilterNet, limite {atten_db} dB (sem equivalente no Equalizer APO)"),
            Kind::Limiter { ceiling_db, release_ms } => format!("# AMA: limitador {ceiling_db} dB, release {release_ms} ms (sem equivalente no Equalizer APO)"),
            Kind::Compressor { p } => format!("# AMA: compressor {} dB, {}:1, make-up {:.1} dB (sem equivalente no Equalizer APO)", p.threshold_db, p.ratio, p.total_makeup()),
            Kind::DeEsser { p } => format!("# AMA: de-esser {} Hz, {} dB (sem equivalente no Equalizer APO)", p.freq_hz, p.threshold_db),
            Kind::Preamp { gain_db } => format!("{pre}Preamp: {gain_db} dB"),
            Kind::Filter { p } => format!("{pre}Filter: {}", fmt_filter(p)),
            Kind::GraphicEq { nodes, .. } => {
                let v: Vec<String> = nodes.iter().map(|(f, g)| format!("{f} {g}")).collect();
                format!("{pre}GraphicEQ: {}", v.join("; "))
            }
            Kind::Loudness { ref_level, ref_offset, attenuation } => format!(
                "{pre}LoudnessCorrection: State 1 ReferenceLevel {ref_level} ReferenceOffset {ref_offset} Attenuation {attenuation}"
            ),
            Kind::Convolution { file } => format!("{pre}Convolution: {}", base.join(file).display()),
            Kind::Delay { ms } => format!("{pre}Delay: {ms} ms"),
            Kind::Vst { library, chunk, params } => {
                let mut l = format!("{pre}VSTPlugin: Library \"{}\"", base.join(library).display());
                if let Some(c) = chunk {
                    let _ = write!(l, " ChunkData \"{c}\"");
                } else {
                    for (k, v) in params {
                        if k.contains(' ') {
                            let _ = write!(l, " \"{k}\" {v}");
                        } else {
                            let _ = write!(l, " {k} {v}");
                        }
                    }
                }
                l
            }
        };
        s.push_str(&line);
        s.push('\n');
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejeita_nan_e_preserva_avisos_include() {
        let dir = std::env::temp_dir().join("ama_eapo_test");
        let _ = std::fs::create_dir_all(&dir);
        let p = dir.join("cfg.txt");
        std::fs::write(&p, "Preamp: -3.5 db # comentario\nGraphicEQ: 100 NaN; 200 1.5; 500 inf\nInclude: nao_existe_1.txt; nao_existe_2.txt\n").unwrap();
        let (blocks, warn) = import(&p).unwrap();
        assert_eq!(blocks.len(), 2);
        match &blocks[0].kind {
            Kind::Preamp { gain_db } => assert!((*gain_db + 3.5).abs() < 1e-9),
            k => panic!("{k:?}"),
        }
        match &blocks[1].kind {
            Kind::GraphicEq { nodes, .. } => assert_eq!(nodes, &vec![(200.0, 1.5)]),
            k => panic!("{k:?}"),
        }
        assert_eq!(warn.len(), 2, "warn: {warn:?}");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
