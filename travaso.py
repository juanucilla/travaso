#!/usr/bin/env python3
"""
Travaso — un server MCP che travasa una memoria da un'intelligenza artificiale a un'altra.

  1. CONSERVA  al primo avvio fotografa la memoria in archivio/, che non viene mai modificato.
  2. SCARICA   ogni consultazione consegna fatti nuovi; quando l'IA li ha salvati nella propria
               memoria chiama travaso_assorbi e i fatti escono dal serbatoio.
  3. SVUOTA    quando ogni fatto è stato assorbito il serbatoio è vuoto: il travaso è finito.
               L'archivio resta consultabile con travaso_archivio e ripristinabile con travaso_ripristina.

MEMORIA CONDIVISA (dalla 1.1): un pozzo comune, condivisa.jsonl, che NON si svuota mai. Più IA
(per esempio Cursor e Codex) lo leggono e ci scrivono, e ogni fatto porta la sua provenienza
(fonte = claude, codex, cursor…). I fatti travasati dal serbatoio finiscono anche qui, con
fonte = TRAVASO_ORIGINE (predefinita "claude").

Funziona con qualsiasi client MCP: ChatGPT/Codex, Claude, Cursor, Gemini CLI, VS Code...
Trasporti: stdio (predefinito) oppure HTTP "streamable" con --http PORTA.
Solo libreria standard, Python 3.9+.

Uso:
  python travaso.py                          # stdio
  python travaso.py --http 8765              # http://127.0.0.1:8765/mcp
  python travaso.py --importa CARTELLA       # carica file .md nel serbatoio e ne fa l'archivio
  python travaso.py --stato                  # stampa il livello del serbatoio
  python travaso.py --versa-tutto            # versa tutto il serbatoio nella memoria condivisa

Variabili d'ambiente:
  TRAVASO_DIR       cartella dati, uguale per tutte le IA che condividono (predefinita ~/.travaso)
  TRAVASO_CLIENT    nome di chi scrive nella condivisa: cursor, codex, claude… (predefinito "ia")
  TRAVASO_MODALITA  completa (travaso + condivisa, predefinita) | condivisa (non si svuota mai) | travaso
  TRAVASO_ORIGINE   fonte dei fatti travasati dal serbatoio (predefinita "claude")
  TRAVASO_AUTO=1    svuota subito ciò che viene consultato, senza conferma
"""
from __future__ import annotations

import datetime as _dt
import hashlib
import json
import math
import os
import re
import shutil
import sys
import unicodedata
from pathlib import Path

__version__ = "1.1.0"
NAME = "travaso"

STOPWORDS = set("""
a ad al alla alle agli ai all anche che chi con da dal dalla dei del della delle dello di e ed gli ha ho i il in
la le lo ma mi ne nei nel nella non o per piu più poi quale quali se si sia sono su sul sulla tra un una uno
the of and or to in on for with is are was be by at as from it its an this that not no
""".split())

SECRET_RE = re.compile(
    r"(?i)(AKIA[0-9A-Z]{16}|-----BEGIN [A-Z ]*PRIVATE KEY|sk-[A-Za-z0-9]{20,}|ghp_[A-Za-z0-9]{20,}|gho_[A-Za-z0-9]{20,}|"
    r"xox[abp]-|password\s*[:=]|passwd\s*[:=]|api[_-]?key\s*[:=]|secret\s*[:=]|token\s*[:=])"
)


def _norm(s: str) -> str:
    s = unicodedata.normalize("NFKD", s)
    return "".join(c for c in s if not unicodedata.combining(c)).lower()


def _tokens(s: str) -> list[str]:
    return [t for t in re.findall(r"\w[\w\-\.@/]*\w|\w", _norm(s)) if t not in STOPWORDS]


def _frontmatter(text: str) -> tuple[dict, str, int]:
    """Restituisce (metadati, corpo, numero di righe del frontmatter)."""
    meta: dict = {}
    if text.startswith("---"):
        lines = text.split("\n")
        for i in range(1, len(lines)):
            if lines[i].strip() == "---":
                for ln in lines[1:i]:
                    if ":" in ln:
                        k, v = ln.split(":", 1)
                        meta[k.strip()] = v.strip()
                return meta, "\n".join(lines[i + 1:]), i + 1
    return meta, text, 0


