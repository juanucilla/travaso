//! Travaso: un server MCP che travasa una memoria da un'intelligenza artificiale all'altra
//! e la condivide tra più IA. Riscrittura in Rust della versione Python (stesso formato dati,
//! stessi strumenti, stesso comportamento).
//!
//!   1. CONSERVA  al primo avvio fotografa la memoria in archivio/, che non viene mai modificato.
//!   2. SCARICA   ogni consultazione consegna fatti nuovi; travaso_assorbi li toglie dal serbatoio.
//!   3. SVUOTA    quando tutto è assorbito il serbatoio è a 0%. L'archivio resta.
//!
//! MEMORIA CONDIVISA: condivisa.jsonl, registro solo-aggiunte che non si svuota mai, letto e
//! scritto da più IA (Cursor, Codex…). Ogni fatto porta la sua fonte.
//!
//! Uso:
//!   travaso                      stdio (MCP)
//!   travaso --http 8765          http://127.0.0.1:8765/mcp
//!   travaso --importa CARTELLA   carica i .md nel serbatoio e ne fa l'archivio
//!   travaso --stato              livello di serbatoio e condivisa
//!   travaso --versa-tutto        versa tutto il serbatoio nella memoria condivisa
//!
//! Variabili: TRAVASO_DIR, TRAVASO_CLIENT, TRAVASO_MODALITA (completa|condivisa|travaso),
//! TRAVASO_ORIGINE (predefinita "claude"), TRAVASO_AUTO=1.

use indexmap::IndexMap;
use regex::Regex;
use serde_json::{json, Map, Value};
use std::collections::{HashMap, HashSet};
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use unicode_normalization::char::is_combining_mark;
use unicode_normalization::UnicodeNormalization;

const VERSION: &str = env!("CARGO_PKG_VERSION");
const NAME: &str = "travaso";

// ------------------------------------------------------------------ utilità
fn stopwords() -> &'static HashSet<&'static str> {
    static S: OnceLock<HashSet<&'static str>> = OnceLock::new();
    S.get_or_init(|| {
        "a ad al alla alle agli ai all anche che chi con da dal dalla dei del della delle dello di e ed gli ha ho i il in \
         la le lo ma mi ne nei nel nella non o per piu più poi quale quali se si sia sono su sul sulla tra un una uno \
         the of and or to in on for with is are was be by at as from it its an this that not no"
            .split_whitespace()
            .collect()
    })
}

fn re(pat: &'static str, cell: &'static OnceLock<Regex>) -> &'static Regex {
    cell.get_or_init(|| Regex::new(pat).expect("regex valida"))
}

fn secret_re() -> &'static Regex {
    static R: OnceLock<Regex> = OnceLock::new();
    re(r"(?i)(AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY|sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}|gho_[A-Za-z0-9]{20,}|xox[abp]-|password\s*[:=]|passwd\s*[:=]|api[_-]?key\s*[:=]|secret\s*[:=]|token\s*[:=])", &R)
}

fn norm(s: &str) -> String {
    s.nfkd().filter(|c| !is_combining_mark(*c)).collect::<String>().to_lowercase()
}

fn tokens(s: &str) -> Vec<String> {
    static R: OnceLock<Regex> = OnceLock::new();
    let r = re(r"\w[\w\-\./@]*\w|\w", &R);
    let n = norm(s);
    r.find_iter(&n).map(|m| m.as_str().to_string()).filter(|t| !stopwords().contains(t.as_str())).collect()
}

fn squash(s: &str) -> String {
    s.split_whitespace().collect::<Vec<_>>().join(" ")
}

fn now() -> String {
    chrono::Local::now().to_rfc3339_opts(chrono::SecondsFormat::Secs, false)
}

fn sha10(s: &str) -> String {
    sha1_smol::Sha1::from(s).digest().to_string()[..10].to_string()
}

/// (metadati, corpo, numero di righe del frontmatter)
fn frontmatter(text: &str) -> (HashMap<String, String>, String, usize) {
    let mut meta = HashMap::new();
    if text.starts_with("---") {
        let lines: Vec<&str> = text.split('\n').collect();
        for i in 1..lines.len() {
            if lines[i].trim() == "---" {
                for ln in &lines[1..i] {
                    if let Some((k, v)) = ln.split_once(':') {
                        meta.insert(k.trim().to_string(), v.trim().to_string());
                    }
                }
                return (meta, lines[i + 1..].join("\n"), i + 1);
            }
        }
    }
    (meta, text.to_string(), 0)
}

fn md_files(base: &Path) -> Vec<(String, PathBuf)> {
    fn walk(dir: &Path, base: &Path, out: &mut Vec<(String, PathBuf)>) {
        if let Ok(rd) = fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, base, out);
                } else if p.extension().map(|x| x == "md").unwrap_or(false) {
                    let rel = p.strip_prefix(base).unwrap_or(&p).components()
                        .map(|c| c.as_os_str().to_string_lossy().into_owned()).collect::<Vec<_>>().join("/");
                    out.push((rel, p));
                }
            }
        }
    }
    let mut out = Vec::new();
    walk(base, base, &mut out);
    out.sort_by(|a, b| a.0.cmp(&b.0));
    out
}

