"""Test end-to-end di Travaso via MCP stdio. Esegui: python tests/test_travaso.py"""
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
# TRAVASO_BIN=percorso/del/binario Rust per testare la versione Rust con gli stessi test
CMD = [os.environ["TRAVASO_BIN"]] if os.environ.get("TRAVASO_BIN") else [sys.executable, str(SERVER)]


def main() -> None:
    tmp = Path(tempfile.mkdtemp(prefix="travaso-test-"))
    try:
        env = dict(os.environ, TRAVASO_DIR=str(tmp))
        r = subprocess.run(CMD + ["--importa", str(EXAMPLES)], env=env,
                           capture_output=True, encoding="utf-8")
        imported = json.loads(r.stdout)
        assert imported["file_importati"] == 2, imported
        total = imported["fatti"]

        p = subprocess.Popen(CMD, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                             encoding="utf-8", env=env)
        n = 0

        def call(method, params=None):
            nonlocal n
            n += 1
            p.stdin.write(json.dumps({"jsonrpc": "2.0", "id": n, "method": method, "params": params or {}}) + "\n")
            p.stdin.flush()
            return json.loads(p.stdout.readline())

        def tool(name, **a):
            res = call("tools/call", {"name": name, "arguments": a})["result"]
            return json.loads(res["content"][0]["text"])

        assert call("initialize")["result"]["serverInfo"]["name"] == "travaso"
        assert len(call("tools/list")["result"]["tools"]) == 12  # 6 travaso + 6 memoria condivisa

        # 2. SCARICA: consultare consegna fatti nuovi; assorbirli li toglie dal serbatoio
        s = tool("travaso_cerca", query="naufragar mare")
        assert any("naufragar" in r["text"] for r in s["results"]), s
        again = tool("travaso_cerca", query="naufragar mare")
        assert again["results"] == [], "i fatti in viaggio non devono essere riconsegnati"
        a = tool("travaso_assorbi", ids=s["da_assorbire"], dove="test")
        assert a["assorbiti"] == len(s["da_assorbire"]) and a["fatti_nel_serbatoio"] == total - a["assorbiti"], a

        s = tool("travaso_cerca", query="Hz 432 Sol")
        assert any("432" in r["text"] for r in s["results"]), s
        tool("travaso_assorbi", ids=s["da_assorbire"], dove="test")

        # 3. SVUOTA: si continua fino a vuoto=true
        while True:
            r = tool("travaso_prossimi", limit=7)
            if r["results"]:
                tool("travaso_assorbi", ids=r["da_assorbire"], dove="test")
            if r["exhausted"]:
                break
        st = tool("travaso_stato")
        assert st["vuoto"] and st["fatti_nel_serbatoio"] == 0 and st["fatti_assorbiti_totali"] == total, st
        assert not list((tmp / "serbatoio").rglob("*.md")), "i file svuotati devono sparire"

        # 1. CONSERVA: l'archivio è intatto e consultabile
        arc = tool("travaso_archivio", query="ermo colle")
        assert any("ermo colle" in r["text"] for r in arc["results"]), arc
        assert len(list((tmp / "archivio").rglob("*.md"))) == 2

        # ripristino
        assert "error" in tool("travaso_ripristina", conferma=False)
        assert tool("travaso_ripristina", conferma=True)["fatti_nel_serbatoio"] == total
        p.kill()
        print(f"TUTTI I TEST OK ({total} fatti travasati e ripristinati)")
    finally:
        shutil.rmtree(tmp, ignore_errors=True)


if __name__ == "__main__":
    main()