def _now() -> str:
    return _dt.datetime.now().astimezone().isoformat(timespec="seconds")


class Travaso:
    def __init__(self, root: Path, auto: bool = False):
        self.root = root
        self.tank = root / "serbatoio"
        self.archive = root / "archivio"
        self.log = root / "travasato.jsonl"
        self.auto = auto
        self.shared: "Condivisa | None" = None  # se impostata, ciò che si assorbe finisce anche lì
        self.origin = os.environ.get("TRAVASO_ORIGINE", "claude")
        self.in_transit: set[str] = set()
        self.tank.mkdir(parents=True, exist_ok=True)
        if not self.archive.exists() and any(self.tank.rglob("*.md")):
            shutil.copytree(self.tank, self.archive)  # 1. CONSERVA
        self.reload()

    # ------------------------------------------------------------ indice
    def _scan(self, base: Path) -> tuple[dict, list[dict]]:
        files: dict[str, dict] = {}
        facts: list[dict] = []
        if not base.exists():
            return files, facts
        for path in sorted(base.rglob("*.md")):
            rel = path.relative_to(base).as_posix()
            meta, body, _ = _frontmatter(path.read_text(encoding="utf-8", errors="replace"))
            aliases = [a.strip() for a in re.findall(r"[^\[\],]+", meta.get("aliases", "")) if a.strip()]
            info = {"file": rel, "name": meta.get("name", path.stem), "description": meta.get("description", ""),
                    "aliases": aliases, "facts": 0}
            section, seen_hash = "", {}
            for line in body.split("\n"):
                s = line.strip()
                if not s:
                    continue
                if s.startswith("#"):
                    section = s.lstrip("#").strip()
                    continue
                h = hashlib.sha1(f"{rel}\n{s}".encode("utf-8")).hexdigest()[:10]
                seen_hash[h] = seen_hash.get(h, 0) + 1
                fid = h if seen_hash[h] == 1 else f"{h}-{seen_hash[h]}"
                header = " ".join([info["name"], " ".join(aliases), section])
                facts.append({"id": fid, "file": rel, "section": section, "line": s,
                              "text": re.sub(r"^[-*]\s+", "", s), "_tok": _tokens(s), "_hdr": _tokens(header)})
                info["facts"] += 1
            files[rel] = info
        return files, facts

    def reload(self) -> None:
        self.files, self.facts = self._scan(self.tank)
        self.by_id = {f["id"]: f for f in self.facts}
        self.in_transit &= set(self.by_id)
        corpus = self.facts or self._scan(self.archive)[1]
        df: dict[str, int] = {}
        for f in corpus:
            for t in set(f["_tok"]) | set(f["_hdr"]):
                df[t] = df.get(t, 0) + 1
        n = max(len(corpus), 1)
        self.idf = {t: math.log(1 + n / d) for t, d in df.items()}

    def _archive_count(self) -> int:
        return len(self._scan(self.archive)[1])

    def _absorbed_count(self) -> int:
        if not self.log.exists():
            return 0
        with self.log.open(encoding="utf-8") as fh:
            return sum(1 for ln in fh if ln.strip())

    # ------------------------------------------------------------ ricerca
    def _score(self, f: dict, q: list[str]) -> float:
        body, hdr, score = set(f["_tok"]), set(f["_hdr"]), 0.0
        for t in q:
            w = self.idf.get(t, 1.0)
            if t in body:
                score += w
            elif len(t) > 3 and any(len(b) > 3 and (b.startswith(t) or t.startswith(b)) for b in body):
                score += 0.6 * w
            if t in hdr:
                score += 0.5 * w
        return score

    def _suggest(self, results: list[dict], q: list[str]) -> list[str]:
        cand: dict[str, int] = {}
        word = r"[^\W\d_][\w&\-]*"
        for r in results:
            text = r["text"]
            for m in re.finditer(rf"(?<![\w&\-])([A-Z]{word}(?:\s+[A-Z]{word})?|[A-Z]{{2,}}[\w\-]*)", text):
                term, start = m.group(1), m.start(1)
                before = text[:start].rstrip()
                if (start == 0 or before.endswith((".", ":", ";", "(", "—"))) and not term.isupper() and " " not in term:
                    continue
                if len(term) < 3 or all(t in q for t in _tokens(term)):
                    continue
                cand[term] = cand.get(term, 0) + 1
        return [k for k, _ in sorted(cand.items(), key=lambda kv: -kv[1])][:8]

    def _deliver(self, picked: list[dict]) -> list[dict]:
        out = [{"id": f["id"], "file": f["file"], "section": f["section"], "text": f["text"]} for f in picked]
        if self.auto:
            self.absorb([f["id"] for f in picked], "auto")
        else:
            self.in_transit.update(f["id"] for f in picked)
        return out

    # ------------------------------------------------------------ strumenti
    def status(self) -> dict:
        self.reload()  # un altro processo (un'altra IA) può aver svuotato il serbatoio
        total_archive = self._archive_count()
        left = len(self.facts)
        level = round(100 * left / total_archive, 1) if total_archive else 0.0
        return {
            "serbatoio_percento": level,
            "fatti_nel_serbatoio": left,
            "fatti_in_viaggio": len(self.in_transit),
            "fatti_assorbiti_totali": self._absorbed_count(),
            "fatti_in_archivio": total_archive,
            "vuoto": left == 0,
            "file": list(self.files.values()),
            "istruzioni": (
                "Il travaso è finito: il serbatoio è vuoto. L'archivio resta consultabile con travaso_archivio."
                if left == 0 else
                "Consulta con travaso_cerca (rilancia con next_queries) o travaso_prossimi. Salva ogni fatto "
                "ricevuto nella TUA memoria permanente, poi chiama travaso_assorbi con i loro id: solo allora escono "
                "dal serbatoio. Continua finché vuoto=true."),
        }

    def search(self, query: str, limit: int = 8) -> dict:
        self.reload()
        q = _tokens(query)
        if not q:
            return {"error": "query vuota"}
        hits = sorted(((self._score(f, q), f) for f in self.facts), key=lambda x: -x[0])
        fresh = [f for s, f in hits if s > 0 and f["id"] not in self.in_transit]
        page = fresh[: max(1, min(int(limit), 50))]
        results = self._deliver(page)
        remaining = len(fresh) - len(page)
        return {"query": query, "results": results, "remaining_matches": remaining,
                "exhausted_for_query": remaining == 0, "next_queries": self._suggest(results, q),
                "da_assorbire": [] if self.auto else [r["id"] for r in results],
                "serbatoio_percento": self.status()["serbatoio_percento"]}

    def next(self, limit: int = 15, file: str | None = None) -> dict:
        self.reload()
        pool = [f for f in self.facts if f["id"] not in self.in_transit and (not file or f["file"] == file)]
        page = pool[: max(1, min(int(limit), 100))]
        results = self._deliver(page)
        return {"results": results, "remaining": len(pool) - len(page),
                "exhausted": len(pool) - len(page) == 0,
                "da_assorbire": [] if self.auto else [r["id"] for r in results]}

    def absorb(self, ids: list[str], where: str = "") -> dict:
        """2. SCARICA: i fatti confermati escono dai file del serbatoio e finiscono nel registro."""
        with _FileLock(self.root / "serbatoio.lock"):
            return self._absorb(ids, where)

    def _absorb(self, ids: list[str], where: str) -> dict:
        self.reload()
        ids =[i for i in dict.fromkeys(ids) if i in self.by_id]
        if not ids:
            return {"assorbiti": 0, "nota": "nessun id valido (forse già assorbiti)"}
        by_file: dict[str, list[dict]] = {}
        for i in ids:
            by_file.setdefault(self.by_id[i]["file"], []).append(self.by_id[i])
        stamp = _now()
        with self.log.open("a", encoding="utf-8") as log:
            for rel, items in by_file.items():
                path = self.tank / rel
                lines = path.read_text(encoding="utf-8").split("\n")
                _, _, fm = _frontmatter("\n".join(lines))
                for it in items:
                    for k in range(fm, len(lines)):
                        if lines[k].strip() == it["line"]:
                            del lines[k]
                            break
                    log.write(json.dumps({"id": it["id"], "file": rel, "text": it["text"],
                                          "assorbito_da": where, "quando": stamp}, ensure_ascii=False) + "\n")
                    if self.shared is not None:
                        topic = Path(rel).stem + (f" · {it['section']}" if it["section"] else "")
                        self.shared.remember(it["text"], topic, self.origin)
                _, body, _ = _frontmatter("\n".join(lines))
                if not any(ln.strip() and not ln.strip().startswith("#") for ln in body.split("\n")):
                    path.unlink()  # 3. SVUOTA: il file non ha più fatti
                else:
                    path.write_text("\n".join(lines), encoding="utf-8")
        self.in_transit -= set(ids)
        self.reload()
        st = self.status()
        return {"assorbiti": len(ids), "serbatoio_percento": st["serbatoio_percento"],
                "fatti_nel_serbatoio": st["fatti_nel_serbatoio"], "vuoto": st["vuoto"]}

    def archive_search(self, query: str, limit: int = 8) -> dict:
        """1. CONSERVA: l'archivio originale è sempre consultabile, anche a serbatoio vuoto."""
        q = _tokens(query)
        _, facts = self._scan(self.archive)
        hits = sorted(((self._score(f, q), f) for f in facts), key=lambda x: -x[0])
        return {"query": query, "results": [{"file": f["file"], "section": f["section"], "text": f["text"]}
                                            for s, f in hits if s > 0][: max(1, min(int(limit), 50))]}

    def restore(self, confirm: bool) -> dict:
        if not confirm:
            return {"error": "passa conferma=true: il serbatoio verrà riempito di nuovo dall'archivio"}
        if not self.archive.exists():
            return {"error": "nessun archivio"}
        shutil.rmtree(self.tank)
        shutil.copytree(self.archive, self.tank)
        self.in_transit.clear()
        self.reload()
        return {"ok": True, "fatti_nel_serbatoio": len(self.facts)}

    def import_dir(self, src: Path) -> dict:
        n = 0
        for p in sorted(src.rglob("*.md")):
            text = p.read_text(encoding="utf-8", errors="replace")
            if SECRET_RE.search(text):
                print(f"saltato (sembra contenere segreti): {p}", file=sys.stderr)
                continue
            dst = self.tank / p.relative_to(src)
            dst.parent.mkdir(parents=True, exist_ok=True)
            dst.write_text(text, encoding="utf-8")
            n += 1
        if self.archive.exists():
            shutil.rmtree(self.archive)
        shutil.copytree(self.tank, self.archive)
        self.reload()
        return {"file_importati": n, "fatti": len(self.facts)}

    def pour_all(self) -> dict:
        """Versa tutto il serbatoio nella memoria condivisa (senza passare da un'IA)."""
        if self.shared is None:
            return {"error": "memoria condivisa non attiva"}
        ids = [f["id"] for f in self.facts]
        if not ids:
            return {"assorbiti": 0, "vuoto": True}
        return self.absorb(ids, "memoria condivisa")