fn copy_dir(src: &Path, dst: &Path) -> io::Result<()> {
    fs::create_dir_all(dst)?;
    for e in fs::read_dir(src)? {
        let e = e?;
        let to = dst.join(e.file_name());
        if e.path().is_dir() { copy_dir(&e.path(), &to)?; } else { fs::copy(e.path(), to)?; }
    }
    Ok(())
}

fn is_upper(s: &str) -> bool {
    let mut cased = false;
    for c in s.chars() {
        if c.is_lowercase() { return false; }
        if c.is_uppercase() { cased = true; }
    }
    cased
}

/// Lock esclusivo tra processi: più IA avviano ciascuna il proprio server sugli stessi file.
struct FileLock(File);
impl FileLock {
    fn new(path: &Path) -> io::Result<Self> {
        let f = OpenOptions::new().create(true).append(true).open(path)?;
        f.lock()?;
        Ok(FileLock(f))
    }
}
impl Drop for FileLock {
    fn drop(&mut self) { let _ = self.0.unlock(); }
}

fn err(msg: impl Into<String>) -> Value { json!({ "error": msg.into() }) }

// ------------------------------------------------------------------ serbatoio (travaso)
#[derive(Clone)]
struct Fact { id: String, file: String, section: String, line: String, text: String, tok: Vec<String>, hdr: Vec<String> }

struct Scan { files: Vec<Value>, facts: Vec<Fact> }

fn scan(base: &Path) -> Scan {
    let mut files = Vec::new();
    let mut facts = Vec::new();
    static ALIAS: OnceLock<Regex> = OnceLock::new();
    static BULLET: OnceLock<Regex> = OnceLock::new();
    let alias_re = re(r"[^\[\],]+", &ALIAS);
    let bullet_re = re(r"^[-*]\s+", &BULLET);
    for (rel, path) in md_files(base) {
        let raw = String::from_utf8_lossy(&fs::read(&path).unwrap_or_default()).into_owned();
        let (meta, body, _) = frontmatter(&raw);
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        let name = meta.get("name").cloned().unwrap_or(stem);
        let aliases: Vec<String> = alias_re.find_iter(meta.get("aliases").map(String::as_str).unwrap_or(""))
            .map(|m| m.as_str().trim().to_string()).filter(|a| !a.is_empty()).collect();
        let mut count = 0usize;
        let mut section = String::new();
        let mut seen: HashMap<String, usize> = HashMap::new();
        for line in body.split('\n') {
            let s = line.trim();
            if s.is_empty() { continue; }
            if s.starts_with('#') { section = s.trim_start_matches('#').trim().to_string(); continue; }
            let h = sha10(&format!("{rel}\n{s}"));
            let n = seen.entry(h.clone()).or_insert(0);
            *n += 1;
            let id = if *n == 1 { h } else { format!("{h}-{n}") };
            let header = format!("{} {} {}", name, aliases.join(" "), section);
            facts.push(Fact { id, file: rel.clone(), section: section.clone(), line: s.to_string(),
                text: bullet_re.replace(s, "").into_owned(), tok: tokens(s), hdr: tokens(&header) });
            count += 1;
        }
        files.push(json!({ "file": rel, "name": name, "description": meta.get("description").cloned().unwrap_or_default(),
                           "aliases": aliases, "facts": count }));
    }
    Scan { files, facts }
}

struct Travaso {
    root: PathBuf, tank: PathBuf, archive: PathBuf, log: PathBuf,
    auto: bool, origin: String,
    in_transit: HashSet<String>,
    files: Vec<Value>, facts: Vec<Fact>, by_id: HashMap<String, usize>, idf: HashMap<String, f64>,
}

impl Travaso {
    fn new(root: &Path, auto: bool) -> Self {
        let tank = root.join("serbatoio");
        let archive = root.join("archivio");
        let _ = fs::create_dir_all(&tank);
        if !archive.exists() && !md_files(&tank).is_empty() {
            let _ = copy_dir(&tank, &archive); // 1. CONSERVA
        }
        let mut t = Travaso { root: root.to_path_buf(), tank, archive, log: root.join("travasato.jsonl"), auto,
            origin: std::env::var("TRAVASO_ORIGINE").unwrap_or_else(|_| "claude".into()),
            in_transit: HashSet::new(), files: vec![], facts: vec![], by_id: HashMap::new(), idf: HashMap::new() };
        t.reload();
        t
    }

    fn reload(&mut self) {
        let sc = scan(&self.tank);
        self.files = sc.files;
        self.facts = sc.facts;
        self.by_id = self.facts.iter().enumerate().map(|(i, f)| (f.id.clone(), i)).collect();
        let ids: HashSet<String> = self.by_id.keys().cloned().collect();
        self.in_transit.retain(|i| ids.contains(i));
        let corpus = if self.facts.is_empty() { scan(&self.archive).facts } else { self.facts.clone() };
        let mut df: HashMap<String, usize> = HashMap::new();
        for f in &corpus {
            let set: HashSet<&String> = f.tok.iter().chain(f.hdr.iter()).collect();
            for t in set { *df.entry(t.clone()).or_insert(0) += 1; }
        }
        let n = corpus.len().max(1) as f64;
        self.idf = df.into_iter().map(|(t, d)| (t, (1.0 + n / d as f64).ln())).collect();
    }

