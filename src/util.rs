//! Utilidades: normalización de texto, formateo, matching difuso.

use std::time::{Duration, Instant};

use unicode_normalization::UnicodeNormalization;

/// Convierte a minúsculas ASCII, quita acentos/diacríticos y colapsa espacios.
/// Se usa para comparar títulos/artistas entre addons.
pub fn fold_ascii(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for c in s.nfd() {
        if c.is_ascii() {
            let l = c.to_ascii_lowercase();
            match l {
                '\'' | '’' | '`' | '"' | '\u{0301}'..='\u{036f}' => {}
                _ => out.push(l),
            }
        }
    }
    // colapsar whitespace
    let mut res = String::with_capacity(out.len());
    let mut prev_space = false;
    for c in out.chars() {
        if c.is_whitespace() {
            if !prev_space && !res.is_empty() {
                res.push(' ');
            }
            prev_space = true;
        } else {
            res.push(c);
            prev_space = false;
        }
    }
    while res.ends_with(' ') {
        res.pop();
    }
    res
}

/// Quita ruido típico de títulos: "(Remasterizado)", "[Deluxe]", "feat. X", etc.
pub fn strip_title_noise(s: &str) -> String {
    let noise = [
        "remaster", "remastered", "remasterizado", "deluxe", "bonus track", "bonus",
        "explicit", "clean", "single version", "radio edit", "album version", "mono",
        "stereo", "live", "acoustic", "acapella", "a cappella", "instrumental",
        "extended", "mix", "edit", "version", "versión", "anniversary", "super deluxe",
    ];
    let folded = fold_ascii(s);
    let mut out = String::new();
    let bytes: Vec<char> = folded.chars().collect();
    let mut i = 0;
    let mut depth = 0i32;
    while i < bytes.len() {
        let c = bytes[i];
        match c {
            '(' | '[' | '{' => {
                depth += 1;
                // mirar si el contenido es ruido
                let mut j = i + 1;
                let mut lvl = depth;
                while j < bytes.len() && lvl > 0 {
                    match bytes[j] {
                        '(' | '[' | '{' => lvl += 1,
                        ')' | ']' | '}' => lvl -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                let inner: String = bytes[i + 1..(j.saturating_sub(1)).min(bytes.len())]
                    .iter()
                    .collect();
                let skip_paren = noise.iter().any(|n| inner.contains(n)) || inner.len() <= 4;
                if skip_paren {
                    i = j;
                    depth -= 1;
                    continue;
                } else {
                    out.push(c);
                }
            }
            ')' | ']' | '}' => {
                depth = (depth - 1).max(0);
                out.push(c);
            }
            _ => out.push(c),
        }
        i += 1;
    }
    let _ = depth;
    fold_ascii(&out)
}

/// Puntaje de coincidencia entre lo que se pidió y lo que devolvió el addon.
/// 1.0 = seguro; < 0.55 = probablemente no es el tema.
pub fn match_score(
    want_title: &str,
    want_artist: &str,
    want_isrc: Option<&str>,
    want_duration: Option<f64>,
    got_title: &str,
    got_artist: &str,
    got_isrc: Option<&str>,
    got_duration: Option<f64>,
) -> f64 {
    if let (Some(w), Some(g)) = (want_isrc, got_isrc) {
        if !w.is_empty() && w.eq_ignore_ascii_case(g) {
            return 1.0;
        }
    }
    let wt = strip_title_noise(want_title);
    let gt = strip_title_noise(got_title);
    let wa = fold_ascii(want_artist);
    let ga = fold_ascii(got_artist);

    let t_sim = similarity(&wt, &gt);
    let a_sim = if wa.is_empty() || ga.is_empty() {
        0.75
    } else if ga.contains(&wa) || wa.contains(&ga) {
        1.0
    } else {
        similarity(&wa, &ga)
    };

    let mut score = 0.55 * t_sim + 0.45 * a_sim;

    if let (Some(w), Some(g)) = (want_duration, got_duration) {
        if w > 0.0 && g > 0.0 {
            let diff = (w - g).abs();
            if diff <= 2.0 {
                score += 0.08;
            } else if diff > 20.0 {
                score -= 0.25;
            }
        }
    }
    score.clamp(0.0, 1.0)
}

/// Similaridad 0..1 barata (bigramas + prefijo), suficiente para ranking.
pub fn similarity(a: &str, b: &str) -> f64 {
    if a.is_empty() || b.is_empty() {
        return 0.0;
    }
    if a == b {
        return 1.0;
    }
    // prefijo común
    let common = a.chars().zip(b.chars()).take_while(|(x, y)| x == y).count() as f64;
    let max_len = a.chars().count().max(b.chars().count()) as f64;
    let prefix_bonus = common / max_len;

    let ba = bigrams(a);
    let bb = bigrams(b);
    if ba.is_empty() || bb.is_empty() {
        return prefix_bonus;
    }
    let mut shared = 0usize;
    for g in &ba {
        if bb.contains(g) {
            shared += 1;
        }
    }
    let dice = (2.0 * shared as f64) / (ba.len() + bb.len()) as f64;
    (0.75 * dice + 0.25 * prefix_bonus).clamp(0.0, 1.0)
}

fn bigrams(s: &str) -> Vec<[char; 2]> {
    let c: Vec<char> = s.chars().collect();
    if c.len() < 2 {
        return c.windows(1).map(|w| [w[0], ' ']).collect();
    }
    c.windows(2).map(|w| [w[0], w[1]]).collect()
}

/// "m:ss" / "h:mm:ss"
pub fn fmt_duration(secs: f64) -> String {
    if !secs.is_finite() || secs <= 0.0 {
        return "--:--".into();
    }
    let s = secs.round() as u64;
    let h = s / 3600;
    let m = (s % 3600) / 60;
    let sec = s % 60;
    if h > 0 {
        format!("{h}:{m:02}:{sec:02}")
    } else {
        format!("{m}:{sec:02}")
    }
}

pub fn fmt_duration_opt(secs: Option<f64>) -> String {
    match secs {
        Some(s) if s > 0.0 => fmt_duration(s),
        _ => String::new(),
    }
}

pub fn fmt_bytes(n: u64) -> String {
    const U: [&str; 5] = ["B", "KB", "MB", "GB", "TB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

pub fn fmt_latency(d: Duration) -> String {
    let us = d.as_micros();
    if us < 1000 {
        format!("{us}µs")
    } else if us < 1_000_000 {
        format!("{:.1}ms", us as f64 / 1000.0)
    } else {
        format!("{:.2}s", us as f64 / 1_000_000.0)
    }
}

/// Trunca un string por chars (no bytes) agregando "…".
pub fn ellipsize(s: &str, max: usize) -> String {
    let c: Vec<char> = s.chars().collect();
    if c.len() <= max {
        s.to_string()
    } else {
        let mut out: String = c.into_iter().take(max.saturating_sub(1)).collect();
        out.push('…');
        out
    }
}

/// Reloj monotónico simple para timeouts de UI (debounce, etc.)
#[derive(Debug, Clone)]
pub struct Deadline {
    at: Instant,
}

impl Deadline {
    pub fn after(d: Duration) -> Self {
        Self {
            at: Instant::now() + d,
        }
    }
    pub fn expired(&self) -> bool {
        Instant::now() >= self.at
    }
    pub fn remaining(&self) -> Duration {
        self.at.saturating_duration_since(Instant::now())
    }
}

/// Normaliza la URL de un addon a su "base" (sin `/manifest.json`).
/// Acepta:
///   https://host/token/manifest.json
///   https://host/token
///   https://host/token/
///   host/token
pub fn normalize_addon_base(input: &str) -> anyhow::Result<String> {
    let mut s = input.trim().to_string();
    if s.is_empty() {
        anyhow::bail!("URL vacía");
    }
    if !s.contains("://") {
        s = format!("https://{s}");
    }
    // quitar query/fragment
    if let Some(i) = s.find(['?', '#']) {
        s.truncate(i);
    }
    while s.ends_with('/') {
        s.pop();
    }
    let lower = s.to_ascii_lowercase();
    for suffix in ["/manifest.json", "/manifest"] {
        if lower.ends_with(suffix) {
            s.truncate(s.len() - suffix.len());
            break;
        }
    }
    if s.ends_with("://") || s.chars().filter(|c| *c == '/').count() < 2 {
        anyhow::bail!("URL de addon inválida: {input}");
    }
    Ok(s)
}

/// Une base + path + query sin duplicar barras.
pub fn join_url(base: &str, path: &str) -> String {
    let b = base.trim_end_matches('/');
    let p = if path.starts_with('/') {
        path.to_string()
    } else {
        format!("/{path}")
    };
    format!("{b}{p}")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn folds() {
        assert_eq!(fold_ascii("  Árbol  Niño  "), "arbol nino");
        assert_eq!(fold_ascii("Daft Punk"), "daft punk");
    }

    #[test]
    fn normalizes_addon_urls() {
        let b = "https://tido.tido.tido.dpdns.org/479f187c279e42819675c81a973529c1";
        assert_eq!(
            normalize_addon_base(&format!("{b}/manifest.json")).unwrap(),
            b
        );
        assert_eq!(normalize_addon_base(&format!("{b}/")).unwrap(), b);
        assert_eq!(normalize_addon_base(b).unwrap(), b);
        assert!(normalize_addon_base("https://host").is_err());
    }

    #[test]
    fn duration_seconds_vs_ms() {
        assert_eq!(crate::models::seconds_from(240.0), 240.0);
        assert_eq!(crate::models::seconds_from(240000.0), 240.0);
    }
}