# ---------------------------------------------------------------- memoria condivisa
class _FileLock:
    """Lock tra processi (Cursor e Codex avviano ciascuno il proprio server sullo stesso file)."""

    def __init__(self, path: Path):
        self.path = path

    def __enter__(self):
        self.fh = open(self.path, "a+b")
        if os.name == "nt":
            import msvcrt
            import time
            while True:
                try:
                    self.fh.seek(0)
                    msvcrt.locking(self.fh.fileno(), msvcrt.LK_NBLCK, 1)
                    break
                except OSError:
                    time.sleep(0.05)
        else:
            import fcntl
            fcntl.flock(self.fh.fileno(), fcntl.LOCK_EX)
        return self

    def __exit__(self, *exc):
        if os.name == "nt":
            import msvcrt
            self.fh.seek(0)
            msvcrt.locking(self.fh.fileno(), msvcrt.LK_UNLCK, 1)
        else:
            import fcntl
            fcntl.flock(self.fh.fileno(), fcntl.LOCK_UN)
        self.fh.close()


class Condivisa:
    """Pozzo comune a più IA: registro solo-aggiunte (condivisa.jsonl), non si svuota mai."""

    def __init__(self, root: Path, client: str):
        self.root = root
        self.file = root / "condivisa.jsonl"
        self.lockfile = root / "condivisa.lock"
        self.client = re.sub(r"[^\w\-]", "", client.lower())[:32] or "ia"
        self.seen: set[str] = set()
        root.mkdir(parents=True, exist_ok=True)

    # ------------------------------------------------------------ registro
    def _load(self) -> dict[str, dict]:
        facts: dict[str, dict] = {}
        if not self.file.exists():
            return facts
        with self.file.open(encoding="utf-8") as fh:
            for ln in fh:
                try:
                    ev = json.loads(ln)
                except json.JSONDecodeError:
                    continue
                fid, op = ev.get("id"), ev.get("op")
                if op == "add" and fid not in facts:
                    facts[fid] = {"id": fid, "testo": ev["testo"], "argomento": ev.get("argomento", ""),
                                  "fonte": ev.get("fonte", ""), "creato": ev.get("quando", ""),
                                  "aggiornato": ev.get("quando", ""), "modificato_da": []}
                elif op == "edit" and fid in facts:
                    facts[fid]["testo"] = ev["testo"]
                    facts[fid]["aggiornato"] = ev.get("quando", "")
                    facts[fid]["modificato_da"].append(ev.get("fonte", ""))
                elif op == "del" and fid in facts:
                    del facts[fid]
        return facts

    def _append(self, ev: dict) -> None:
        with self.file.open("a", encoding="utf-8") as fh:
            fh.write(json.dumps(ev, ensure_ascii=False) + "\n")

    @staticmethod
    def _score(f: dict, q: list[str]) -> float:
        body, hdr, score = set(_tokens(f["testo"])), set(_tokens(f["argomento"] + " " + f["fonte"])), 0.0
        for t in q:
            if t in body:
                score += 1.0
            elif len(t) > 3 and any(len(b) > 3 and (b.startswith(t) or t.startswith(b)) for b in body):
                score += 0.6
            if t in hdr:
                score += 0.5
        return score

    @staticmethod
    def _public(f: dict) -> dict:
        return {k: f[k] for k in ("id", "testo", "argomento", "fonte", "aggiornato")}

    # ------------------------------------------------------------ strumenti
    def remember(self, text: str, topic: str = "", source: str | None = None) -> dict:
        text = " ".join(str(text).split())
        if not text:
            return {"error": "testo vuoto"}
        if SECRET_RE.search(text):
            return {"error": "rifiutato: sembra contenere un segreto (password, chiave o token)"}
        source = source or self.client
        with _FileLock(self.lockfile):
            facts = self._load()
            key = _norm(text)
            for f in facts.values():
                if _norm(f["testo"]) == key:
                    return {"ok": True, "id": f["id"], "nota": f"già presente (fonte: {f['fonte']})"}
            fid = hashlib.sha1(f"{key}\n{_now()}".encode("utf-8")).hexdigest()[:10]
            self._append({"op": "add", "id": fid, "testo": text, "argomento": " ".join(str(topic).split()),
                          "fonte": source, "quando": _now()})
        return {"ok": True, "id": fid, "fonte": source}

    def search(self, query: str, limit: int = 8, sources: list[str] | None = None,
               exclude_mine: bool = False, include_seen: bool = False) -> dict:
        q = _tokens(query)
        facts = self._load().values()
        if sources:
            facts = [f for f in facts if f["fonte"] in sources]
        if exclude_mine:
            facts = [f for f in facts if f["fonte"] != self.client]
        hits = sorted(((self._score(f, q), f) for f in facts), key=lambda x: -x[0]) if q else []
        fresh = [f for s, f in hits if s > 0 and (include_seen or f["id"] not in self.seen)]
        page = fresh[: max(1, min(int(limit), 50))]
        self.seen.update(f["id"] for f in page)
        remaining = len(fresh) - len(page)
        return {"query": query, "results": [self._public(f) for f in page],
                "remaining_matches": remaining, "exhausted_for_query": remaining == 0}

    def list(self, topic: str | None = None, source: str | None = None, limit: int = 30, offset: int = 0) -> dict:
        facts = sorted(self._load().values(), key=lambda f: f["aggiornato"], reverse=True)
        if topic:
            facts = [f for f in facts if _norm(topic) in _norm(f["argomento"])]
        if source:
            facts = [f for f in facts if f["fonte"] == source]
        page = facts[offset: offset + max(1, min(int(limit), 100))]
        return {"totale": len(facts), "results": [self._public(f) for f in page],
                "prossimo_offset": offset + len(page) if offset + len(page) < len(facts) else None}

    def edit(self, fid: str, text: str) -> dict:
        text = " ".join(str(text).split())
        if not text:
            return {"error": "testo vuoto"}
        if SECRET_RE.search(text):
            return {"error": "rifiutato: sembra contenere un segreto"}
        with _FileLock(self.lockfile):
            if fid not in self._load():
                return {"error": f"id non trovato: {fid}"}
            self._append({"op": "edit", "id": fid, "testo": text, "fonte": self.client, "quando": _now()})
        return {"ok": True, "id": fid}

    def forget(self, fid: str) -> dict:
        with _FileLock(self.lockfile):
            if fid not in self._load():
                return {"error": f"id non trovato: {fid}"}
            self._append({"op": "del", "id": fid, "fonte": self.client, "quando": _now()})
        return {"ok": True, "id": fid}

    def status(self) -> dict:
        facts = list(self._load().values())
        by_source: dict[str, int] = {}
        for f in facts:
            by_source[f["fonte"]] = by_source.get(f["fonte"], 0) + 1
        return {"fatti": len(facts), "per_fonte": by_source, "io_sono": self.client,
                "file": str(self.file), "letti_in_questa_sessione": len(self.seen)}