    fn score(&self, f: &Fact, q: &[String]) -> f64 {
        let body: HashSet<&String> = f.tok.iter().collect();
        let hdr: HashSet<&String> = f.hdr.iter().collect();
        let mut score = 0.0;
        for t in q {
            let w = *self.idf.get(t).unwrap_or(&1.0);
            if body.contains(t) {
                score += w;
            } else if t.chars().count() > 3 && body.iter().any(|b| b.chars().count() > 3 && (b.starts_with(t.as_str()) || t.starts_with(b.as_str()))) {
                score += 0.6 * w;
            }
            if hdr.contains(t) { score += 0.5 * w; }
        }
        score
    }

    fn suggest(results: &[Value], q: &[String]) -> Vec<String> {
        static R: OnceLock<Regex> = OnceLock::new();
        let r = re(r"([A-Z][^\W\d_][\w&\-]*(?:\s+[A-Z][^\W\d_][\w&\-]*)?|[A-Z]{2,}[\w\-]*)", &R);
        let mut cand: Vec<(String, usize)> = Vec::new();
        for res in results {
            let text = res["text"].as_str().unwrap_or("");
            for m in r.find_iter(text) {
                let start = m.start();
                let prev = text[..start].chars().last();
                if let Some(c) = prev { if c.is_alphanumeric() || c == '_' || c == '&' || c == '-' { continue; } }
                let term = m.as_str();
                let before = text[..start].trim_end();
                if (start == 0 || before.ends_with(['.', ':', ';', '(', '—'])) && !is_upper(term) && !term.contains(' ') { continue; }
                if term.chars().count() < 3 || tokens(term).iter().all(|t| q.contains(t)) { continue; }
                match cand.iter_mut().find(|(k, _)| k == term) {
                    Some(e) => e.1 += 1,
                    None => cand.push((term.to_string(), 1)),
                }
            }
        }
        cand.sort_by(|a, b| b.1.cmp(&a.1));
        cand.into_iter().take(8).map(|(k, _)| k).collect()
    }

    fn deliver(&mut self, picked: Vec<Fact>, shared: Option<&mut Condivisa>) -> Vec<Value> {
        let out: Vec<Value> = picked.iter().map(|f| json!({"id": f.id, "file": f.file, "section": f.section, "text": f.text})).collect();
        if self.auto {
            let ids: Vec<String> = picked.iter().map(|f| f.id.clone()).collect();
            self.absorb(&ids, "auto", shared);
        } else {
            for f in &picked { self.in_transit.insert(f.id.clone()); }
        }
        out
    }

    fn status(&mut self) -> Value {
        self.reload(); // un'altra IA può aver svuotato il serbatoio
        let total_archive = scan(&self.archive).facts.len();
        let left = self.facts.len();
        let level = if total_archive > 0 { (1000.0 * left as f64 / total_archive as f64).round() / 10.0 } else { 0.0 };
        let absorbed = fs::read_to_string(&self.log).map(|s| s.lines().filter(|l| !l.trim().is_empty()).count()).unwrap_or(0);
        let instr = if left == 0 {
            "Il travaso è finito: il serbatoio è vuoto. L'archivio resta consultabile con travaso_archivio."
        } else {
            "Consulta con travaso_cerca (rilancia con next_queries) o travaso_prossimi. Salva ogni fatto ricevuto nella TUA memoria permanente, poi chiama travaso_assorbi con i loro id: solo allora escono dal serbatoio. Continua finché vuoto=true."
        };
        json!({ "serbatoio_percento": level, "fatti_nel_serbatoio": left, "fatti_in_viaggio": self.in_transit.len(),
                "fatti_assorbiti_totali": absorbed, "fatti_in_archivio": total_archive, "vuoto": left == 0,
                "file": self.files, "istruzioni": instr })
    }

    fn search(&mut self, query: &str, limit: i64, shared: Option<&mut Condivisa>) -> Value {
        self.reload();
        let q = tokens(query);
        if q.is_empty() { return err("query vuota"); }
        let mut hits: Vec<(f64, usize)> = self.facts.iter().enumerate().map(|(i, f)| (self.score(f, &q), i)).collect();
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let fresh: Vec<Fact> = hits.iter().filter(|(s, i)| *s > 0.0 && !self.in_transit.contains(&self.facts[*i].id))
            .map(|(_, i)| self.facts[*i].clone()).collect();
        let lim = limit.clamp(1, 50) as usize;
        let remaining = fresh.len().saturating_sub(lim);
        let page: Vec<Fact> = fresh.into_iter().take(lim).collect();
        let results = self.deliver(page, shared);
        let nq = Self::suggest(&results, &q);
        let da: Vec<Value> = if self.auto { vec![] } else { results.iter().map(|r| r["id"].clone()).collect() };
        let level = self.status()["serbatoio_percento"].clone();
        json!({ "query": query, "results": results, "remaining_matches": remaining, "exhausted_for_query": remaining == 0,
                "next_queries": nq, "da_assorbire": da, "serbatoio_percento": level })
    }

    fn next(&mut self, limit: i64, file: Option<&str>, shared: Option<&mut Condivisa>) -> Value {
        self.reload();
        let pool: Vec<Fact> = self.facts.iter().filter(|f| !self.in_transit.contains(&f.id) && file.map(|x| f.file == x).unwrap_or(true)).cloned().collect();
        let lim = limit.clamp(1, 100) as usize;
        let remaining = pool.len().saturating_sub(lim);
        let page: Vec<Fact> = pool.into_iter().take(lim).collect();
        let results = self.deliver(page, shared);
        let da: Vec<Value> = if self.auto { vec![] } else { results.iter().map(|r| r["id"].clone()).collect() };
        json!({ "results": results, "remaining": remaining, "exhausted": remaining == 0, "da_assorbire": da })
    }

