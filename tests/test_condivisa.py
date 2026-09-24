"""Memoria condivisa: Claude si svuota nella condivisa, Cursor e Codex la leggono e ci scrivono.
Esegui: python tests/test_condivisa.py"""
import json
import os
import shutil
import subprocess
import sys
import tempfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[1]
SERVER = ROOT / "travaso.py"
EXAMPLES = ROOT / "examples" / "memoria"
CMD = [os.environ["TRAVASO_BIN"]] if os.environ.get("TRAVASO_BIN") else [sys.executable, str(SERVER)]


class Client:
    def __init__(self, env: dict):
        self.p = subprocess.Popen(CMD, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                                  encoding="utf-8", env=env)
        self.n = 0
        self.call("initialize")

    def call(self, method, params=None):
        self.n += 1
        self.p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": self.n, "method": method, "params": params or {}}) + "\n")
        self.p.stdin.flush()
        return json.loads(self.p.stdout.readline())

    def tools(self):
        return [t["name"] for t in self.call("tools/list")["result"]["tools"]]

    def tool(self, name, **a):
        r = self.call("tools/call", {"name": name, "arguments": a})
        if "error" in r:
            return {"_rpc_error": r["error"]["message"]}
        return json.loads(r["result"]["content"][0]["text"])

    def close(self):
        self.p.kill()


def main() -> None:
    tmp = Path(tempfile.mkdtemp(prefix="travaso-cond-"))
    try:
        base = dict(os.environ, TRAVASO_DIR=str(tmp))
        subprocess.run(CMD + ["--importa", str(EXAMPLES)], env=base, capture_output=True)

        codex = Client(dict(base, TRAVASO_CLIENT="codex", TRAVASO_MODALITA="completa"))
        cursor = Client(dict(base, TRAVASO_CLIENT="cursor", TRAVASO_MODALITA="condivisa"))

        # Cursor NON si svuota: niente strumenti travaso_*
        assert not any(t.startswith("travaso_") for t in cursor.tools()), cursor.tools()
        assert "memoria_cerca" in cursor.tools()
        assert "_rpc_error" in cursor.tool("travaso_prossimi")

        # Claude si svuota nella condivisa (via Codex)
        r = codex.tool("travaso_cerca", query="naufragar mare")
        codex.tool("travaso_assorbi", ids=r["da_assorbire"], dove="condivisa")
        hit = cursor.tool("memoria_cerca", query="naufragar")
        assert hit["results"] and hit["results"][0]["fonte"] == "claude", hit

        # Codex scrive, Cursor legge; Cursor scrive, Codex legge
        codex.tool("memoria_ricorda", testo="Giovanni usa Doppler per i segreti di Artusi", argomento="artusi")
        cursor.tool("memoria_ricorda", testo="Il repo di Travaso è juanucilla/travaso", argomento="travaso")
        seen_by_cursor = cursor.tool("memoria_cerca", query="Doppler")
        assert seen_by_cursor["results"][0]["fonte"] == "codex", seen_by_cursor
        seen_by_codex = codex.tool("memoria_cerca", query="repo Travaso", escludi_mie=True)
        assert seen_by_codex["results"][0]["fonte"] == "cursor", seen_by_codex

        # niente doppioni, niente segreti, correzione e oblio
        assert "già presente" in cursor.tool("memoria_ricorda", testo="Giovanni usa Doppler per i segreti di Artusi")["nota"]
        assert "error" in codex.tool("memoria_ricorda", testo="password: hunter2")
        fid = seen_by_cursor["results"][0]["id"]
        assert cursor.tool("memoria_correggi", id=fid, testo="Giovanni usa Doppler per tutti i segreti")["ok"]
        again = codex.tool("memoria_cerca", query="Doppler tutti", include_seen=True)
        assert any("tutti i segreti" in x["testo"] for x in again["results"]), again

        # svuotamento completo di Claude: la condivisa resta piena, Cursor non perde nulla
        out = subprocess.run(CMD + ["--versa-tutto"], env=dict(base, TRAVASO_CLIENT="codex"),
                             capture_output=True, encoding="utf-8")
        assert json.loads(out.stdout)["vuoto"] is True, out.stdout
        st = cursor.tool("memoria_stato")
        assert st["per_fonte"]["claude"] >= 30 and st["per_fonte"]["codex"] == 1 and st["per_fonte"]["cursor"] == 1, st
        assert codex.tool("travaso_stato")["vuoto"] is True
        assert codex.tool("travaso_archivio", query="ermo colle")["results"], "l'archivio deve restare"

        codex.close()
        cursor.close()
        print(f"TUTTI I TEST OK (condivisa: {st['fatti']} fatti, per fonte {st['per_fonte']})")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