class App:
    def __init__(self, t: "Travaso | None", s: "Condivisa | None", mode: str):
        self.t, self.s, self.mode = t, s, mode


# ---------------------------------------------------------------- MCP
TOOLS = [
    {"name": "travaso_stato",
     "description": "Livello del serbatoio di memoria da travasare: quanti fatti restano, quanti sono stati assorbiti, file e istruzioni. Chiamalo per primo.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "travaso_cerca",
     "description": "Cerca nel serbatoio e consegna solo fatti non ancora consegnati, con i termini da cercare dopo (next_queries). Salva i fatti nella tua memoria permanente e poi chiama travaso_assorbi.",
     "inputSchema": {"type": "object", "required": ["query"], "properties": {
         "query": {"type": "string"}, "limit": {"type": "integer", "default": 8}}}},
    {"name": "travaso_prossimi",
     "description": "Consegna in ordine i prossimi fatti del serbatoio, per svuotarlo tutto. Ripeti finché exhausted=true, assorbendo dopo ogni blocco.",
     "inputSchema": {"type": "object", "properties": {
         "limit": {"type": "integer", "default": 15}, "file": {"type": "string"}}}},
    {"name": "travaso_assorbi",
     "description": "Conferma che hai salvato questi fatti nella tua memoria: escono dal serbatoio per sempre (restano solo in archivio).",
     "inputSchema": {"type": "object", "required": ["ids"], "properties": {
         "ids": {"type": "array", "items": {"type": "string"}},
         "dove": {"type": "string", "description": "dove li hai salvati, es. 'memoria ChatGPT'"}}}},
    {"name": "travaso_archivio",
     "description": "Cerca nell'archivio originale, che non si svuota mai. Da usare solo per ricordare qualcosa dopo il travaso.",
     "inputSchema": {"type": "object", "required": ["query"], "properties": {
         "query": {"type": "string"}, "limit": {"type": "integer", "default": 8}}}},
    {"name": "travaso_ripristina",
     "description": "Riempie di nuovo il serbatoio dall'archivio. Solo su richiesta esplicita dell'utente.",
     "inputSchema": {"type": "object", "required": ["conferma"], "properties": {"conferma": {"type": "boolean"}}}},
]