    /// 2. SCARICA: i fatti confermati escono dai file del serbatoio, finiscono nel registro e nella condivisa.
    fn absorb(&mut self, ids: &[String], where_: &str, mut shared: Option<&mut Condivisa>) -> Value {
        let _lock = FileLock::new(&self.root.join("serbatoio.lock"));
        self.reload();
        let mut uniq: Vec<String> = Vec::new();
        for i in ids { if self.by_id.contains_key(i) && !uniq.contains(i) { uniq.push(i.clone()); } }
        if uniq.is_empty() { return json!({"assorbiti": 0, "nota": "nessun id valido (forse già assorbiti)"}); }
        let mut by_file: IndexMap<String, Vec<Fact>> = IndexMap::new();
        for i in &uniq { let f = self.facts[self.by_id[i]].clone(); by_file.entry(f.file.clone()).or_default().push(f); }
        let stamp = now();
        let mut log = match OpenOptions::new().create(true).append(true).open(&self.log) { Ok(f) => f, Err(e) => return err(e.to_string()) };
        for (rel, items) in &by_file {
            let path = self.tank.join(rel);
            let raw = fs::read_to_string(&path).unwrap_or_default();
            let mut lines: Vec<String> = raw.split('\n').map(String::from).collect();
            let (_, _, fm) = frontmatter(&raw);
            for it in items {
                if let Some(k) = (fm..lines.len()).find(|&k| lines[k].trim() == it.line) { lines.remove(k); }
                let _ = writeln!(log, "{}", json!({"id": it.id, "file": rel, "text": it.text, "assorbito_da": where_, "quando": stamp}));
                if let Some(s) = shared.as_deref_mut() {
                    let stem = Path::new(rel).file_stem().map(|x| x.to_string_lossy().into_owned()).unwrap_or_default();
                    let topic = if it.section.is_empty() { stem } else { format!("{stem} · {}", it.section) };
                    s.remember(&it.text, &topic, Some(&self.origin));
                }
            }
            let joined = lines.join("\n");
            let (_, body, _) = frontmatter(&joined);
            if !body.split('\n').any(|l| !l.trim().is_empty() && !l.trim().starts_with('#')) {
                let _ = fs::remove_file(&path); // 3. SVUOTA
            } else {
                let _ = fs::write(&path, joined);
            }
        }
        for i in &uniq { self.in_transit.remove(i); }
        let st = self.status();
        json!({ "assorbiti": uniq.len(), "serbatoio_percento": st["serbatoio_percento"],
                "fatti_nel_serbatoio": st["fatti_nel_serbatoio"], "vuoto": st["vuoto"] })
    }

    fn archive_search(&self, query: &str, limit: i64) -> Value {
        let q = tokens(query);
        let facts = scan(&self.archive).facts;
        let mut hits: Vec<(f64, &Fact)> = facts.iter().map(|f| (self.score(f, &q), f)).collect();
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let res: Vec<Value> = hits.into_iter().filter(|(s, _)| *s > 0.0).take(limit.clamp(1, 50) as usize)
            .map(|(_, f)| json!({"file": f.file, "section": f.section, "text": f.text})).collect();
        json!({ "query": query, "results": res })
    }

    fn restore(&mut self, confirm: bool) -> Value {
        if !confirm { return err("passa conferma=true: il serbatoio verrà riempito di nuovo dall'archivio"); }
        if !self.archive.exists() { return err("nessun archivio"); }
        let _ = fs::remove_dir_all(&self.tank);
        if let Err(e) = copy_dir(&self.archive, &self.tank) { return err(e.to_string()); }
        self.in_transit.clear();
        self.reload();
        json!({ "ok": true, "fatti_nel_serbatoio": self.facts.len() })
    }

    fn import_dir(&mut self, src: &Path) -> Value {
        let mut n = 0;
        for (rel, p) in md_files(src) {
            let text = String::from_utf8_lossy(&fs::read(&p).unwrap_or_default()).into_owned();
            if secret_re().is_match(&text) { eprintln!("saltato (sembra contenere segreti): {}", p.display()); continue; }
            let dst = self.tank.join(&rel);
            if let Some(par) = dst.parent() { let _ = fs::create_dir_all(par); }
            let _ = fs::write(&dst, text);
            n += 1;
        }
        let _ = fs::remove_dir_all(&self.archive);
        let _ = copy_dir(&self.tank, &self.archive);
        self.reload();
        json!({ "file_importati": n, "fatti": self.facts.len() })
    }

    fn pour_all(&mut self, shared: Option<&mut Condivisa>) -> Value {
        if shared.is_none() { return err("memoria condivisa non attiva"); }
        self.reload();
        let ids: Vec<String> = self.facts.iter().map(|f| f.id.clone()).collect();
        if ids.is_empty() { return json!({"assorbiti": 0, "vuoto": true}); }
        self.absorb(&ids, "memoria condivisa", shared)
    }
}

// ------------------------------------------------------------------ memoria condivisa
#[derive(Clone)]
struct SFact { id: String, testo: String, argomento: String, fonte: String, aggiornato: String }

struct Condivisa { file: PathBuf, lockfile: PathBuf, client: String, seen: HashSet<String> }

