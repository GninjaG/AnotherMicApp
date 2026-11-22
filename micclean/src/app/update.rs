//! Atualização pelos releases do GitHub: confere ao abrir e só instala se o usuário pedir.
use micclean::log;
use std::path::PathBuf;

pub const REPO: &str = "GninjaG/AnotherMicApp";
const ASSET: &str = "AMA.exe";

#[derive(Clone, Debug)]
pub struct Release {
    pub tag: String,
    pub page: String,
    exe_url: String,
    size: u64,
}

fn wide(s: &str) -> Vec<u16> {
    s.encode_utf16().chain(std::iter::once(0)).collect()
}

/// GET por HTTPS com o WinHTTP do Windows (proxy e certificados do sistema, segue redirecionamentos).
fn get(url: &str) -> Result<Vec<u8>, String> {
    use windows::Win32::Networking::WinHttp::*;
    use windows::core::PCWSTR;
    let rest = url.strip_prefix("https://").ok_or("só https")?;
    let (host, path) = rest.split_once('/').map(|(h, p)| (h, format!("/{p}"))).unwrap_or((rest, "/".into()));
    struct H(*mut core::ffi::c_void);
    impl Drop for H {
        fn drop(&mut self) {
            if !self.0.is_null() {
                unsafe {
                    let _ = WinHttpCloseHandle(self.0);
                }
            }
        }
    }
    let err = |w: &str| format!("{w}: {}", std::io::Error::last_os_error());
    unsafe {
        let ua = wide(&format!("AMA/{}", env!("CARGO_PKG_VERSION")));
        let s = H(WinHttpOpen(PCWSTR(ua.as_ptr()), WINHTTP_ACCESS_TYPE_AUTOMATIC_PROXY, PCWSTR::null(), PCWSTR::null(), 0));
        if s.0.is_null() {
            return Err(err("WinHttpOpen"));
        }
        let _ = WinHttpSetTimeouts(s.0, 10_000, 10_000, 15_000, 30_000);
        let h = wide(host);
        let c = H(WinHttpConnect(s.0, PCWSTR(h.as_ptr()), INTERNET_DEFAULT_HTTPS_PORT, 0));
        if c.0.is_null() {
            return Err(err("conexão"));
        }
        let (verb, p) = (wide("GET"), wide(&path));
        let r = H(WinHttpOpenRequest(c.0, PCWSTR(verb.as_ptr()), PCWSTR(p.as_ptr()), PCWSTR::null(), PCWSTR::null(), std::ptr::null(), WINHTTP_FLAG_SECURE));
        if r.0.is_null() {
            return Err(err("pedido"));
        }
        let hd: Vec<u16> = "Accept: application/vnd.github+json, application/octet-stream\r\n".encode_utf16().collect();
        WinHttpSendRequest(r.0, Some(&hd), None, 0, 0, 0).map_err(|e| e.to_string())?;
        WinHttpReceiveResponse(r.0, std::ptr::null_mut()).map_err(|e| e.to_string())?;
        let mut code = 0u32;
        let mut len = 4u32;
        WinHttpQueryHeaders(r.0, WINHTTP_QUERY_STATUS_CODE | WINHTTP_QUERY_FLAG_NUMBER, PCWSTR::null(), Some(&mut code as *mut u32 as *mut _), &mut len, std::ptr::null_mut()).map_err(|e| e.to_string())?;
        if code != 200 {
            return Err(format!("HTTP {code}"));
        }
        let mut out = Vec::new();
        loop {
            let mut n = 0u32;
            WinHttpQueryDataAvailable(r.0, &mut n).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            let at = out.len();
            out.resize(at + n as usize, 0);
            let mut got = 0u32;
            WinHttpReadData(r.0, out[at..].as_mut_ptr() as *mut _, n, &mut got).map_err(|e| e.to_string())?;
            out.truncate(at + got as usize);
            if out.len() > 200 << 20 {
                return Err("resposta grande demais".into());
            }
        }
        Ok(out)
    }
}

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut it = s.trim().trim_start_matches(['v', 'V']).split(['.', '-', '+']).map(|x| x.parse::<u32>().ok());
    Some((it.next()??, it.next()??, it.next().flatten().unwrap_or(0)))
}