SHARED_TOOLS = [
    {"name": "memoria_stato",
     "description": "Memoria CONDIVISA tra più IA (Cursor, Codex, Claude…): quanti fatti ci sono, per fonte, e chi sei tu. Non si svuota mai.",
     "inputSchema": {"type": "object", "properties": {}}},
    {"name": "memoria_cerca",
     "description": "Cerca nella memoria condivisa (fatti scritti da tutte le IA, ognuno con la sua fonte). Restituisce solo fatti non ancora letti in questa sessione: rilancia finché exhausted_for_query=true. Usalo prima di rispondere su utente e progetti.",
     "inputSchema": {"type": "object", "required": ["query"], "properties": {
         "query": {"type": "string"}, "limit": {"type": "integer", "default": 8},
         "fonti": {"type": "array", "items": {"type": "string"}, "description": "es. [\"claude\",\"codex\"]"},
         "escludi_mie": {"type": "boolean", "default": False},
         "include_seen": {"type": "boolean", "default": False}}}},
    {"name": "memoria_elenco",
     "description": "Elenca i fatti della memoria condivisa, dai più recenti, filtrando per argomento o fonte.",
     "inputSchema": {"type": "object", "properties": {
         "argomento": {"type": "string"}, "fonte": {"type": "string"},
         "limit": {"type": "integer", "default": 30}, "offset": {"type": "integer", "default": 0}}}},
    {"name": "memoria_ricorda",
     "description": "Aggiunge alla memoria condivisa un fatto durevole detto dall'utente o deciso nel lavoro. Le altre IA lo vedranno. Mai segreti o dati sanitari.",
     "inputSchema": {"type": "object", "required": ["testo"], "properties": {
         "testo": {"type": "string"}, "argomento": {"type": "string"}}}},
    {"name": "memoria_correggi",
     "description": "Aggiorna il testo di un fatto della memoria condivisa (resta traccia di chi l'ha modificato).",
     "inputSchema": {"type": "object", "required": ["id", "testo"], "properties": {
         "id": {"type": "string"}, "testo": {"type": "string"}}}},
    {"name": "memoria_dimentica",
     "description": "Toglie un fatto dalla memoria condivisa. Solo se l'utente lo chiede o il fatto è sbagliato.",
     "inputSchema": {"type": "object", "required": ["id"], "properties": {"id": {"type": "string"}}}},
]