impl Condivisa {
    fn new(root: &Path, client: &str) -> Self {
        let _ = fs::create_dir_all(root);
        let c: String = client.to_lowercase().chars().filter(|c| c.is_alphanumeric() || *c == '_' || *c == '-').take(32).collect();
        Condivisa { file: root.join("condivisa.jsonl"), lockfile: root.join("condivisa.lock"),
                    client: if c.is_empty() { "ia".into() } else { c }, seen: HashSet::new() }
    }

    fn load(&self) -> IndexMap<String, SFact> {
        let mut facts: IndexMap<String, SFact> = IndexMap::new();
        let Ok(f) = File::open(&self.file) else { return facts };
        for ln in BufReader::new(f).lines().map_while(Result::ok) {
            let Ok(ev) = serde_json::from_str::<Value>(&ln) else { continue };
            let id = ev["id"].as_str().unwrap_or("").to_string();
            let s = |k: &str| ev[k].as_str().unwrap_or("").to_string();
            match ev["op"].as_str() {
                Some("add") if !facts.contains_key(&id) => {
                    facts.insert(id.clone(), SFact { id, testo: s("testo"), argomento: s("argomento"), fonte: s("fonte"), aggiornato: s("quando") });
                }
                Some("edit") => if let Some(f) = facts.get_mut(&id) { f.testo = s("testo"); f.aggiornato = s("quando"); },
                Some("del") => { facts.shift_remove(&id); }
                _ => {}
            }
        }
        facts
    }

    fn append(&self, ev: Value) -> io::Result<()> {
        let mut f = OpenOptions::new().create(true).append(true).open(&self.file)?;
        writeln!(f, "{ev}")
    }

    fn score(f: &SFact, q: &[String]) -> f64 {
        let body: HashSet<String> = tokens(&f.testo).into_iter().collect();
        let hdr: HashSet<String> = tokens(&format!("{} {}", f.argomento, f.fonte)).into_iter().collect();
        let mut score = 0.0;
        for t in q {
            if body.contains(t) { score += 1.0; }
            else if t.chars().count() > 3 && body.iter().any(|b| b.chars().count() > 3 && (b.starts_with(t.as_str()) || t.starts_with(b.as_str()))) { score += 0.6; }
            if hdr.contains(t) { score += 0.5; }
        }
        score
    }

    fn public(f: &SFact) -> Value {
        json!({"id": f.id, "testo": f.testo, "argomento": f.argomento, "fonte": f.fonte, "aggiornato": f.aggiornato})
    }

    fn remember(&mut self, text: &str, topic: &str, source: Option<&str>) -> Value {
        let text = squash(text);
        if text.is_empty() { return err("testo vuoto"); }
        if secret_re().is_match(&text) { return err("rifiutato: sembra contenere un segreto (password, chiave o token)"); }
        let source = source.unwrap_or(&self.client).to_string();
        let _lock = FileLock::new(&self.lockfile);
        let key = norm(&text);
        if let Some(f) = self.load().values().find(|f| norm(&f.testo) == key) {
            return json!({"ok": true, "id": f.id, "nota": format!("già presente (fonte: {})", f.fonte)});
        }
        let id = sha10(&format!("{key}\n{}\n{}", now(), std::process::id()));
        if let Err(e) = self.append(json!({"op": "add", "id": id, "testo": text, "argomento": squash(topic), "fonte": source, "quando": now()})) {
            return err(e.to_string());
        }
        json!({"ok": true, "id": id, "fonte": source})
    }

    fn search(&mut self, query: &str, limit: i64, sources: Option<Vec<String>>, exclude_mine: bool, include_seen: bool) -> Value {
        let q = tokens(query);
        let mut facts: Vec<SFact> = self.load().into_values().collect();
        if let Some(src) = sources.filter(|s| !s.is_empty()) { facts.retain(|f| src.contains(&f.fonte)); }
        if exclude_mine { facts.retain(|f| f.fonte != self.client); }
        let mut hits: Vec<(f64, SFact)> = if q.is_empty() { vec![] } else { facts.into_iter().map(|f| (Self::score(&f, &q), f)).collect() };
        hits.sort_by(|a, b| b.0.partial_cmp(&a.0).unwrap_or(std::cmp::Ordering::Equal));
        let fresh: Vec<SFact> = hits.into_iter().filter(|(s, f)| *s > 0.0 && (include_seen || !self.seen.contains(&f.id))).map(|(_, f)| f).collect();
        let lim = limit.clamp(1, 50) as usize;
        let remaining = fresh.len().saturating_sub(lim);
        let page: Vec<SFact> = fresh.into_iter().take(lim).collect();
        for f in &page { self.seen.insert(f.id.clone()); }
        json!({"query": query, "results": page.iter().map(Self::public).collect::<Vec<_>>(),
               "remaining_matches": remaining, "exhausted_for_query": remaining == 0})
    }

    fn list(&self, topic: Option<&str>, source: Option<&str>, limit: i64, offset: i64) -> Value {
        let mut facts: Vec<SFact> = self.load().into_values().collect();
        facts.sort_by(|a, b| b.aggiornato.cmp(&a.aggiornato));
        if let Some(t) = topic.filter(|t| !t.is_empty()) { let t = norm(t); facts.retain(|f| norm(&f.argomento).contains(&t)); }
        if let Some(s) = source.filter(|s| !s.is_empty()) { facts.retain(|f| f.fonte == s); }
        let off = offset.max(0) as usize;
        let page: Vec<Value> = facts.iter().skip(off).take(limit.clamp(1, 100) as usize).map(Self::public).collect();
        let next = if off + page.len() < facts.len() { json!(off + page.len()) } else { Value::Null };
        json!({"totale": facts.len(), "results": page, "prossimo_offset": next})
    }