pub fn is_newer(tag: &str, current: &str) -> bool {
    matches!((parse_version(tag), parse_version(current)), (Some(a), Some(b)) if a > b)
}

fn parse_release(body: &[u8]) -> Result<Release, String> {
    let v: serde_json::Value = serde_json::from_slice(body).map_err(|e| e.to_string())?;
    let tag = v["tag_name"].as_str().ok_or("sem tag_name")?.to_string();
    let page = v["html_url"].as_str().unwrap_or_default().to_string();
    let a = v["assets"].as_array().and_then(|a| a.iter().find(|x| x["name"].as_str() == Some(ASSET))).ok_or("release sem AMA.exe")?;
    let exe_url = a["browser_download_url"].as_str().ok_or("sem link do AMA.exe")?.to_string();
    let size = a["size"].as_u64().unwrap_or(0);
    Ok(Release { tag, page, exe_url, size })
}

/// Release mais novo que a versão atual, se houver.
pub fn check() -> Result<Option<Release>, String> {
    let r = parse_release(&get(&format!("https://api.github.com/repos/{REPO}/releases/latest"))?)?;
    Ok(is_newer(&r.tag, env!("CARGO_PKG_VERSION")).then_some(r))
}

/// Baixa o exe novo e troca pelo atual. O antigo vai para `versoes_antigas\`. Retorna o caminho do exe.
pub fn install(r: &Release) -> Result<PathBuf, String> {
    let data = get(&r.exe_url)?;
    if data.len() < 1 << 20 || !data.starts_with(b"MZ") || (r.size > 0 && data.len() as u64 != r.size) {
        return Err("o arquivo baixado não parece o AMA.exe".into());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    let dir = exe.parent().ok_or("pasta do exe")?.to_path_buf();
    let new = dir.join("AMA.exe.novo");
    std::fs::write(&new, &data).map_err(|e| format!("{}: {e}", new.display()))?;
    let olds = dir.join("versoes_antigas");
    std::fs::create_dir_all(&olds).map_err(|e| e.to_string())?;
    let old = olds.join(format!("AMA-{}-{}.exe", env!("CARGO_PKG_VERSION"), super::presets::chrono_stamp()));
    // o Windows deixa renomear o exe em uso
    std::fs::rename(&exe, &old).map_err(|e| format!("mover o exe atual: {e}"))?;
    if let Err(e) = std::fs::rename(&new, &exe) {
        let _ = std::fs::rename(&old, &exe);
        return Err(format!("instalar: {e}"));
    }
    log!("atualizado para {}; versão anterior em {}", r.tag, old.display());
    Ok(exe)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[ignore]
    fn ao_vivo() {
        println!("{:?}", get(&format!("https://api.github.com/repos/{REPO}/releases/latest")).map(|b| String::from_utf8_lossy(&b).chars().take(200).collect::<String>()));
    }

    #[test]
    fn versoes() {
        assert!(is_newer("v0.2.0", "0.1.0"));
        assert!(is_newer("v1.0", "0.9.9"));
        assert!(!is_newer("v0.1.0", "0.1.0"));
        assert!(!is_newer("v0.0.9", "0.1.0"));
        assert!(!is_newer("nightly", "0.1.0"));
    }

    #[test]
    fn le_release() {
        let j = br#"{"tag_name":"v0.2.0","html_url":"https://github.com/x/y/releases/tag/v0.2.0","assets":[{"name":"AMA-v0.2.0-windows-x64.zip","browser_download_url":"https://a/zip","size":5},{"name":"AMA.exe","browser_download_url":"https://a/AMA.exe","size":123}]}"#;
        let r = parse_release(j).unwrap();
        assert_eq!((r.tag.as_str(), r.exe_url.as_str(), r.size), ("v0.2.0", "https://a/AMA.exe", 123));
    }
}