INSTRUCTIONS = ("Travaso: memoria da trasferire nella tua. Procedura: travaso_stato → travaso_cerca/travaso_prossimi → "
                "salva i fatti nella tua memoria permanente → travaso_assorbi con i loro id → ripeti finché vuoto=true.")
SHARED_INSTRUCTIONS = ("Memoria condivisa tra più IA: consultala con memoria_cerca prima di rispondere su utente e progetti "
                       "(rilancia finché exhausted_for_query=true); salva con memoria_ricorda i fatti nuovi e durevoli. "
                       "Ogni fatto porta la sua fonte. Non si svuota mai.")


def _tools_for(app: "App") -> list[dict]:
    tools = []
    if app.t is not None:
        tools += TOOLS
    if app.s is not None:
        tools += SHARED_TOOLS
    return tools


def _call_shared(s: "Condivisa", name: str, a: dict) -> dict | None:
    if name == "memoria_stato":
        return s.status()
    if name == "memoria_cerca":
        return s.search(a["query"], a.get("limit", 8), a.get("fonti"), bool(a.get("escludi_mie")),
                        bool(a.get("include_seen")))
    if name == "memoria_elenco":
        return s.list(a.get("argomento"), a.get("fonte"), a.get("limit", 30), a.get("offset", 0))
    if name == "memoria_ricorda":
        return s.remember(a["testo"], a.get("argomento", ""))
    if name == "memoria_correggi":
        return s.edit(a["id"], a["testo"])
    if name == "memoria_dimentica":
        return s.forget(a["id"])
    return None