    fn edit(&self, id: &str, text: &str) -> Value {
        let text = squash(text);
        if text.is_empty() { return err("testo vuoto"); }
        if secret_re().is_match(&text) { return err("rifiutato: sembra contenere un segreto"); }
        let _lock = FileLock::new(&self.lockfile);
        if !self.load().contains_key(id) { return err(format!("id non trovato: {id}")); }
        match self.append(json!({"op": "edit", "id": id, "testo": text, "fonte": self.client, "quando": now()})) {
            Ok(_) => json!({"ok": true, "id": id}), Err(e) => err(e.to_string()),
        }
    }

    fn forget(&self, id: &str) -> Value {
        let _lock = FileLock::new(&self.lockfile);
        if !self.load().contains_key(id) { return err(format!("id non trovato: {id}")); }
        match self.append(json!({"op": "del", "id": id, "fonte": self.client, "quando": now()})) {
            Ok(_) => json!({"ok": true, "id": id}), Err(e) => err(e.to_string()),
        }
    }

    fn status(&self) -> Value {
        let facts = self.load();
        let mut by: IndexMap<String, usize> = IndexMap::new();
        for f in facts.values() { *by.entry(f.fonte.clone()).or_insert(0) += 1; }
        json!({"fatti": facts.len(), "per_fonte": by, "io_sono": self.client,
               "file": self.file.display().to_string(), "letti_in_questa_sessione": self.seen.len()})
    }
}

// ------------------------------------------------------------------ MCP
struct App { t: Option<Travaso>, s: Option<Condivisa> }

fn travaso_tools() -> Value {
    json!([
      {"name":"travaso_stato","description":"Livello del serbatoio di memoria da travasare: quanti fatti restano, quanti sono stati assorbiti, file e istruzioni. Chiamalo per primo.","inputSchema":{"type":"object","properties":{}}},
      {"name":"travaso_cerca","description":"Cerca nel serbatoio e consegna solo fatti non ancora consegnati, con i termini da cercare dopo (next_queries). Salva i fatti nella tua memoria permanente e poi chiama travaso_assorbi.","inputSchema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"},"limit":{"type":"integer","default":8}}}},
      {"name":"travaso_prossimi","description":"Consegna in ordine i prossimi fatti del serbatoio, per svuotarlo tutto. Ripeti finché exhausted=true, assorbendo dopo ogni blocco.","inputSchema":{"type":"object","properties":{"limit":{"type":"integer","default":15},"file":{"type":"string"}}}},
      {"name":"travaso_assorbi","description":"Conferma che hai salvato questi fatti nella tua memoria: escono dal serbatoio per sempre (restano solo in archivio).","inputSchema":{"type":"object","required":["ids"],"properties":{"ids":{"type":"array","items":{"type":"string"}},"dove":{"type":"string","description":"dove li hai salvati, es. 'memoria ChatGPT'"}}}},
      {"name":"travaso_archivio","description":"Cerca nell'archivio originale, che non si svuota mai. Da usare solo per ricordare qualcosa dopo il travaso.","inputSchema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"},"limit":{"type":"integer","default":8}}}},
      {"name":"travaso_ripristina","description":"Riempie di nuovo il serbatoio dall'archivio. Solo su richiesta esplicita dell'utente.","inputSchema":{"type":"object","required":["conferma"],"properties":{"conferma":{"type":"boolean"}}}}
    ])
}

fn shared_tools() -> Value {
    json!([
      {"name":"memoria_stato","description":"Memoria CONDIVISA tra più IA (Cursor, Codex, Claude…): quanti fatti ci sono, per fonte, e chi sei tu. Non si svuota mai.","inputSchema":{"type":"object","properties":{}}},
      {"name":"memoria_cerca","description":"Cerca nella memoria condivisa (fatti scritti da tutte le IA, ognuno con la sua fonte). Restituisce solo fatti non ancora letti in questa sessione: rilancia finché exhausted_for_query=true. Usalo prima di rispondere su utente e progetti.","inputSchema":{"type":"object","required":["query"],"properties":{"query":{"type":"string"},"limit":{"type":"integer","default":8},"fonti":{"type":"array","items":{"type":"string"},"description":"es. [\"claude\",\"codex\"]"},"escludi_mie":{"type":"boolean","default":false},"include_seen":{"type":"boolean","default":false}}}},
      {"name":"memoria_elenco","description":"Elenca i fatti della memoria condivisa, dai più recenti, filtrando per argomento o fonte.","inputSchema":{"type":"object","properties":{"argomento":{"type":"string"},"fonte":{"type":"string"},"limit":{"type":"integer","default":30},"offset":{"type":"integer","default":0}}}},
      {"name":"memoria_ricorda","description":"Aggiunge alla memoria condivisa un fatto durevole detto dall'utente o deciso nel lavoro. Le altre IA lo vedranno. Mai segreti o dati sanitari.","inputSchema":{"type":"object","required":["testo"],"properties":{"testo":{"type":"string"},"argomento":{"type":"string"}}}},
      {"name":"memoria_correggi","description":"Aggiorna il testo di un fatto della memoria condivisa (resta traccia di chi l'ha modificato).","inputSchema":{"type":"object","required":["id","testo"],"properties":{"id":{"type":"string"},"testo":{"type":"string"}}}},
      {"name":"memoria_dimentica","description":"Toglie un fatto dalla memoria condivisa. Solo se l'utente lo chiede o il fatto è sbagliato.","inputSchema":{"type":"object","required":["id"],"properties":{"id":{"type":"string"}}}}
    ])
}

