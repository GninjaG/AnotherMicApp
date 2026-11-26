use micclean::fx::chain::Preset;
use std::path::{Path, PathBuf};

pub struct Store {
    pub dir: PathBuf,
}

fn file_name(name: &str) -> String {
    let s: String = name.chars().map(|c| if "<>:\"/\\|?*".contains(c) || c.is_control() { '_' } else { c }).collect();
    let s = s.trim().trim_end_matches('.').to_string();
    if s.is_empty() { "preset".into() } else { s }
}

impl Store {
    pub fn new(base: &Path) -> Store {
        let dir = base.join("presets");
        let _ = std::fs::create_dir_all(&dir);
        Store { dir }
    }

    fn path(&self, name: &str) -> PathBuf {
        self.dir.join(format!("{}.toml", file_name(name)))
    }

    pub fn list(&self) -> Vec<String> {
        let mut v: Vec<String> = std::fs::read_dir(&self.dir)
            .into_iter()
            .flatten()
            .flatten()
            .filter(|e| e.path().extension().is_some_and(|x| x == "toml"))
            .filter_map(|e| std::fs::read_to_string(e.path()).ok())
            .filter_map(|t| toml::from_str::<Preset>(&t).ok().map(|p| p.name))
            .collect();
        v.sort_by_key(|s| s.to_lowercase());
        v.dedup();
        v
    }

    pub fn exists(&self, name: &str) -> bool {
        self.path(name).exists()
    }

    pub fn load(&self, name: &str) -> Result<Preset, String> {
        let t = std::fs::read_to_string(self.path(name)).map_err(|e| format!("{name}: {e}"))?;
        let mut p: Preset = toml::from_str(&t).map_err(|e| format!("{name}: {e}"))?;
        p.name = name.to_string();
        Ok(p)
    }

    pub fn save(&self, p: &Preset) -> Result<(), String> {
        let s = toml::to_string_pretty(p).map_err(|e| e.to_string())?;
        let path = self.path(&p.name);
        let tmp = path.with_extension("toml.tmp");
        std::fs::write(&tmp, s).map_err(|e| e.to_string())?;
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())
    }

    /// Nada é apagado: o arquivo vai para presets/_excluidos.
    pub fn remove(&self, name: &str) -> Result<(), String> {
        let bin = self.dir.join("_excluidos");
        std::fs::create_dir_all(&bin).map_err(|e| e.to_string())?;
        let stamp = chrono_stamp();
        let to = bin.join(format!("{}-{stamp}.toml", file_name(name)));
        std::fs::rename(self.path(name), to).map_err(|e| e.to_string())
    }

    pub fn rename(&self, old: &str, p: &Preset) -> Result<(), String> {
        self.save(p)?;
        if !file_name(old).eq_ignore_ascii_case(&file_name(&p.name)) {
            let _ = std::fs::remove_file(self.path(old));
        }
        Ok(())
    }

    pub fn unique(&self, base: &str) -> String {
        if !self.exists(base) {
            return base.to_string();
        }
        (2..).map(|i| format!("{base} {i}")).find(|n| !self.exists(n)).unwrap()
    }
}

pub fn chrono_stamp() -> String {
    let s = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0) as i64;
    // data civil a partir de segundos (UTC), sem dependência extra
    let days = s.div_euclid(86400);
    let secs = s.rem_euclid(86400);
    let z = days + 719468;
    let era = z.div_euclid(146097);
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    format!("{y:04}{m:02}{d:02}-{:02}{:02}{:02}", secs / 3600, secs % 3600 / 60, secs % 60)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rename_mesmo_nome_outra_caixa_nao_apaga_arquivo() {
        let dir = std::env::temp_dir().join("ama_presets_case_test");
        let _ = std::fs::remove_dir_all(&dir);
        let store = Store::new(&dir);
        let p = Preset::default_chain("Teste");
        store.save(&p).unwrap();
        let mut p2 = p.clone();
        p2.name = "teste".into();
        store.rename("Teste", &p2).unwrap();
        assert!(store.exists("teste"));
        assert!(store.load("teste").is_ok());
        let _ = std::fs::remove_dir_all(&dir);
    }
}