def handle(app: "App", msg: dict) -> dict | None:
    method, mid = msg.get("method"), msg.get("id")
    if mid is None:
        return None
    t = app.t
    if method == "initialize":
        pv = (msg.get("params") or {}).get("protocolVersion", "2025-06-18")
        instr = " ".join(x for x in (INSTRUCTIONS if t else "", SHARED_INSTRUCTIONS if app.s else "") if x)
        result = {"protocolVersion": pv, "capabilities": {"tools": {}},
                  "serverInfo": {"name": NAME, "version": __version__}, "instructions": instr}
    elif method == "ping":
        result = {}
    elif method == "tools/list":
        result = {"tools": _tools_for(app)}
    elif method == "tools/call":
        p = msg.get("params") or {}
        name, a = p.get("name"), p.get("arguments") or {}
        try:
            out = _call_shared(app.s, name, a) if app.s is not None and name.startswith("memoria_") else None
            if out is not None:
                pass
            elif t is None or not name.startswith("travaso_"):
                return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": f"tool non disponibile: {name}"}}
            elif name == "travaso_stato":
                out = t.status()
            elif name == "travaso_cerca":
                out = t.search(a["query"], a.get("limit", 8))
            elif name == "travaso_prossimi":
                out = t.next(a.get("limit", 15), a.get("file"))
            elif name == "travaso_assorbi":
                out = t.absorb(list(a.get("ids") or []), a.get("dove", ""))
            elif name == "travaso_archivio":
                out = t.archive_search(a["query"], a.get("limit", 8))
            elif name == "travaso_ripristina":
                out = t.restore(bool(a.get("conferma")))
            else:
                return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32602, "message": f"tool sconosciuto: {name}"}}
            result = {"content": [{"type": "text", "text": json.dumps(out, ensure_ascii=False, indent=1)}],
                      "isError": "error" in out}
        except Exception as e:  # noqa: BLE001
            result = {"content": [{"type": "text", "text": f"errore: {e}"}], "isError": True}
    else:
        return {"jsonrpc": "2.0", "id": mid, "error": {"code": -32601, "message": f"metodo non supportato: {method}"}}
    return {"jsonrpc": "2.0", "id": mid, "result": result}