const INSTRUCTIONS: &str = "Travaso: memoria da trasferire nella tua. Procedura: travaso_stato → travaso_cerca/travaso_prossimi → salva i fatti nella tua memoria permanente → travaso_assorbi con i loro id → ripeti finché vuoto=true.";
const SHARED_INSTRUCTIONS: &str = "Memoria condivisa tra più IA: consultala con memoria_cerca prima di rispondere su utente e progetti (rilancia finché exhausted_for_query=true); salva con memoria_ricorda i fatti nuovi e durevoli. Ogni fatto porta la sua fonte. Non si svuota mai.";

fn arg_str<'a>(a: &'a Map<String, Value>, k: &str) -> Result<&'a str, String> {
    a.get(k).and_then(Value::as_str).ok_or_else(|| format!("errore: manca '{k}'"))
}
fn arg_opt<'a>(a: &'a Map<String, Value>, k: &str) -> Option<&'a str> { a.get(k).and_then(Value::as_str) }
fn arg_int(a: &Map<String, Value>, k: &str, d: i64) -> i64 {
    a.get(k).and_then(|v| v.as_i64().or_else(|| v.as_f64().map(|f| f as i64)).or_else(|| v.as_str().and_then(|s| s.parse().ok()))).unwrap_or(d)
}
fn arg_bool(a: &Map<String, Value>, k: &str) -> bool { a.get(k).and_then(Value::as_bool).unwrap_or(false) }
fn arg_list(a: &Map<String, Value>, k: &str) -> Vec<String> {
    a.get(k).and_then(Value::as_array).map(|v| v.iter().filter_map(|x| x.as_str().map(String::from)).collect()).unwrap_or_default()
}

fn call_tool(app: &mut App, name: &str, a: &Map<String, Value>) -> Option<Result<Value, String>> {
    if name.starts_with("memoria_") {
        let s = app.s.as_mut()?;
        return Some((|| -> Result<Value, String> {
            Ok(match name {
                "memoria_stato" => s.status(),
                "memoria_cerca" => { let src = arg_list(a, "fonti"); s.search(arg_str(a, "query")?, arg_int(a, "limit", 8), (!src.is_empty()).then_some(src), arg_bool(a, "escludi_mie"), arg_bool(a, "include_seen")) }
                "memoria_elenco" => s.list(arg_opt(a, "argomento"), arg_opt(a, "fonte"), arg_int(a, "limit", 30), arg_int(a, "offset", 0)),
                "memoria_ricorda" => s.remember(arg_str(a, "testo")?, arg_opt(a, "argomento").unwrap_or(""), None),
                "memoria_correggi" => s.edit(arg_str(a, "id")?, arg_str(a, "testo")?),
                "memoria_dimentica" => s.forget(arg_str(a, "id")?),
                _ => return Err(String::new()),
            })
        })()).filter(|r| !matches!(r, Err(e) if e.is_empty()));
    }
    if name.starts_with("travaso_") {
        let t = app.t.as_mut()?;
        let s = app.s.as_mut();
        return Some((|| -> Result<Value, String> {
            Ok(match name {
                "travaso_stato" => t.status(),
                "travaso_cerca" => t.search(arg_str(a, "query")?, arg_int(a, "limit", 8), s),
                "travaso_prossimi" => t.next(arg_int(a, "limit", 15), arg_opt(a, "file"), s),
                "travaso_assorbi" => t.absorb(&arg_list(a, "ids"), arg_opt(a, "dove").unwrap_or(""), s),
                "travaso_archivio" => t.archive_search(arg_str(a, "query")?, arg_int(a, "limit", 8)),
                "travaso_ripristina" => t.restore(arg_bool(a, "conferma")),
                _ => return Err(String::new()),
            })
        })()).filter(|r| !matches!(r, Err(e) if e.is_empty()));
    }
    None
}

fn handle(app: &mut App, msg: &Value) -> Option<Value> {
    let id = msg.get("id")?.clone();
    if id.is_null() { return None; }
    let method = msg["method"].as_str().unwrap_or("");
    let result = match method {
        "initialize" => {
            let pv = msg["params"]["protocolVersion"].as_str().unwrap_or("2025-06-18");
            let instr = [app.t.is_some().then_some(INSTRUCTIONS), app.s.is_some().then_some(SHARED_INSTRUCTIONS)]
                .into_iter().flatten().collect::<Vec<_>>().join(" ");
            json!({"protocolVersion": pv, "capabilities": {"tools": {}}, "serverInfo": {"name": NAME, "version": VERSION}, "instructions": instr})
        }
        "ping" => json!({}),
        "tools/list" => {
            let mut tools = Vec::new();
            if app.t.is_some() { tools.extend(travaso_tools().as_array().cloned().unwrap_or_default()); }
            if app.s.is_some() { tools.extend(shared_tools().as_array().cloned().unwrap_or_default()); }
            json!({"tools": tools})
        }
        "tools/call" => {
            let name = msg["params"]["name"].as_str().unwrap_or("");
            let empty = Map::new();
            let a = msg["params"]["arguments"].as_object().unwrap_or(&empty);
            match call_tool(app, name, a) {
                None => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32602, "message": format!("tool non disponibile: {name}")}})),
                Some(Ok(out)) => {
                    let is_err = out.get("error").is_some();
                    json!({"content": [{"type": "text", "text": serde_json::to_string_pretty(&out).unwrap_or_default()}], "isError": is_err})
                }
                Some(Err(e)) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
            }
        }
        _ => return Some(json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": format!("metodo non supportato: {method}")}})),
    };
    Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
}