def serve_stdio(t: "App") -> None:
    for raw in sys.stdin.buffer:
        line = raw.decode("utf-8", errors="replace").strip()
        if not line:
            continue
        try:
            msg = json.loads(line)
        except json.JSONDecodeError:
            continue
        msgs = msg if isinstance(msg, list) else [msg]
        for m in msgs:
            resp = handle(t, m)
            if resp is not None:
                sys.stdout.buffer.write((json.dumps(resp, ensure_ascii=False) + "\n").encode("utf-8"))
                sys.stdout.buffer.flush()


def serve_http(t: "App", port: int, host: str = "127.0.0.1") -> None:
    from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
    import threading
    lock = threading.Lock()

    class H(BaseHTTPRequestHandler):
        def _send(self, code: int, body: bytes = b"", ctype: str = "application/json") -> None:
            self.send_response(code)
            self.send_header("Content-Type", ctype)
            self.send_header("Content-Length", str(len(body)))
            self.end_headers()
            self.wfile.write(body)

        def do_GET(self):  # noqa: N802
            if self.path.rstrip("/") in ("", "/health"):
                self._send(200, json.dumps({"server": NAME, "version": __version__}).encode())
            else:
                self._send(405)

        def do_POST(self):  # noqa: N802
            if not self.path.startswith("/mcp"):
                return self._send(404)
            size = int(self.headers.get("Content-Length") or 0)
            try:
                msg = json.loads(self.rfile.read(size).decode("utf-8"))
            except json.JSONDecodeError:
                return self._send(400)
            with lock:
                msgs = msg if isinstance(msg, list) else [msg]
                resps = [r for r in (handle(t, m) for m in msgs) if r is not None]
            if not resps:
                return self._send(202)
            body = resps if isinstance(msg, list) else resps[0]
            self._send(200, json.dumps(body, ensure_ascii=False).encode("utf-8"))

        def log_message(self, *args):  # silenzioso
            pass

    print(f"travaso in ascolto su http://{host}:{port}/mcp", file=sys.stderr)
    ThreadingHTTPServer((host, port), H).serve_forever()


def main(argv: list[str] | None = None) -> None:
    argv = list(sys.argv[1:] if argv is None else argv)
    root = Path(os.environ.get("TRAVASO_DIR", Path.home() / ".travaso")).expanduser()
    mode = os.environ.get("TRAVASO_MODALITA", "completa").lower()
    if mode not in ("completa", "condivisa", "travaso"):
        mode = "completa"
    s = Condivisa(root, os.environ.get("TRAVASO_CLIENT", "ia")) if mode in ("completa", "condivisa") else None
    t = None
    if mode in ("completa", "travaso"):
        t = Travaso(root, auto=os.environ.get("TRAVASO_AUTO") == "1")
        t.shared = s
    app = App(t, s, mode)
    if argv[:1] == ["--importa"] and len(argv) > 1 and t:
        out = t.import_dir(Path(argv[1]).expanduser())
    elif argv[:1] == ["--stato"]:
        out = {"modalita": mode, "travaso": t.status() if t else None, "condivisa": s.status() if s else None}
    elif argv[:1] == ["--versa-tutto"] and t:
        out = t.pour_all()
    elif argv[:1] == ["--http"]:
        return serve_http(app, int(argv[1]) if len(argv) > 1 else 8765)
    elif argv[:1] == ["--version"]:
        out = {"travaso": __version__}
    else:
        return serve_stdio(app)
    sys.stdout.buffer.write(json.dumps(out, ensure_ascii=False, indent=1).encode("utf-8") + b"\n")


if __name__ == "__main__":
    main()