fn dispatch(app: &mut App, msg: &Value) -> Option<Value> {
    match msg {
        Value::Array(list) => {
            let out: Vec<Value> = list.iter().filter_map(|m| handle(app, m)).collect();
            (!out.is_empty()).then(|| Value::Array(out))
        }
        m => handle(app, m),
    }
}

fn serve_stdio(app: &mut App) {
    let stdin = io::stdin();
    let mut out = io::stdout().lock();
    let mut buf = Vec::new();
    let mut lock = stdin.lock();
    loop {
        buf.clear();
        match lock.read_until(b'\n', &mut buf) { Ok(0) | Err(_) => break, Ok(_) => {} }
        let line = String::from_utf8_lossy(&buf);
        let line = line.trim();
        if line.is_empty() { continue; }
        let Ok(msg) = serde_json::from_str::<Value>(line) else { continue };
        let resps: Vec<Value> = match &msg {
            Value::Array(l) => l.iter().filter_map(|m| handle(app, m)).collect(),
            m => handle(app, m).into_iter().collect(),
        };
        for r in resps {
            let _ = writeln!(out, "{r}");
            let _ = out.flush();
        }
    }
}

fn serve_http(app: &mut App, port: u16) {
    let server = match tiny_http::Server::http(("127.0.0.1", port)) {
        Ok(s) => s,
        Err(e) => { eprintln!("impossibile ascoltare sulla porta {port}: {e}"); std::process::exit(1); }
    };
    eprintln!("travaso in ascolto su http://127.0.0.1:{port}/mcp");
    let ctype = tiny_http::Header::from_bytes(&b"Content-Type"[..], &b"application/json"[..]).expect("header");
    for mut req in server.incoming_requests() {
        let url = req.url().to_string();
        let resp = match (req.method(), url.as_str()) {
            (tiny_http::Method::Get, "/" | "/health") => tiny_http::Response::from_string(json!({"server": NAME, "version": VERSION}).to_string()).with_header(ctype.clone()),
            (tiny_http::Method::Post, u) if u.starts_with("/mcp") => {
                let mut body = String::new();
                let _ = req.as_reader().read_to_string(&mut body);
                match serde_json::from_str::<Value>(&body) {
                    Err(_) => tiny_http::Response::from_string("").with_status_code(400),
                    Ok(msg) => match dispatch(app, &msg) {
                        None => tiny_http::Response::from_string("").with_status_code(202),
                        Some(v) => tiny_http::Response::from_string(v.to_string()).with_header(ctype.clone()),
                    },
                }
            }
            (tiny_http::Method::Get, _) => tiny_http::Response::from_string("").with_status_code(405),
            _ => tiny_http::Response::from_string("").with_status_code(404),
        };
        let _ = req.respond(resp);
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let root = std::env::var("TRAVASO_DIR").map(PathBuf::from).unwrap_or_else(|_| {
        let home = std::env::var("USERPROFILE").or_else(|_| std::env::var("HOME")).unwrap_or_else(|_| ".".into());
        PathBuf::from(home).join(".travaso")
    });
    let mut mode = std::env::var("TRAVASO_MODALITA").unwrap_or_else(|_| "completa".into()).to_lowercase();
    if !["completa", "condivisa", "travaso"].contains(&mode.as_str()) { mode = "completa".into(); }
    let s = (mode != "travaso").then(|| Condivisa::new(&root, &std::env::var("TRAVASO_CLIENT").unwrap_or_else(|_| "ia".into())));
    let t = (mode != "condivisa").then(|| Travaso::new(&root, std::env::var("TRAVASO_AUTO").as_deref() == Ok("1")));
    let mut app = App { t, s };

    let out = match args.first().map(String::as_str) {
        Some("--importa") if args.len() > 1 && app.t.is_some() => app.t.as_mut().unwrap().import_dir(Path::new(&args[1])),
        Some("--stato") => {
            let tv = app.t.as_mut().map(|t| t.status()).unwrap_or(Value::Null);
            let sv = app.s.as_ref().map(|s| s.status()).unwrap_or(Value::Null);
            json!({"modalita": mode, "travaso": tv, "condivisa": sv})
        }
        Some("--versa-tutto") if app.t.is_some() => { let s = app.s.as_mut(); app.t.as_mut().unwrap().pour_all(s) }
        Some("--http") => { let port = args.get(1).and_then(|p| p.parse().ok()).unwrap_or(8765); return serve_http(&mut app, port); }
        Some("--version") => json!({"travaso": VERSION}),
        _ => return serve_stdio(&mut app),
    };
    println!("{}", serde_json::to_string_pretty(&out).unwrap_or_default());
}
